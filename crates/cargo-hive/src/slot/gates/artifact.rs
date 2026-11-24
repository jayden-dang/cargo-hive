//! Verifies the Cargo artifact records and executable bytes used by Nextest discovery.

use cargo_hive_platform::{flock::open_owner_state_file, slot::sha256_reader};
use cargo_hive_state::slots::MAX_LISTING_ARTIFACTS;
pub(crate) use cargo_hive_state::slots::{ListingArtifact, ReviewedTestBinary};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs::{self, Metadata},
    io,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(crate) const MAX_CARGO_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_CARGO_FRAME_BYTES: usize = 2 * 1024 * 1024;
const MAX_CARGO_FRAMES: usize = 32 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_TOTAL_EXECUTABLE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const HASH_DEADLINE: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ListingMode {
    Regular,
    Ignored,
}

#[derive(Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
enum CargoMessage {
    CompilerArtifact(Box<CargoArtifact>),
    CompilerMessage {},
    BuildScriptExecuted {},
    BuildFinished { success: bool },
}

#[derive(Deserialize)]
struct CargoArtifact {
    package_id: String,
    manifest_path: PathBuf,
    target: CargoTarget,
    profile: CargoProfile,
    features: Vec<String>,
    filenames: Vec<PathBuf>,
    executable: Option<PathBuf>,
    fresh: bool,
}

#[derive(Deserialize)]
struct CargoTarget {
    src_path: PathBuf,
    name: String,
    kind: Vec<String>,
    test: bool,
}

#[derive(Deserialize)]
struct CargoProfile {
    test: bool,
}

