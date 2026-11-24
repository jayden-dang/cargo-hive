//! Reviewed aggregate checkout tree evidence.

use super::{Refusal, SourceStamp, digest, refused};
use crate::slot::{canonical_worktree_path, check_cancelled, directory_stamp, git_error};
use cargo_hive_cargo::git::Git;
use cargo_hive_platform::{flock::open_owner_state_file, signals::cancelled, slot::sha256_reader};
use cargo_hive_state::{StateError, slots};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

const TREE_LIMIT: usize = 2 * 1024 * 1024;
const ENTRY_LIMIT: usize = 16_384;
const PATH_LIMIT: usize = 4_096;
const DEPTH_LIMIT: usize = 32;
const FILE_LIMIT: u64 = 8 * 1024 * 1024;
const CONTENT_LIMIT: u64 = 128 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TreeManifest {
    entries: usize,
    sha256: String,
}

impl TreeManifest {
    pub(super) fn valid(&self) -> bool {
        (1..=ENTRY_LIMIT).contains(&self.entries) && slots::hex(&self.sha256, 64)
    }
}

struct TreeEntry<'a> {
    row: &'a [u8],
    path: &'a Path,
    executable: bool,
}

fn tree_refusal() -> Refusal {
    refused("Could not verify the reviewed worktree inventory, file modes or contents.")
}

fn tree_entries(
    listing: &[u8],
    entries: usize,
    oid_len: usize,
) -> Result<Vec<TreeEntry<'_>>, Refusal> {
    if listing.len() > TREE_LIMIT || !(1..=ENTRY_LIMIT).contains(&entries) {
        return Err(tree_refusal());
    }
    let body = listing.strip_suffix(b"\0").ok_or_else(tree_refusal)?;
    let mut paths = BTreeSet::new();
    let mut parsed = Vec::with_capacity(entries);
    for row in body.split(|byte| *byte == 0) {
        check_cancelled()?;
        if parsed.len() == entries {
            return Err(tree_refusal());
        }
        let text = std::str::from_utf8(row).map_err(|_| tree_refusal())?;
        let (attrs, name) = text.split_once('\t').ok_or_else(tree_refusal)?;
        let mut fields = attrs.split(' ');
        let executable = match (fields.next(), fields.next(), fields.next(), fields.next()) {
            (Some(mode @ ("100644" | "100755")), Some("blob"), Some(oid), None)
                if oid.len() == oid_len && slots::object_id(oid) =>
            {
                mode == "100755"
            }
            _ => return Err(tree_refusal()),
        };
        if name.len() > PATH_LIMIT
            || name.chars().any(char::is_control)
            || name.split('/').count() > DEPTH_LIMIT
            || name.split('/').any(|component| {
                component.is_empty()
                    || matches!(component, "." | "..")
                    || component.eq_ignore_ascii_case(".git")
            })
            || !paths.insert(name)
        {
            return Err(tree_refusal());
        }
        parsed.push(TreeEntry {
            row,
            path: Path::new(name),
            executable,
        });
    }
    if parsed.len() != entries {
        return Err(tree_refusal());
    }
    Ok(parsed)
}

fn stamp(metadata: &Metadata) -> SourceStamp {
    (directory_stamp(metadata), metadata.len(), metadata.mode())
}

fn checked_directories(
    path: &Path,
    device: u64,
    directories: &mut BTreeMap<PathBuf, SourceStamp>,
    identities: &mut BTreeSet<slots::FileIdentity>,
) -> Result<(), Refusal> {
    for directory in path.ancestors() {
        if directories.contains_key(directory) {
            break;
        }
        check_cancelled()?;
        if directories.len() >= ENTRY_LIMIT
            || directory.canonicalize().ok().as_deref() != Some(directory)
        {
            return Err(tree_refusal());
        }
        let metadata = slots::owner_directory(directory).map_err(|_| tree_refusal())?;
        let observation = stamp(&metadata);
        if metadata.dev() != device || !identities.insert(identity(&observation)) {
            return Err(tree_refusal());
        }
        directories.insert(directory.to_path_buf(), observation);
    }
    Ok(())
}

fn identity(((identity, ..), ..): &SourceStamp) -> slots::FileIdentity {
    *identity
}

