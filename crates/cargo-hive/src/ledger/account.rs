//! Measures one path into a ledger and then removes it. A directory is walked without
//! following links; a file or a symlink is one entry, from a single stat. A dry run measures
//! and leaves the path in place. A path that is already gone counts nothing.

use super::{Ledger, record, scan};
use crate::platform::{EntryKind, EntryStat, Fs};
use std::fmt;
use std::io;
use std::path::Path;

#[doc(hidden)]
#[derive(Debug)]
pub struct Removed {
    pub files: u64,
    pub existed: bool,
}

#[derive(Debug)]
pub(crate) enum RemoveFailed {
    Scan(String),
    Remove(io::Error),
}

impl fmt::Display for RemoveFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RemoveFailed::Scan(why) => write!(f, "{why}"),
            RemoveFailed::Remove(error) => write!(f, "could not remove it: {error}"),
        }
    }
}

impl std::error::Error for RemoveFailed {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RemoveFailed::Scan(_) => None,
            RemoveFailed::Remove(error) => Some(error),
        }
    }
}

pub(crate) fn remove_counted(
    fs: &dyn Fs,
    ledger: &mut Ledger,
    target: &Path,
    path: &Path,
    account_bytes: bool,
    dry_run: bool,
) -> Result<Removed, RemoveFailed> {
    let stat = match fs.entry_stat(path) {
        Ok(stat) => stat,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Removed {
                files: 0,
                existed: false,
            });
        }
        Err(e) => {
            return Err(RemoveFailed::Scan(format!(
                "could not stat {}: {e}",
                path.display()
            )));
        }
    };
    match stat.kind {
        EntryKind::File | EntryKind::Symlink => {
            remove_leaf(fs, ledger, target, path, stat, account_bytes, dry_run)
        }
        EntryKind::Dir | EntryKind::Other => {
            remove_dir(fs, ledger, target, path, account_bytes, dry_run)
        }
    }
}

fn remove_leaf(
    fs: &dyn Fs,
    ledger: &mut Ledger,
    target: &Path,
    path: &Path,
    stat: EntryStat,
    account_bytes: bool,
    dry_run: bool,
) -> Result<Removed, RemoveFailed> {
    let measured = if account_bytes {
        fs.sharing(path).map(|sharing| (stat, Some(sharing)))
    } else {
        Ok((stat, None))
    };
    if !dry_run {
        match fs.remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Removed {
                    files: 0,
                    existed: false,
                });
            }
            Err(e) => {
                ledger.unmeasured(path.to_path_buf(), format!("removal failed part-way: {e}"));
                return Err(RemoveFailed::Remove(e));
            }
        }
    }
    match measured {
        Ok((stat, sharing)) => ledger.file(target, &stat, sharing.as_ref()),
        Err(e) => ledger.unmeasured(path.to_path_buf(), e.to_string()),
    }
    Ok(Removed {
        files: 1,
        existed: true,
    })
}

fn remove_dir(
    fs: &dyn Fs,
    ledger: &mut Ledger,
    target: &Path,
    path: &Path,
    account_bytes: bool,
    dry_run: bool,
) -> Result<Removed, RemoveFailed> {
    let files = match scan(fs, path, account_bytes) {
        Ok(None) => {
            return Ok(Removed {
                files: 0,
                existed: false,
            });
        }
        Ok(Some(files)) => files,
        Err(why) => return Err(RemoveFailed::Scan(why)),
    };
    let count = files.len() as u64;
    if !dry_run {
        match fs.remove_tree(path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Removed {
                    files: 0,
                    existed: false,
                });
            }
            Err(e) => {
                ledger.unmeasured(path.to_path_buf(), format!("removal failed part-way: {e}"));
                return Err(RemoveFailed::Remove(e));
            }
        }
    }
    record(ledger, target, files);
    Ok(Removed {
        files: count,
        existed: true,
    })
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::platform::{self, Fs};
    use std::path::PathBuf;

    fn scratch(label: &str) -> PathBuf {
        let d = PathBuf::from(format!(
            "/private/tmp/hive-it-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_file_root_is_measured_and_removed() {
        let platform = platform::current().unwrap();
        let fs: &dyn Fs = platform.fs.as_ref();
        let dir = scratch("account-file");
        let file = dir.join("libdep-0123456789abcdef.rlib");
        std::fs::write(&file, vec![7u8; 8192]).unwrap();
        let mut ledger = Ledger::new();
        let removed = remove_counted(fs, &mut ledger, &dir, &file, true, false).unwrap();
        assert_eq!(removed.files, 1);
        assert!(!file.exists());
        let tally = ledger.finish();
        assert!(tally.total.freed >= 8192, "freed {}", tally.total.freed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_root_is_walked_without_following_symlinks_and_removed() {
        let platform = platform::current().unwrap();
        let fs: &dyn Fs = platform.fs.as_ref();
        let dir = scratch("account-dir");
        let unit = dir.join("unit");
        std::fs::create_dir_all(unit.join("sub")).unwrap();
        std::fs::write(unit.join("sub/a"), b"aaaa").unwrap();
        std::os::unix::fs::symlink(&dir, unit.join("link-out")).unwrap();
        let mut ledger = Ledger::new();
        let removed = remove_counted(fs, &mut ledger, &dir, &unit, true, false).unwrap();
        assert_eq!(removed.files, 1);
        assert!(!unit.exists());
        assert!(dir.exists(), "the symlink target must survive");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dry_run_measures_and_removes_nothing() {
        let platform = platform::current().unwrap();
        let fs: &dyn Fs = platform.fs.as_ref();
        let dir = scratch("account-dry");
        let file = dir.join("a.rlib");
        std::fs::write(&file, vec![1u8; 4096]).unwrap();
        let mut ledger = Ledger::new();
        let removed = remove_counted(fs, &mut ledger, &dir, &file, true, true).unwrap();
        assert_eq!(removed.files, 1);
        assert!(file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_that_is_already_gone_counts_nothing() {
        let platform = platform::current().unwrap();
        let fs: &dyn Fs = platform.fs.as_ref();
        let dir = scratch("account-gone");
        let mut ledger = Ledger::new();
        let removed =
            remove_counted(fs, &mut ledger, &dir, &dir.join("missing"), true, false).unwrap();
        assert_eq!(removed.files, 0);
        assert!(!removed.existed);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