/// Binds the exact reviewed Nextest selection to results from the current Cargo invocation.
/// The caller must independently authorize all compilation and startup effects.
pub(crate) fn capture(
    stdout: &[u8],
    expected: &[ReviewedTestBinary],
    output_roots: &[PathBuf],
    retained: &[ListingArtifact],
) -> io::Result<Vec<ListingArtifact>> {
    if stdout.len() > MAX_CARGO_OUTPUT_BYTES
        || expected.is_empty()
        || expected.len() > MAX_LISTING_ARTIFACTS
        || retained.len() > MAX_LISTING_ARTIFACTS
        || output_roots.is_empty()
        || output_roots.len() > 2
    {
        return Err(invalid("Cargo artifact manifest exceeds its bounds"));
    }
    let output_roots = output_roots
        .iter()
        .map(|root| {
            let physical = fs::canonicalize(root)?;
            let info = fs::metadata(root)?;
            if root.as_os_str() != physical.as_os_str() || !info.is_dir() {
                return Err(invalid("Cargo output root is unproved"));
            }
            Ok((root, info.dev()))
        })
        .collect::<io::Result<Vec<_>>>()?;
    let mut pending = expected.iter().collect::<BTreeSet<_>>();
    if pending.len() != expected.len() {
        return Err(invalid(
            "reviewed test selection contains duplicate targets",
        ));
    }
    let stream = stdout
        .strip_suffix(b"\n")
        .ok_or_else(|| invalid("Cargo output has an incomplete JSON frame"))?;
    let started = Instant::now();
    let mut total_bytes = 0u64;
    let mut finished = false;
    let mut identities = BTreeSet::new();
    let mut binaries = Vec::with_capacity(expected.len());
    for (number, line) in stream.split(|byte| *byte == b'\n').enumerate() {
        if finished || number >= MAX_CARGO_FRAMES || line.len() > MAX_CARGO_FRAME_BYTES {
            return Err(invalid("Cargo artifact frame exceeds the stream contract"));
        }
        let message = serde_json::from_slice(line)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        match message {
            CargoMessage::CompilerArtifact(row) if row.profile.test => {
                let Some(executable) = row.executable else {
                    continue;
                };
                let target = ReviewedTestBinary {
                    package_id: row.package_id,
                    manifest_path: row.manifest_path,
                    source_path: row.target.src_path,
                    target_name: row.target.name,
                    target_kind: row.target.kind,
                    features: row.features,
                };
                if !row.target.test
                    || !target.source_path.is_absolute()
                    || !target.manifest_path.is_absolute()
                    || target.manifest_path.file_name() != Some(OsStr::new("Cargo.toml"))
                    || !row
                        .filenames
                        .iter()
                        .any(|file| file.as_os_str() == executable.as_os_str())
                    || !pending.remove(&target)
                {
                    return Err(invalid("Cargo produced an unreviewed test target"));
                }
                let (_, device) = output_roots
                    .iter()
                    .find(|(root, _)| executable.starts_with(root))
                    .ok_or_else(|| invalid("test executable is outside the reviewed outputs"))?;
                let (file, info) = open_executable(&executable)?;
                if info.dev() != *device || !identities.insert((info.dev(), info.ino())) {
                    return Err(invalid(
                        "test executable escaped its reviewed output or aliases another",
                    ));
                }
                total_bytes = total_bytes
                    .checked_add(info.len())
                    .filter(|total| *total <= MAX_TOTAL_EXECUTABLE_BYTES)
                    .ok_or_else(|| invalid("test executables exceed their aggregate byte bound"))?;
                let sha256 = hash_executable(&executable, file, &info, started)?;
                let artifact = ListingArtifact {
                    target,
                    executable,
                    sha256,
                    device: info.dev(),
                    inode: info.ino(),
                    bytes: info.len(),
                    mode: info.mode(),
                };
                if row.fresh && !retained.contains(&artifact) {
                    return Err(invalid(
                        "reused test executable has no matching retained receipt",
                    ));
                }
                binaries.push(artifact);
            }
            CargoMessage::BuildFinished { success: true } => finished = true,
            CargoMessage::BuildFinished { success: false } => {
                return Err(invalid("Cargo reported an unsuccessful build"));
            }
            CargoMessage::CompilerArtifact(_)
            | CargoMessage::CompilerMessage {}
            | CargoMessage::BuildScriptExecuted {} => {}
        }
    }
    if !finished || !pending.is_empty() {
        return Err(invalid(
            "Cargo artifact manifest is missing reviewed targets or completion",
        ));
    }
    binaries.sort_unstable_by(|left, right| left.target.cmp(&right.target));
    Ok(binaries)
}

/// Verifies the exact discovery invocation and rechecks its executable before launch.
pub(crate) fn verify_listing(
    artifacts: &[ListingArtifact],
    executable: &Path,
    cwd: &Path,
    args: &[OsString],
) -> io::Result<ListingMode> {
    let mode = match args {
        [list, format, terse] if list == "--list" && format == "--format" && terse == "terse" => {
            ListingMode::Regular
        }
        [list, format, terse, ignored]
            if list == "--list"
                && format == "--format"
                && terse == "terse"
                && ignored == "--ignored" =>
        {
            ListingMode::Ignored
        }
        _ => {
            return Err(invalid(
                "Nextest requested an unreviewed discovery invocation",
            ));
        }
    };
    if artifacts.len() > MAX_LISTING_ARTIFACTS
        || fs::canonicalize(cwd)?.as_os_str() != cwd.as_os_str()
    {
        return Err(invalid(
            "Nextest discovery manifest or working directory is unproved",
        ));
    }
    let mut selected = artifacts.iter().filter(|artifact| {
        artifact.executable.as_os_str() == executable.as_os_str()
            && artifact.target.manifest_path.parent().map(Path::as_os_str) == Some(cwd.as_os_str())
    });
    let artifact = selected
        .next()
        .filter(|_| selected.next().is_none())
        .ok_or_else(|| {
            invalid("Nextest requested an unreviewed executable or working directory")
        })?;
    let started = Instant::now();
    let (file, info) = open_executable(executable)?;
    if info.dev() != artifact.device
        || info.ino() != artifact.inode
        || info.len() != artifact.bytes
        || info.mode() != artifact.mode
        || hash_executable(executable, file, &info, started)? != artifact.sha256
    {
        return Err(invalid(
            "Nextest discovery executable changed after compilation",
        ));
    }
    Ok(mode)
}