fn verified_file(
    path: &Path,
    executable: Option<bool>,
    device: u64,
    limit: u64,
    budget: &mut u64,
) -> Result<(String, SourceStamp), Refusal> {
    let mut file = open_owner_state_file(path).map_err(|_| tree_refusal())?;
    let metadata = file.metadata().map_err(|_| tree_refusal())?;
    if metadata.dev() != device
        || metadata.len() > limit
        || executable.is_some_and(|mode| mode != (metadata.mode() & 0o100 != 0))
    {
        return Err(tree_refusal());
    }
    *budget = budget
        .checked_sub(metadata.len())
        .ok_or_else(tree_refusal)?;
    let before = stamp(&metadata);
    let hash = sha256_reader(&mut file, metadata.len(), cancelled).map_err(|err| {
        if err.kind() == io::ErrorKind::Interrupted && cancelled() {
            StateError::Cancelled.into()
        } else {
            tree_refusal()
        }
    })?;
    let named = fs::symlink_metadata(path).map_err(|_| tree_refusal())?;
    if !named.is_file()
        || stamp(&named) != before
        || stamp(&file.metadata().map_err(|_| tree_refusal())?) != before
    {
        return Err(tree_refusal());
    }
    let hash = hash.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok((hash, before))
}

fn check_absent(path: &Path) -> Result<PathBuf, Refusal> {
    if canonical_worktree_path(path).ok().as_deref() != Some(path) {
        return Err(tree_refusal());
    }
    for candidate in path.ancestors() {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if candidate != path && metadata.is_dir() => {
                return Ok(candidate.to_path_buf());
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            _ => return Err(tree_refusal()),
        }
    }
    Err(tree_refusal())
}

struct TreeSnapshot<'a> {
    root: &'a Path,
    device: u64,
    directories: BTreeMap<PathBuf, SourceStamp>,
    files: BTreeMap<PathBuf, SourceStamp>,
    identities: BTreeSet<slots::FileIdentity>,
    absences: Vec<PathBuf>,
    remaining_bytes: u64,
}

impl<'a> TreeSnapshot<'a> {
    fn open(root: &'a Path) -> Result<Self, Refusal> {
        let metadata = slots::owner_directory(root).map_err(|_| tree_refusal())?;
        if root.canonicalize().ok().as_deref() != Some(root) {
            return Err(tree_refusal());
        }
        let observation = stamp(&metadata);
        Ok(Self {
            root,
            device: metadata.dev(),
            directories: BTreeMap::from([(root.to_path_buf(), observation)]),
            files: BTreeMap::new(),
            identities: BTreeSet::from([identity(&observation)]),
            absences: Vec::new(),
            remaining_bytes: CONTENT_LIMIT,
        })
    }

    fn observe_parents(&mut self, parent: &Path) -> Result<(), Refusal> {
        if !parent.starts_with(self.root) {
            return Err(tree_refusal());
        }
        checked_directories(
            parent,
            self.device,
            &mut self.directories,
            &mut self.identities,
        )
    }

    fn file(
        &mut self,
        path: PathBuf,
        executable: Option<bool>,
        limit: u64,
    ) -> Result<String, Refusal> {
        self.observe_parents(path.parent().ok_or_else(tree_refusal)?)?;
        if path.canonicalize().ok().as_ref() != Some(&path) {
            return Err(tree_refusal());
        }
        let (hash, observation) = verified_file(
            &path,
            executable,
            self.device,
            limit,
            &mut self.remaining_bytes,
        )?;
        if !self.identities.insert(identity(&observation)) {
            return Err(tree_refusal());
        }
        self.files.insert(path, observation);
        Ok(hash)
    }

    fn absent(&mut self, path: PathBuf) -> Result<(), Refusal> {
        self.observe_parents(&check_absent(&path)?)?;
        self.absences.push(path);
        Ok(())
    }

    fn verify(self) -> Result<(), Refusal> {
        for (path, observation) in self.directories.iter().chain(&self.files) {
            check_cancelled()?;
            let metadata = fs::symlink_metadata(path).map_err(|_| tree_refusal())?;
            if stamp(&metadata) != *observation {
                return Err(tree_refusal());
            }
        }
        for path in self.absences {
            check_cancelled()?;
            check_absent(&path)?;
        }
        Ok(())
    }
}

