//! Locks one Cargo target the way a build locks it, and skips the target when a lock is not sound.
//! Present `.cargo-build-lock` and `.cargo-lock` files are taken, and `.cargo-artifact-lock` when
//! examples are listed. Dropping a successful result releases every lock.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::cargo_target;
use crate::platform::{Fs, HeldLock, TryLock};

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
}

pub fn acquire(fs: &dyn Fs, target: &Path, lists_examples: bool) -> Acquired {
    let Some(Ok(found)) = cargo_target::identify(target) else {
        return Acquired::Skipped(Reason::Missing);
    };
    let profiles: Vec<PathBuf> = found.into_iter().map(|(path, _)| path).collect();
    let mut held = Vec::new();
    for dir in &profiles {
        let locks = present_locks(fs, dir, lists_examples);
        if locks.is_empty() && fs.entry_stat(&dir.join(".fingerprint")).is_ok() {
            return Acquired::Skipped(Reason::Missing);
        }
        for path in locks {
            match fs.try_lock_exclusive(&path) {
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
    if !same_directories(target, &profiles) {
        return Acquired::Skipped(Reason::ProfileSet);
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

fn same_directories(target: &Path, profiles: &[PathBuf]) -> bool {
    let Some(Ok(found)) = cargo_target::identify(target) else {
        return false;
    };
    let again: BTreeSet<_> = found.into_iter().map(|(path, _)| path).collect();
    let first: BTreeSet<_> = profiles.iter().cloned().collect();
    again == first
}