fn open_executable(path: &Path) -> io::Result<(fs::File, Metadata)> {
    if !path.is_absolute() || fs::canonicalize(path)?.as_os_str() != path.as_os_str() {
        return Err(invalid("test executable path is not physical"));
    }
    let file = open_owner_state_file(path)?;
    let info = file.metadata()?;
    if info.mode() & 0o100 == 0 || info.len() == 0 || info.len() > MAX_EXECUTABLE_BYTES {
        return Err(invalid("test executable mode or byte length is unproved"));
    }
    Ok((file, info))
}

fn hash_executable(
    path: &Path,
    mut file: fs::File,
    info: &Metadata,
    started: Instant,
) -> io::Result<String> {
    let bytes = sha256_reader(&mut file, info.len(), || started.elapsed() >= HASH_DEADLINE)?;
    let stamp = |meta: &Metadata| {
        (
            meta.dev(),
            meta.ino(),
            meta.mode(),
            meta.uid(),
            meta.nlink(),
            meta.len(),
            meta.mtime(),
            meta.mtime_nsec(),
            meta.ctime(),
            meta.ctime_nsec(),
        )
    };
    let expected = stamp(info);
    if stamp(&file.metadata()?) != expected
        || stamp(&fs::symlink_metadata(path)?) != expected
        || fs::canonicalize(path)?.as_os_str() != path.as_os_str()
    {
        return Err(invalid("test executable changed while hashing"));
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use cargo_hive_platform::host_fs;
    use serde_json::{Value, json};
    use std::{
        fs::{self, Permissions},
        os::unix::fs::PermissionsExt as _,
    };

    fn target(cwd: &Path) -> ReviewedTestBinary {
        ReviewedTestBinary {
            package_id: "path+file:///workspace/gateways#0.1.0".into(),
            manifest_path: cwd.join("Cargo.toml"),
            source_path: cwd.join("tests/listing.rs"),
            target_name: "listing".into(),
            target_kind: vec!["test".into()],
            features: vec!["extra".into()],
        }
    }

    fn artifact_row(target: &ReviewedTestBinary, executable: &Path) -> Value {
        json!({
            "reason": "compiler-artifact",
            "package_id": target.package_id,
            "manifest_path": target.manifest_path,
            "target": {
                "src_path": target.source_path, "name": target.target_name,
                "kind": target.target_kind, "test": true
            },
            "profile": { "test": true },
            "features": target.features,
            "filenames": [executable],
            "executable": executable,
            "fresh": false
        })
    }

    fn stream(row: &Value) -> Vec<u8> {
        format!("{row}\n{{\"reason\":\"build-finished\",\"success\":true}}\n").into_bytes()
    }

    fn listing_fixture(root: &Path) -> (PathBuf, ReviewedTestBinary, Value) {
        let executable = root.join("listing-334455");
        fs::write(&executable, b"abc").unwrap();
        fs::set_permissions(&executable, Permissions::from_mode(0o700)).unwrap();
        let target = target(root);
        let row = artifact_row(&target, &executable);
        (executable, target, row)
    }

    #[test]
    fn exactly_reviewed_compiler_artifact_authorizes_both_discovery_forms() {
        let scratch = host_fs()
            .temp_dir(Path::new("/private/tmp"), "hive-it-nextest-artifact-")
            .unwrap();
        let root = scratch.path();
        let (executable, target, row) = listing_fixture(root);
        let report = stream(&row);
        let artifacts = capture(&report, &[target], &[root.into()], &[]).unwrap();

        let stat = executable.metadata().unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].device, stat.dev());
        assert_eq!(artifacts[0].inode, stat.ino());
        assert_eq!(artifacts[0].bytes, 3);
        assert_eq!(
            artifacts[0].sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        for (args, mode) in [
            (vec!["--list", "--format", "terse"], ListingMode::Regular),
            (
                vec!["--list", "--format", "terse", "--ignored"],
                ListingMode::Ignored,
            ),
        ] {
            let args = args.into_iter().map(OsString::from).collect::<Vec<_>>();
            assert_eq!(
                verify_listing(&artifacts, &executable, root, &args).unwrap(),
                mode
            );
        }
    }

    #[test]
    fn cargo_receipt_requires_the_reviewed_selection_and_complete_json_frames() {
        let scratch = host_fs()
            .temp_dir(Path::new("/private/tmp"), "hive-it-nextest-artifact-")
            .unwrap();
        let root = scratch.path();
        let (_, target, row) = listing_fixture(root);
        let mut unreviewed = row.clone();
        unreviewed["features"] = json!(["other"]);
        let mut duplicate = format!("{row}\n").into_bytes();
        duplicate.extend(stream(&row));
        let mut trailing = stream(&row);
        trailing.extend(b"{\"reason\":\"compiler-message\"}\n");
        let mut partial = stream(&row);
        partial.pop();

        for report in [
            stream(&unreviewed),
            duplicate,
            trailing,
            partial,
            format!("{row}\n").into_bytes(),
        ] {
            assert!(capture(&report, std::slice::from_ref(&target), &[root.into()], &[]).is_err());
        }
    }

    #[test]
    fn reused_test_binary_requires_same_retained_file_and_target_receipt() {
        let scratch = host_fs()
            .temp_dir(Path::new("/private/tmp"), "hive-it-nextest-artifact-")
            .unwrap();
        let root = scratch.path();
        let (executable, target, mut row) = listing_fixture(root);
        let expected = [target];
        let roots = [root.to_path_buf()];
        let receipt = capture(&stream(&row), &expected, &roots, &[]).unwrap();
        row["fresh"] = json!(true);
        let fresh = stream(&row);

        assert!(capture(&fresh, &expected, &roots, &[]).is_err());
        assert_eq!(
            capture(&fresh, &expected, &roots, &receipt).unwrap(),
            receipt
        );
        fs::write(executable, b"def").unwrap();
        assert!(capture(&fresh, &expected, &roots, &receipt).is_err());
    }

    #[test]
    fn listing_refuses_unreviewed_modes_directories_and_changed_executables() {
        let scratch = host_fs()
            .temp_dir(Path::new("/private/tmp"), "hive-it-nextest-artifact-")
            .unwrap();
        let root = scratch.path();
        let (executable, target, row) = listing_fixture(root);
        let receipts = capture(&stream(&row), &[target], &[root.into()], &[]).unwrap();
        let regular = ["--list", "--format", "terse"].map(OsString::from);
        let other_args = ["--list", "--format", "pretty"].map(OsString::from);
        assert!(verify_listing(&receipts, &executable, root, &other_args).is_err());
        assert!(
            verify_listing(&receipts, &executable, Path::new("/private/tmp"), &regular).is_err()
        );

        fs::set_permissions(&executable, Permissions::from_mode(0o755)).unwrap();
        assert!(verify_listing(&receipts, &executable, root, &regular).is_err());
        fs::set_permissions(&executable, Permissions::from_mode(0o700)).unwrap();
        fs::write(&executable, b"def").unwrap();
        assert!(verify_listing(&receipts, &executable, root, &regular).is_err());

        let replacement = root.join("replacement");
        fs::write(&replacement, b"abc").unwrap();
        fs::set_permissions(&replacement, Permissions::from_mode(0o700)).unwrap();
        fs::rename(replacement, &executable).unwrap();
        assert!(verify_listing(&receipts, &executable, root, &regular).is_err());
    }
}
