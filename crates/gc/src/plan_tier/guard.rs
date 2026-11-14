//! Accepts one path for removal inside a root, then deletes it by kind.
//! `check_entry` never removes anything. A `.` or `..` component is outside the root.
//! A directory is removed as a tree; a file or symbolic link is unlinked. The link
//! itself is removed, not the path it names.

use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use cargo_hive_platform::{EntryKind, EntryStat, Fs};

pub enum Expect {
    Entry { kind: EntryKind, dev: u64, ino: u64 },
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deleted {
    Removed,
    Missing,
    Changed,
    Outside,
}

pub fn check_entry(
    fs: &dyn Fs,
    root: &Path,
    path: &Path,
    expect: Expect,
) -> Result<EntryStat, Deleted> {
    if steps_through_dot(path) {
        return Err(Deleted::Outside);
    }
    let parent = canonical_parent(path)?;
    let root = root.canonicalize().map_err(|_| Deleted::Outside)?;
    if !parent.starts_with(root) {
        return Err(Deleted::Outside);
    }
    let stat = match fs.entry_stat(path) {
        Ok(stat) => stat,
        Err(e) if e.kind() == ErrorKind::NotFound => return Err(Deleted::Missing),
        Err(_) => return Err(Deleted::Changed),
    };
    if let Expect::Entry { kind, dev, ino } = expect
        && (stat.kind != kind || stat.dev != dev || stat.ino != ino)
    {
        return Err(Deleted::Changed);
    }
    if stat.kind == EntryKind::Other {
        return Err(Deleted::Changed);
    }
    Ok(stat)
}

pub fn delete_checked(fs: &dyn Fs, root: &Path, path: &Path, expect: Expect) -> Deleted {
    let stat = match check_entry(fs, root, path, expect) {
        Ok(stat) => stat,
        Err(reason) => return reason,
    };
    let removed = match stat.kind {
        EntryKind::Dir => fs.remove_tree(path),
        EntryKind::File | EntryKind::Symlink => fs.remove_file(path),
        EntryKind::Other => return Deleted::Changed,
    };
    match removed {
        Ok(()) => Deleted::Removed,
        Err(e) if e.kind() == ErrorKind::NotFound => Deleted::Missing,
        Err(_) => Deleted::Changed,
    }
}

fn steps_through_dot(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
        || path
            .as_os_str()
            .as_encoded_bytes()
            .split(|b| *b == b'/')
            .any(|part| part == b".")
}

fn canonical_parent(path: &Path) -> Result<PathBuf, Deleted> {
    let Some(parent) = path.parent() else {
        return Err(Deleted::Outside);
    };
    match std::fs::canonicalize(parent) {
        Ok(parent) => Ok(parent),
        Err(e) if e.kind() == ErrorKind::NotFound => Err(Deleted::Missing),
        Err(_) => Err(Deleted::Outside),
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use cargo_hive_platform::{self as platform, Fs};
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    fn root(label: &str) -> PathBuf {
        let d = PathBuf::from(format!(
            "/private/tmp/hive-it-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("target/deps")).unwrap();
        d
    }

    #[test]
    fn removes_a_file_a_directory_and_a_symlink_by_kind_and_reports_missing() {
        let p = platform::current().unwrap();
        let fs: &dyn Fs = p.fs.as_ref();
        let d = root("guard-kinds");
        let t = d.join("target");
        std::fs::write(t.join("deps/a.rlib"), b"x").unwrap();
        std::fs::create_dir_all(t.join("deps/unit")).unwrap();
        std::fs::write(t.join("deps/unit/f"), b"x").unwrap();
        symlink("/etc", t.join("deps/link")).unwrap();
        for name in ["a.rlib", "unit", "link"] {
            assert_eq!(
                delete_checked(fs, &t, &t.join("deps").join(name), Expect::Any),
                Deleted::Removed,
                "{name}"
            );
        }
        assert!(
            std::path::Path::new("/etc").exists(),
            "the link target must survive"
        );
        assert_eq!(
            delete_checked(fs, &t, &t.join("deps/a.rlib"), Expect::Any),
            Deleted::Missing
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_a_path_outside_the_target_or_reached_through_a_symlinked_parent() {
        let p = platform::current().unwrap();
        let fs: &dyn Fs = p.fs.as_ref();
        let d = root("guard-outside");
        let t = d.join("target");
        let outside = d.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep"), b"x").unwrap();
        assert_eq!(
            delete_checked(fs, &t, &outside.join("keep"), Expect::Any),
            Deleted::Outside
        );
        symlink(&outside, t.join("deps/escape")).unwrap();
        assert_eq!(
            delete_checked(fs, &t, &t.join("deps/escape/keep"), Expect::Any),
            Deleted::Outside
        );
        assert!(outside.join("keep").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_an_entry_whose_identity_differs_and_check_entry_never_mutates() {
        let p = platform::current().unwrap();
        let fs: &dyn Fs = p.fs.as_ref();
        let d = root("guard-changed");
        let t = d.join("target");
        let f = t.join("deps/a.rlib");
        std::fs::write(&f, b"x").unwrap();
        let st = fs.entry_stat(&f).unwrap();
        let wrong = Expect::Entry {
            kind: st.kind,
            dev: st.dev,
            ino: st.ino + 1,
        };
        assert_eq!(delete_checked(fs, &t, &f, wrong), Deleted::Changed);
        assert!(f.exists());
        let right = Expect::Entry {
            kind: st.kind,
            dev: st.dev,
            ino: st.ino,
        };
        assert!(check_entry(fs, &t, &f, right).is_ok());
        assert!(f.exists(), "check_entry must not remove");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_whole_target_dir_is_checked_against_its_parent() {
        let p = platform::current().unwrap();
        let fs: &dyn Fs = p.fs.as_ref();
        let d = root("guard-whole");
        let t = d.join("target");
        assert!(check_entry(fs, &d, &t, Expect::Any).is_ok());
        assert_eq!(
            check_entry(fs, &t, &t, Expect::Any).unwrap_err(),
            Deleted::Outside
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_dot_and_dot_dot_and_leaves_a_sibling_of_the_root() {
        let p = platform::current().unwrap();
        let fs: &dyn Fs = p.fs.as_ref();
        let d = root("guard-dots");
        let t = d.join("target");
        std::fs::create_dir_all(t.join("child")).unwrap();
        let sibling = d.join("sibling");
        std::fs::write(&sibling, b"x").unwrap();
        assert_eq!(
            check_entry(fs, &t, &t.join("child/.."), Expect::Any),
            Err(Deleted::Outside)
        );
        assert_eq!(
            check_entry(fs, &t, &t.join("child/../.."), Expect::Any),
            Err(Deleted::Outside)
        );
        let through_dot = PathBuf::from(format!("{}/child/.", t.display()));
        assert_eq!(
            check_entry(fs, &t, &through_dot, Expect::Any),
            Err(Deleted::Outside)
        );
        assert_eq!(
            delete_checked(fs, &t, &t.join("child/../.."), Expect::Any),
            Deleted::Outside
        );
        assert!(sibling.exists(), "a sibling of the root must survive");
        let _ = std::fs::remove_dir_all(&d);
    }
}
