//! Cargo target dirs: identification by tag and fingerprint layout, and
//! ownership classification for discovered targets.

use crate::fingerprint::ProfileReport;
use serde::Serialize;
use std::cell::Cell;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub const TAG_LINE: &str = "# This file is a cache directory tag created by cargo.";

const MAX_TAG_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    Fingerprint,
    NewBuildDir,
}

fn is_dir(path: &Path) -> Result<bool, String> {
    match fs::metadata(path) {
        Ok(m) => Ok(m.is_dir()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn is_hex16(name: &str) -> bool {
    name.len() == 16
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn sorted_dirs(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => dirs.push(entry.path()),
            Ok(_) => {}
            Err(e) => return Err(format!("{}: {e}", entry.path().display())),
        }
    }
    dirs.sort();
    Ok(dirs)
}

type ListDirs<'a> = dyn Fn(&Path, usize) -> Result<Vec<PathBuf>, String> + 'a;
type IsDir<'a> = dyn Fn(&Path) -> Result<bool, String> + 'a;

/// `Some(layout)` when `dir` holds `.fingerprint/` or `build/<pkg>/<16 hex>/fingerprint/`.
fn profile_layout_with(
    dir: &Path,
    cap: usize,
    list: &ListDirs<'_>,
    is_dir: &IsDir<'_>,
) -> Result<Option<Layout>, String> {
    if is_dir(&dir.join(".fingerprint"))? {
        return Ok(Some(Layout::Fingerprint));
    }
    let build = dir.join("build");
    if !is_dir(&build)? {
        return Ok(None);
    }
    for pkg in list(&build, cap)? {
        for hash in list(&pkg, cap)? {
            let hex = hash
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_hex16);
            if hex && is_dir(&hash.join("fingerprint"))? {
                return Ok(Some(Layout::NewBuildDir));
            }
        }
    }
    Ok(None)
}

/// Profile dirs: direct children, or `<triple>/<profile>/` under `--target` builds.
fn profile_dirs(target: &Path) -> Result<Vec<(PathBuf, Layout)>, String> {
    profile_dirs_with(target, usize::MAX, &|path, _| sorted_dirs(path), &is_dir)
}

fn profile_dirs_with(
    target: &Path,
    cap: usize,
    list: &ListDirs<'_>,
    is_dir: &IsDir<'_>,
) -> Result<Vec<(PathBuf, Layout)>, String> {
    let mut out = Vec::new();
    for child in list(target, cap)? {
        if let Some(layout) = profile_layout_with(&child, cap, list, is_dir)? {
            out.push((child, layout));
            continue;
        }
        for grandchild in list(&child, cap)? {
            if let Some(layout) = profile_layout_with(&grandchild, cap, list, is_dir)? {
                out.push((grandchild, layout));
            }
        }
    }
    Ok(out)
}

/// `Some(Ok(profiles))` for a Cargo target dir. An oversized tag or a Cargo tag
/// with unreadable profiles yields `Some(Err(reason))`. Other dirs yield `None`.
pub fn identify(dir: &Path) -> Option<Result<Vec<(PathBuf, Layout)>, String>> {
    let path = dir.join("CACHEDIR.TAG");
    let body = fs::File::open(&path).and_then(read_tag);
    identify_tag(&path, body, || profile_dirs(dir))
}

/// Same as [`identify`], but every directory listing stops at `cap` entries.
pub fn identify_bounded(
    fs: &dyn cargo_hive_platform::Fs,
    dir: &Path,
    cap: usize,
) -> Option<Result<Vec<(PathBuf, Layout)>, String>> {
    let path = dir.join("CACHEDIR.TAG");
    let body = read_tag_at(|offset, len| fs.read_at(&path, offset, len));
    identify_tag(&path, body, || {
        profile_dirs_with(
            dir,
            cap,
            &|path, cap| bounded_dirs(fs, path, cap, None),
            &|path| is_dir_bounded(fs, path),
        )
    })
}

fn read_tag(reader: impl Read) -> io::Result<Vec<u8>> {
    let mut tag = Vec::new();
    reader
        .take((MAX_TAG_BYTES + 1) as u64)
        .read_to_end(&mut tag)?;
    Ok(tag)
}

fn read_tag_at(mut read: impl FnMut(u64, usize) -> io::Result<Vec<u8>>) -> io::Result<Vec<u8>> {
    let mut tag = Vec::new();
    while tag.len() <= MAX_TAG_BYTES {
        let part = read(tag.len() as u64, MAX_TAG_BYTES + 1 - tag.len())?;
        if part.is_empty() {
            break;
        }
        if tag.is_empty() {
            tag = part;
        } else {
            tag.extend_from_slice(&part);
        }
    }
    Ok(tag)
}

fn identify_tag(
    path: &Path,
    body: io::Result<Vec<u8>>,
    profiles: impl FnOnce() -> Result<Vec<(PathBuf, Layout)>, String>,
) -> Option<Result<Vec<(PathBuf, Layout)>, String>> {
    match valid_tag(&body.ok()?) {
        Ok(true) => finish_profiles(profiles()),
        Ok(false) => None,
        Err(reason) => Some(Err(format!("{}: {reason}", path.display()))),
    }
}

/// Identifies only regular, size-bounded Cargo tags and bounded profile directories.
pub fn identify_strict(
    fs: &dyn cargo_hive_platform::Fs,
    dir: &Path,
    cap: usize,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<(PathBuf, Layout)>, String> {
    const TAG_CAP: usize = 16 * 1024;
    let check_cancel = || {
        if cancel() {
            Err("Cargo profile identification was cancelled".to_string())
        } else {
            Ok(())
        }
    };
    check_cancel()?;
    let path = dir.join("CACHEDIR.TAG");
    let tag = fs
        .read_regular_bounded(&path, TAG_CAP)
        .map_err(|err| format!("{}: {err}", path.display()))?;
    if !valid_tag(&tag).map_err(|err| format!("{}: {err}", path.display()))? {
        return Err(format!("{} is not a Cargo target tag", path.display()));
    }
    let remaining = Cell::new(cap);
    let profiles = profile_dirs_with(
        dir,
        cap,
        &|path, cap| {
            check_cancel()?;
            bounded_dirs(fs, path, cap, Some(&remaining))
        },
        &|path| {
            check_cancel()?;
            is_dir_bounded(fs, path)
        },
    )?;
    if profiles.is_empty() {
        return Err(format!("{} has no supported Cargo profiles", dir.display()));
    }
    Ok(profiles)
}

fn valid_tag(tag: &[u8]) -> Result<bool, &'static str> {
    if tag.len() > MAX_TAG_BYTES {
        return Err("larger than 1 MiB; remove this invalid tag and build the target again");
    }
    Ok(String::from_utf8_lossy(tag)
        .lines()
        .any(|line| line.trim_end() == TAG_LINE))
}

fn finish_profiles(
    found: Result<Vec<(PathBuf, Layout)>, String>,
) -> Option<Result<Vec<(PathBuf, Layout)>, String>> {
    match found {
        Ok(profiles) if profiles.is_empty() => None,
        Ok(profiles) => Some(Ok(profiles)),
        Err(why) => Some(Err(why)),
    }
}

fn bounded_dirs(
    fs: &dyn cargo_hive_platform::Fs,
    dir: &Path,
    cap: usize,
    remaining: Option<&Cell<usize>>,
) -> Result<Vec<PathBuf>, String> {
    let cap = remaining.map_or(cap, Cell::get);
    let entries = fs
        .read_dir_bounded(dir, cap)
        .map_err(|err| format!("{}: {err}", dir.display()))?;
    if let Some(remaining) = remaining {
        let left = cap
            .checked_sub(entries.len())
            .ok_or_else(|| format!("{} has more than {cap} entries", dir.display()))?;
        remaining.set(left);
    }
    let mut dirs = Vec::new();
    for (name, kind) in entries {
        if kind == cargo_hive_platform::EntryKind::Dir {
            dirs.push(dir.join(name));
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn is_dir_bounded(fs: &dyn cargo_hive_platform::Fs, path: &Path) -> Result<bool, String> {
    match fs.entry_stat(path) {
        Ok(stat) => Ok(stat.kind == cargo_hive_platform::EntryKind::Dir),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

/// The realpath of `p`, or of its nearest existing ancestor with the missing tail appended.
pub fn realpath_lossy(p: &Path) -> PathBuf {
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cur = p;
    loop {
        if let Ok(real) = cur.canonicalize() {
            return tail.iter().rev().fold(real, |acc, name| acc.join(name));
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                cur = parent;
            }
            _ => return p.to_path_buf(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Owner {
    Owned {
        checkout: PathBuf,
    },
    Unresolved {
        reason: String,
    },
    OrphanCandidate {
        owned_under_caller_env: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct TargetDir {
    pub path: PathBuf,
    pub owner: Owner,
    pub profiles: Vec<ProfileReport>,
    pub size: Option<u64>,
    pub unreadable: Option<String>,
}

#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzzing {
    use super::*;

    pub fn target_tag(data: &[u8]) {
        let _ = valid_tag(data);
        let mut body = format!("# ignored\n{TAG_LINE}\n").into_bytes();
        body.extend_from_slice(data);
        let chunk = usize::from(data.first().copied().unwrap_or(0)) + 1;
        let read = read_tag_at(|offset, len| {
            let start = usize::try_from(offset).unwrap();
            let n = len.min(chunk).min(body.len().saturating_sub(start));
            Ok(body[start..start + n].to_vec())
        })
        .unwrap();
        assert_eq!(read, body);
        assert_eq!(valid_tag(&read), Ok(true));
        assert_eq!(read_tag(body.as_slice()).unwrap(), body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "hive-target-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn identify_bounded_matches_identify_at_the_cap_and_errors_one_past() {
        let root = scratch("bounded");
        tag(&root, TAG_LINE);
        fs::create_dir_all(root.join("debug/.fingerprint")).unwrap();
        fs::write(root.join("extra"), b"x").unwrap();
        let n = fs::read_dir(&root).unwrap().count();
        let open = identify(&root).unwrap().unwrap();
        let fs = cargo_hive_platform::current().unwrap().fs;
        let capped = identify_bounded(fs.as_ref(), &root, n).unwrap().unwrap();
        assert_eq!(open, capped);
        let err = identify_bounded(fs.as_ref(), &root, n - 1)
            .unwrap()
            .unwrap_err();
        assert!(err.contains("more than"), "{err}");
        assert!(identify(&root).is_some_and(|found| found.is_ok()));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn both_identifiers_refuse_oversized_tags_even_with_a_valid_prefix() {
        let root = scratch("tag-cap");
        mkdir(root.join("debug/.fingerprint"));
        let platform = cargo_hive_platform::current().unwrap();
        let tag_path = root.join("CACHEDIR.TAG");
        let mut body = format!("{TAG_LINE}\n").into_bytes();
        body.resize(1024 * 1024, b'#');
        fs::write(&tag_path, &body).unwrap();
        let exact = [
            identify(&root),
            identify_bounded(platform.fs.as_ref(), &root, 10),
        ];
        body.push(b'#');
        fs::write(&tag_path, &body).unwrap();
        let over = [
            identify(&root),
            identify_bounded(platform.fs.as_ref(), &root, 10),
        ];
        fs::remove_dir_all(&root).unwrap();

        for found in exact {
            assert_eq!(
                found,
                Some(Ok(vec![(root.join("debug"), Layout::Fingerprint)]))
            );
        }
        assert!(
            over.iter().all(|found| matches!(found, Some(Err(why)) if why.contains("CACHEDIR.TAG") && why.contains("larger than 1 MiB") && why.contains("build the target again"))),
            "{over:?}"
        );
    }

    #[test]
    fn positional_tag_reads_accumulate_short_chunks_and_preserve_io_failure_mapping() {
        for data in [&b""[..], &b"\xff\nnoise"[..], &b"123456789"[..]] {
            fuzzing::target_tag(data);
        }
        let mut calls = 0;
        let body = read_tag_at(|_, _| {
            calls += 1;
            if calls == 1 {
                Ok(format!("{TAG_LINE}\n").into_bytes())
            } else {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            }
        });
        assert!(
            identify_tag(Path::new("CACHEDIR.TAG"), body, || panic!(
                "cannot inspect profiles after read failure"
            ))
            .is_none()
        );
    }

    #[test]
    fn tag_prefixes_become_recognized_only_at_the_complete_identifying_line() {
        let body = format!("# comment\n{TAG_LINE}\n# trailing comment\n");
        let identifying_end = "# comment\n".len() + TAG_LINE.len();
        for end in 0..=body.len() {
            assert_eq!(
                valid_tag(&body.as_bytes()[..end]),
                Ok(end >= identifying_end)
            );
        }
    }

    #[test]
    fn strict_identification_counts_entries_across_the_whole_profile_search() {
        let root = scratch("strict-budget");
        tag(&root, TAG_LINE);
        fs::create_dir_all(root.join("triple/debug/.fingerprint")).unwrap();
        let fs = cargo_hive_platform::current().unwrap().fs;

        assert_eq!(
            identify_strict(fs.as_ref(), &root, 3, &|| false).unwrap(),
            [(root.join("triple/debug"), Layout::Fingerprint)]
        );
        let error = identify_strict(fs.as_ref(), &root, 2, &|| false).unwrap_err();
        assert!(error.contains("more than"), "{error}");

        let _ = std::fs::remove_dir_all(root);
    }

    fn tag(dir: &Path, line: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("CACHEDIR.TAG"),
            format!("Signature: 8a477f597d28d172789f06886806bc55\n{line}\n"),
        )
        .unwrap();
    }

    fn mkdir(p: PathBuf) {
        fs::create_dir_all(p).unwrap();
    }

    #[test]
    fn a_target_needs_cargos_tag_line_and_a_profile_with_fingerprints() {
        let root = scratch("identify");
        let plain = root.join("plain");
        mkdir(plain.join("debug/.fingerprint"));
        assert_eq!(identify(&plain), None, "no tag");
        let other = root.join("other");
        tag(
            &other,
            "# This file is a cache directory tag created by someone else.",
        );
        mkdir(other.join("debug/.fingerprint"));
        assert_eq!(identify(&other), None, "someone else's tag");
        let artifacts = root.join("artifacts");
        tag(&artifacts, TAG_LINE);
        mkdir(artifacts.join("debug/deps"));
        assert_eq!(
            identify(&artifacts),
            None,
            "uplifted artifacts only (a target dir whose build dir is elsewhere)"
        );
        let old = root.join("old");
        tag(&old, TAG_LINE);
        for p in [
            "debug/.fingerprint",
            "release/.fingerprint",
            "aarch64-apple-darwin/debug/.fingerprint",
            "doc/fixture",
        ] {
            mkdir(old.join(p));
        }
        assert_eq!(
            identify(&old),
            Some(Ok(vec![
                (old.join("aarch64-apple-darwin/debug"), Layout::Fingerprint),
                (old.join("debug"), Layout::Fingerprint),
                (old.join("release"), Layout::Fingerprint)
            ]))
        );
        let new = root.join("new");
        tag(&new, TAG_LINE);
        mkdir(new.join("debug/build/serde/0123456789abcdef/fingerprint"));
        assert_eq!(
            identify(&new),
            Some(Ok(vec![(new.join("debug"), Layout::NewBuildDir)]))
        );
        let not_hash = root.join("not-hash");
        tag(&not_hash, TAG_LINE);
        mkdir(not_hash.join("debug/build/serde-0123/out"));
        assert_eq!(identify(&not_hash), None);
        let locked = root.join("locked");
        tag(&locked, TAG_LINE);
        mkdir(locked.join("debug/.fingerprint"));
        fs::set_permissions(locked.join("debug"), fs::Permissions::from_mode(0o000)).unwrap();
        let got = identify(&locked);
        fs::set_permissions(locked.join("debug"), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            matches!(&got, Some(Err(why)) if why.contains("debug")),
            "{got:?}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unreadable_package_dir_is_not_treated_as_not_a_target() {
        let root = scratch("unreadable-pkg");
        let dir = root.join("target");
        tag(&dir, TAG_LINE);
        let pkg = dir.join("debug/build/serde");
        mkdir(pkg.join("0123456789abcdef/fingerprint"));
        fs::set_permissions(&pkg, fs::Permissions::from_mode(0o000)).unwrap();
        let got = identify(&dir);
        fs::set_permissions(&pkg, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(&got, Some(Err(why)) if !why.is_empty()), "{got:?}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_missing_tail_is_appended_to_the_realpath_of_its_nearest_existing_ancestor() {
        let root = scratch("lossy");
        assert_eq!(
            realpath_lossy(&root.join("a/b")),
            root.canonicalize().unwrap().join("a/b")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