pub(super) fn verify_whole_tree(
    root: &Path,
    commit: &str,
    manifest: &TreeManifest,
    variants: &BTreeMap<PathBuf, Option<String>>,
) -> Result<(), Refusal> {
    let listing = Git::at(root)
        .slot_read_only(&["ls-tree", "-r", "-z", "--full-tree", commit], TREE_LIMIT)
        .map_err(|err| git_error(root, err))?;
    let entries = tree_entries(&listing, manifest.entries, commit.len())?;
    let mut snapshot = TreeSnapshot::open(root)?;
    let mut untracked_variants = variants.clone();

    // Reviewers build this independently from the Git blob bytes. A declared
    // mutable row keeps its exact committed path, mode and object ID, but leaves
    // the hash field empty. The finite variant review checks its current bytes.
    let mut evidence = b"cargo-hive/slot/checkout-tree/v1\0".to_vec();
    for entry in entries {
        check_cancelled()?;
        let path = root.join(entry.path);
        evidence.extend_from_slice(entry.row);
        evidence.push(0);
        match untracked_variants.remove(entry.path) {
            Some(None) => snapshot.absent(path)?,
            Some(Some(expected)) => {
                if snapshot.file(path, Some(entry.executable), FILE_LIMIT)? != expected {
                    return Err(tree_refusal());
                }
            }
            None => {
                let hash = snapshot.file(path, Some(entry.executable), FILE_LIMIT)?;
                evidence.extend_from_slice(hash.as_bytes());
            }
        }
        evidence.push(0);
    }
    for (path, expected) in untracked_variants {
        check_cancelled()?;
        if !super::relative(&path) {
            return Err(tree_refusal());
        }
        let path = root.join(path);
        if let Some(expected) = expected {
            if snapshot.file(path, None, slots::MAX_BYTES)? != expected {
                return Err(tree_refusal());
            }
        } else {
            snapshot.absent(path)?;
        }
    }
    snapshot.verify()?;
    if digest(&evidence)? != manifest.sha256 {
        return Err(tree_refusal());
    }
    check_cancelled()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{
        ENTRY_LIMIT, FILE_LIMIT, TREE_LIMIT, TreeManifest, TreeSnapshot, identity, stamp,
        tree_entries, verified_file, verify_whole_tree,
    };
    use crate::slot::profile::digest;
    use cargo_hive_platform::{
        host_fs,
        proc::{Capture, Spec, run_bounded_complete},
    };
    use std::collections::BTreeMap;
    use std::fs::{self, FileTimes, OpenOptions, Permissions};
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    fn git(root: &Path, args: &[&str]) -> Vec<u8> {
        let mut spec = Spec::new("/usr/bin/git");
        spec.cwd = Some(root.to_path_buf());
        spec.env_clear = true;
        spec.env_set = [
            ("PATH", "/usr/bin:/bin"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect();
        spec.args = [
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "gc.auto=0",
            "-c",
            "maintenance.auto=false",
        ]
        .into_iter()
        .chain(args.iter().copied())
        .map(Into::into)
        .collect();
        let output = run_bounded_complete(
            &spec,
            Duration::from_secs(10),
            Capture {
                stdout_cap: TREE_LIMIT,
                stderr_cap: 4096,
            },
            || false,
        )
        .unwrap();
        assert!(
            output.status.success(),
            "fixture git operation failed: {}",
            String::from_utf8_lossy(&output.stderr_tail),
        );
        output.stdout
    }

    #[test]
    fn a_reviewed_large_tree_requires_exact_immutable_bytes_and_mode() {
        let scratch = host_fs()
            .temp_dir(Path::new("/private/tmp"), "hive-it-slot-tree-")
            .unwrap();
        let root = scratch.path();
        git(root, &["init", "-q"]);
        let bytes = b"immutable source material\n";
        for index in 0..300 {
            let name = format!("{index:04}-{}", "reviewed-file-".repeat(15));
            fs::write(root.join(name), bytes).unwrap();
        }
        let directory = root.join("nested");
        fs::create_dir(&directory).unwrap();
        let edited = directory.join("portable");
        fs::write(&edited, bytes).unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "reviewed endpoint"]);
        let commit = String::from_utf8(git(root, &["rev-parse", "HEAD"])).unwrap();
        let commit = commit.trim_end();
        let listing = git(root, &["ls-tree", "-r", "-z", "--full-tree", commit]);
        assert!(listing.len() > 64 * 1024);
        assert_eq!(listing.iter().filter(|byte| **byte == 0).count(), 301);

        let file_hash = digest(bytes).unwrap_or_else(|_| panic!("fixture digest"));
        let mut evidence = b"cargo-hive/slot/checkout-tree/v1\0".to_vec();
        for entry in listing
            .strip_suffix(b"\0")
            .unwrap()
            .split(|byte| *byte == 0)
        {
            evidence.extend_from_slice(entry);
            evidence.push(0);
            evidence.extend_from_slice(file_hash.as_bytes());
            evidence.push(0);
        }
        let manifest = TreeManifest {
            entries: 301,
            sha256: digest(&evidence).unwrap_or_else(|_| panic!("fixture manifest digest")),
        };
        let variants = BTreeMap::<PathBuf, Option<String>>::new();
        let verify = || verify_whole_tree(root, commit, &manifest, &variants);
        assert!(verify().is_ok(), "the reviewed large tree should qualify");

        let same_time = fs::metadata(&edited).unwrap().modified().unwrap();
        fs::write(&edited, b"x".repeat(bytes.len())).unwrap();
        OpenOptions::new()
            .write(true)
            .open(&edited)
            .unwrap()
            .set_times(FileTimes::new().set_modified(same_time))
            .unwrap();
        assert_eq!(
            fs::metadata(&edited).unwrap().modified().unwrap(),
            same_time
        );
        assert!(
            verify().is_err(),
            "changed bytes with restored mtime must refuse"
        );
        fs::write(&edited, bytes).unwrap();
        assert!(
            verify().is_ok(),
            "restoring the approved bytes should qualify"
        );
        fs::set_permissions(&edited, Permissions::from_mode(0o755)).unwrap();
        assert!(verify().is_err(), "an undeclared execute bit must refuse");
        fs::set_permissions(&edited, Permissions::from_mode(0o644)).unwrap();
        let same_bytes = directory.join("same-bytes");
        fs::rename(&edited, &same_bytes).unwrap();
        symlink(&same_bytes, &edited).unwrap();
        assert!(verify().is_err(), "a link to identical bytes must refuse");
    }

    #[test]
    fn a_reviewed_variant_must_match_the_checked_presence_bytes_and_mode() {
        let scratch = host_fs()
            .temp_dir(Path::new("/private/tmp"), "hive-it-slot-variant-")
            .unwrap();
        let root = scratch.path();
        git(root, &["init", "-q"]);
        let variant = root.join("data");
        fs::write(&variant, b"original").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "reviewed variant endpoint"]);
        let commit = String::from_utf8(git(root, &["rev-parse", "HEAD"])).unwrap();
        let commit = commit.trim_end();
        let listing = git(root, &["ls-tree", "-r", "-z", "--full-tree", commit]);
        let mut evidence = b"cargo-hive/slot/checkout-tree/v1\0".to_vec();
        evidence.extend_from_slice(&listing);
        evidence.push(0);
        let manifest = TreeManifest {
            entries: 1,
            sha256: digest(&evidence).unwrap_or_else(|_| panic!("fixture manifest digest")),
        };

        let approved = b"accepted variant";
        let checked = digest(approved).unwrap_or_else(|_| panic!("fixture variant digest"));
        let present = BTreeMap::from([(PathBuf::from("data"), Some(checked))]);
        let absent = BTreeMap::from([(PathBuf::from("data"), None)]);
        fs::write(&variant, approved).unwrap();
        assert!(verify_whole_tree(root, commit, &manifest, &present).is_ok());
        fs::write(&variant, b"unexpected input").unwrap();
        assert!(
            verify_whole_tree(root, commit, &manifest, &present).is_err(),
            "a tracked variant changed after its finite check must refuse"
        );
        assert!(verify_whole_tree(root, commit, &manifest, &absent).is_err());
        fs::remove_file(&variant).unwrap();
        assert!(verify_whole_tree(root, commit, &manifest, &absent).is_ok());
        assert!(verify_whole_tree(root, commit, &manifest, &present).is_err());

        let mut with_untracked = absent.clone();
        let expected = digest(b"untracked").unwrap_or_else(|_| panic!("fixture variant digest"));
        with_untracked.insert(PathBuf::from("optional"), Some(expected));
        fs::write(root.join("optional"), b"untracked").unwrap();
        assert!(verify_whole_tree(root, commit, &manifest, &with_untracked).is_ok());
        fs::write(root.join("optional"), b"surprised").unwrap();
        assert!(
            verify_whole_tree(root, commit, &manifest, &with_untracked).is_err(),
            "an untracked variant changed after its finite check must refuse"
        );
    }

    #[test]
    fn tree_records_require_exact_counts_modes_and_complete_bounded_input() {
        let entry = |mode: &str, oid: &str, name: &str| format!("{mode} blob {oid}\t{name}\0");
        let oid = "a".repeat(40);
        let base = entry("100644", &oid, "owned");
        assert!(tree_entries(base.as_bytes(), 1, 40).is_ok());
        assert!(tree_entries(base.as_bytes(), 2, 40).is_err());
        assert!(tree_entries(&base.as_bytes()[..base.len() - 1], 1, 40).is_err());
        assert!(tree_entries(base.repeat(2).as_bytes(), 2, 40).is_err());
        assert!(tree_entries(entry("120000", &oid, "owned").as_bytes(), 1, 40).is_err());
        assert!(tree_entries(entry("100644", &oid, "../other").as_bytes(), 1, 40).is_err());
        assert!(tree_entries(entry("100644", &oid, ".Git/config").as_bytes(), 1, 40).is_err());
        assert!(tree_entries(entry("100644", &oid, "alias//file").as_bytes(), 1, 40).is_err());
        assert!(tree_entries(entry("100644", &oid[..39], "owned").as_bytes(), 1, 40).is_err());
        let mut at_entries = String::new();
        for index in 0..ENTRY_LIMIT {
            at_entries.push_str(&entry("100644", &oid, &index.to_string()));
        }
        assert!(tree_entries(at_entries.as_bytes(), ENTRY_LIMIT, 40).is_ok());
        at_entries.push_str(&base);
        assert!(tree_entries(at_entries.as_bytes(), ENTRY_LIMIT, 40).is_err());

        let leaf_length = 512;
        let mut at_bytes = String::new();
        while at_bytes.len() + 2 * (base.len() + leaf_length) < TREE_LIMIT {
            let prefix = at_bytes.len().to_string();
            let name = format!("{prefix}{}", "x".repeat(leaf_length - prefix.len()));
            at_bytes.push_str(&entry("100644", &oid, &name));
        }
        let row_offset = entry("100644", &oid, "").len();
        let final_length = TREE_LIMIT - at_bytes.len() - row_offset;
        at_bytes.push_str(&entry("100644", &oid, &"z".repeat(final_length)));
        assert_eq!(at_bytes.len(), TREE_LIMIT);
        let count = at_bytes
            .as_bytes()
            .iter()
            .filter(|byte| **byte == 0)
            .count();
        assert!(tree_entries(at_bytes.as_bytes(), count, 40).is_ok());
        at_bytes.push('\0');
        assert!(tree_entries(at_bytes.as_bytes(), count, 40).is_err());
    }

    #[test]
    fn byte_budget_exhaustion_refuses_before_hashing_more_source() {
        let scratch = host_fs()
            .temp_dir(Path::new("/private/tmp"), "hive-it-slot-byte-limit-")
            .unwrap();
        let leaf = scratch.path().join("source");
        fs::write(&leaf, b"1234").unwrap();
        let device = fs::metadata(&leaf).unwrap().dev();
        let mut remaining = 4;
        assert!(verified_file(&leaf, Some(false), device, FILE_LIMIT, &mut remaining).is_ok());
        assert_eq!(remaining, 0);
        assert!(verified_file(&leaf, Some(false), device, FILE_LIMIT, &mut remaining).is_err());

        let file = OpenOptions::new().write(true).open(&leaf).unwrap();
        file.set_len(FILE_LIMIT + 1).unwrap();
        let mut remaining = FILE_LIMIT + 1;
        assert!(verified_file(&leaf, Some(false), device, FILE_LIMIT, &mut remaining).is_err());
        assert_eq!(remaining, FILE_LIMIT + 1);
    }

    #[test]
    fn a_distinct_tree_name_cannot_reuse_an_observed_directory_identity() {
        let scratch = host_fs()
            .temp_dir(Path::new("/private/tmp"), "hive-it-slot-tree-alias-")
            .unwrap();
        let root = scratch.path();
        let directory = root.join("Directory");
        fs::create_dir(&directory).unwrap();
        let mut snapshot = TreeSnapshot::open(root).unwrap_or_else(|_| panic!("fixture root"));
        snapshot
            .observe_parents(&directory)
            .unwrap_or_else(|_| panic!("fixture directory"));
        assert!(
            snapshot.observe_parents(&directory).is_ok(),
            "an observed spelling can be reused"
        );

        // Model the namespace observation where an alias retained a different spelling.
        let observation = snapshot.directories.remove(&directory).unwrap();
        snapshot
            .directories
            .insert(root.join("directory"), observation);
        assert!(
            snapshot.observe_parents(&directory).is_err(),
            "a second name for the same physical directory must refuse"
        );

        let source = root.join("Source");
        fs::write(&source, b"input").unwrap();
        let observation = stamp(&fs::metadata(&source).unwrap());
        snapshot.files.insert(root.join("source"), observation);
        snapshot.identities.insert(identity(&observation));
        assert!(
            snapshot.file(source, Some(false), FILE_LIMIT).is_err(),
            "a second name for the same physical file must refuse"
        );
    }
}
