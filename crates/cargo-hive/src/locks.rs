//! Locks one Cargo target the way a build locks it, and skips the target when a lock is not sound.
//! Present `.cargo-build-lock` and `.cargo-lock` files are taken, and `.cargo-artifact-lock` when
//! examples are listed. Dropping a successful result releases every lock.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::cargo_target;
use cargo_hive_platform::{Fs, HeldLock, TryLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Held,
    Unopenable,
    Unlockable,
    Missing,
    ProfileSet,
}

impl Reason {
    pub fn count_name(self) -> &'static str {
        match self {
            Reason::Held => "targets_skipped_held",
            Reason::Unopenable => "targets_skipped_lock_unopenable",
            Reason::Unlockable => "targets_skipped_lock_unsupported",
            Reason::Missing => "targets_skipped_lock_missing",
            Reason::ProfileSet => "targets_skipped_profile_set",
        }
    }
}

pub struct Locked {
    pub profiles: Vec<PathBuf>,
    pub held: Vec<HeldLock>,
}

pub enum Acquired {
    Locked(Locked),
    Skipped(Reason),
    Failed(String),
}

pub fn acquire(fs: &dyn Fs, target: &Path, lists_examples: bool) -> Acquired {
    acquire_capped(fs, target, lists_examples, crate::caps::CRATE_DIR_CAP)
}

pub fn acquire_capped(fs: &dyn Fs, target: &Path, lists_examples: bool, cap: usize) -> Acquired {
    let found = match cargo_target::identify_bounded(fs, target, cap) {
        Some(Ok(found)) => found,
        Some(Err(why)) => return Acquired::Failed(why),
        None => return Acquired::Skipped(Reason::Missing),
    };
    let profiles: Vec<PathBuf> = found.into_iter().map(|(path, _)| path).collect();
    let mut held = Vec::new();
    for dir in &profiles {
        let locks = present_locks(fs, dir, lists_examples);
        if locks.is_empty() && fs.entry_stat(&dir.join(".fingerprint")).is_ok() {
            return Acquired::Skipped(Reason::Missing);
        }
        for path in locks {
            // Bind the attempt so the HeldLock temporary keeps the 2021 drop point.
            let attempt = fs.try_lock_exclusive(&path);
            match attempt {
                TryLock::Held => return Acquired::Skipped(Reason::Held),
                TryLock::Unopenable(_) => return Acquired::Skipped(Reason::Unopenable),
                TryLock::Unlockable(_) => return Acquired::Skipped(Reason::Unlockable),
                TryLock::Gone => return Acquired::Skipped(Reason::Missing),
                TryLock::Acquired(lock) => {
                    let same = match (lock.identity(), fs.entry_stat(&path)) {
                        (Some((dev, ino)), Ok(stat)) => stat.dev == dev && stat.ino == ino,
                        _ => false,
                    };
                    if !same {
                        return Acquired::Skipped(Reason::Missing);
                    }
                    held.push(lock);
                }
            }
        }
    }
    match same_directories(fs, target, &profiles, cap) {
        Ok(true) => {}
        Ok(false) => return Acquired::Skipped(Reason::ProfileSet),
        Err(why) => return Acquired::Failed(why),
    }
    Acquired::Locked(Locked { profiles, held })
}

fn present_locks(fs: &dyn Fs, dir: &Path, lists_examples: bool) -> Vec<PathBuf> {
    let mut names = vec![".cargo-build-lock", ".cargo-lock"];
    if lists_examples {
        names.push(".cargo-artifact-lock");
    }
    let mut found = Vec::new();
    for name in names {
        let path = dir.join(name);
        if fs.entry_stat(&path).is_ok() {
            found.push(path);
        }
    }
    found
}

fn same_directories(
    fs: &dyn Fs,
    target: &Path,
    profiles: &[PathBuf],
    cap: usize,
) -> Result<bool, String> {
    let found = match cargo_target::identify_bounded(fs, target, cap) {
        Some(Ok(found)) => found,
        Some(Err(why)) => return Err(why),
        None => return Ok(false),
    };
    let again: BTreeSet<_> = found.into_iter().map(|(path, _)| path).collect();
    let first: BTreeSet<_> = profiles.iter().cloned().collect();
    Ok(again == first)
}
