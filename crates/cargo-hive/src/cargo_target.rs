//! Cargo target dirs: identification by tag and fingerprint layout, and
//! ownership classification for discovered targets.

use crate::fingerprint::ProfileReport;
use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const TAG_LINE: &str = "# This file is a cache directory tag created by cargo.";

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

/// `Some(Ok(profiles))` for a Cargo target dir; `Some(Err(reason))` for a dir
/// Cargo tagged whose profiles cannot be read; `None` for anything else.
pub fn identify(dir: &Path) -> Option<Result<Vec<(PathBuf, Layout)>, String>> {
    let tag = fs::read(dir.join("CACHEDIR.TAG")).ok()?;
    if !valid_tag(&tag) {
        return None;
    }
    finish_profiles(profile_dirs(dir))
}

/// Same as [`identify`], but every directory listing stops at `cap` entries.
pub fn identify_bounded(
    fs: &dyn crate::platform::Fs,
    dir: &Path,
    cap: usize,
) -> Option<Result<Vec<(PathBuf, Layout)>, String>> {
    let tag = fs.read(&dir.join("CACHEDIR.TAG")).ok()?;
    if !valid_tag(&tag) {
        return None;
    }
    finish_profiles(profile_dirs_with(
        dir,
        cap,
        &|path, cap| bounded_dirs(fs, path, cap),
        &|path| is_dir_bounded(fs, path),
    ))
}

fn valid_tag(tag: &[u8]) -> bool {
    String::from_utf8_lossy(tag)
        .lines()
        .any(|line| line.trim_end() == TAG_LINE)
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
    fs: &dyn crate::platform::Fs,
    dir: &Path,
    cap: usize,
) -> Result<Vec<PathBuf>, String> {
    let entries = fs
        .read_dir_bounded(dir, cap)
        .map_err(|err| format!("{}: {err}", dir.display()))?;
    let mut dirs = Vec::new();
    for (name, kind) in entries {
        if kind == crate::platform::EntryKind::Dir {
            dirs.push(dir.join(name));
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn is_dir_bounded(fs: &dyn crate::platform::Fs, path: &Path) -> Result<bool, String> {
    match fs.entry_stat(path) {
        Ok(stat) => Ok(stat.kind == crate::platform::EntryKind::Dir),
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
        let fs = crate::platform::current().unwrap().fs;
        let capped = identify_bounded(fs.as_ref(), &root, n).unwrap().unwrap();
        assert_eq!(open, capped);
        let err = identify_bounded(fs.as_ref(), &root, n - 1)
            .unwrap()
            .unwrap_err();
        assert!(err.contains("more than"), "{err}");
        assert!(identify(&root).is_some_and(|found| found.is_ok()));
        let _ = fs::remove_dir_all(root);
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
