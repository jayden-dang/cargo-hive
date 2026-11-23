//! Locks one Cargo target the way a build locks it, and skips the target when a lock is not sound.
//! Present `.cargo-build-lock` and `.cargo-lock` files are taken, and `.cargo-artifact-lock` when
//! examples are listed. Dropping a successful result releases every lock.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::target;
use cargo_hive_platform::{EntryKind, Fs, HeldLock, TryLock};
use std::io;

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
    let found = match target::identify_bounded(fs, target, cap) {
        Some(Ok(found)) => found,
        Some(Err(why)) => return Acquired::Failed(why),
        None => return Acquired::Skipped(Reason::Missing),
    };
    let profiles: Vec<(PathBuf, target::Layout)> = found;
    let mut held = Vec::new();
    for (dir, layout) in &profiles {
        let locks = present_locks(fs, dir, lists_examples);
        let expects_lock = match layout {
            target::Layout::Fingerprint => fs.entry_stat(&dir.join(".fingerprint")).is_ok(),
            target::Layout::NewBuildDir => true,
        };
        if locks.is_empty() && expects_lock {
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
    let profile_paths: Vec<PathBuf> = profiles.iter().map(|(path, _)| path.clone()).collect();
    match same_directories(fs, target, &profile_paths, cap) {
        Ok(true) => {}
        Ok(false) => return Acquired::Skipped(Reason::ProfileSet),
        Err(why) => return Acquired::Failed(why),
    }
    Acquired::Locked(Locked {
        profiles: profile_paths,
        held,
    })
}

const CARGO_LOCKS: [&str; 3] = [".cargo-build-lock", ".cargo-lock", ".cargo-artifact-lock"];

pub struct VerifiedLocks {
    profiles: Vec<VerifiedProfile>,
}

struct VerifiedProfile {
    path: PathBuf,
    layout: target::Layout,
    identity: (u64, u64),
    locks: [Option<HeldLock>; CARGO_LOCKS.len()],
}

pub enum VerifyAcquireError {
    Skipped(Reason),
    Failed(String),
}

fn named_identity(fs: &dyn Fs, path: &Path, kind: EntryKind) -> Result<Option<(u64, u64)>, String> {
    match fs.entry_stat(path) {
        Ok(stat) if stat.kind == kind => Ok(Some((stat.dev, stat.ino))),
        Ok(_) => Err(format!("{} has an unexpected entry type", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

impl VerifiedLocks {
    /// Verifies that the current profile and lock paths still name the acquired inodes.
    pub fn verify_current(&self, fs: &dyn Fs) -> Result<(), String> {
        for profile in &self.profiles {
            if named_identity(fs, &profile.path, EntryKind::Dir)? != Some(profile.identity) {
                return Err(format!(
                    "Cargo profile identity changed: {}",
                    profile.path.display()
                ));
            }
            for (name, held) in CARGO_LOCKS.iter().zip(&profile.locks) {
                let path = profile.path.join(name);
                if named_identity(fs, &path, EntryKind::File)?
                    != held.as_ref().and_then(HeldLock::identity)
                {
                    return Err(format!("Cargo lock identity changed: {}", path.display()));
                }
            }
        }
        Ok(())
    }

    /// Binds a newly discovered profile set and layouts to the locked namespace.
    pub fn verify_profiles(
        &self,
        fs: &dyn Fs,
        found: &[(PathBuf, target::Layout)],
    ) -> Result<(), String> {
        if found.len() != self.profiles.len()
            || !found
                .iter()
                .zip(&self.profiles)
                .all(|((path, layout), profile)| path == &profile.path && layout == &profile.layout)
        {
            return Err("the Cargo profile set changed while locked".into());
        }
        self.verify_current(fs)
    }
}

/// Acquires every present Cargo lock and records the current profile and lock identities.
pub fn acquire_verified(
    fs: &dyn Fs,
    target: &Path,
    cancel: &dyn Fn() -> bool,
) -> Result<VerifiedLocks, VerifyAcquireError> {
    use VerifyAcquireError::{Failed, Skipped};

    let profiles =
        target::identify_strict(fs, target, crate::caps::CRATE_DIR_CAP, cancel).map_err(Failed)?;
    let mut locked = VerifiedLocks {
        profiles: Vec::with_capacity(profiles.len()),
    };
    for (path, layout) in profiles {
        let identity = named_identity(fs, &path, EntryKind::Dir)
            .map_err(Failed)?
            .ok_or(Skipped(Reason::ProfileSet))?;
        let mut locks = [None, None, None];
        for (name, slot) in CARGO_LOCKS.iter().zip(&mut locks) {
            if cancel() {
                return Err(Failed("Cargo lock acquisition was cancelled".into()));
            }
            let lock_path = path.join(name);
            let Some(expected) = named_identity(fs, &lock_path, EntryKind::File).map_err(Failed)?
            else {
                continue;
            };
            let held = match fs.try_lock_exclusive(&lock_path) {
                TryLock::Held => return Err(Skipped(Reason::Held)),
                TryLock::Unopenable(_) => return Err(Skipped(Reason::Unopenable)),
                TryLock::Unlockable(_) => return Err(Skipped(Reason::Unlockable)),
                TryLock::Gone => return Err(Skipped(Reason::Missing)),
                TryLock::Acquired(held) => held,
            };
            if held.identity() != Some(expected) {
                return Err(Skipped(Reason::Missing));
            }
            *slot = Some(held);
        }
        if locks.iter().all(Option::is_none) {
            return Err(Skipped(Reason::Missing));
        }
        locked.profiles.push(VerifiedProfile {
            path,
            layout,
            identity,
            locks,
        });
    }
    locked.verify_current(fs).map_err(Failed)?;
    Ok(locked)
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
    let found = match target::identify_bounded(fs, target, cap) {
        Some(Ok(found)) => found,
        Some(Err(why)) => return Err(why),
        None => return Ok(false),
    };
    let again: BTreeSet<_> = found.into_iter().map(|(path, _)| path).collect();
    let first: BTreeSet<_> = profiles.iter().cloned().collect();
    Ok(again == first)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::target::TAG_LINE;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(label: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let k = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p =
            std::env::temp_dir().join(format!("hive-lock-{label}-{}-{k}-{n}", std::process::id()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn a_unit_dir_profile_with_no_lock_files_is_missing_a_lock() {
        let root = scratch("unit-dir-missing");
        fs::write(
            root.join("CACHEDIR.TAG"),
            format!("Signature: 8a477f597d28d172789f06886806bc55\n{TAG_LINE}\n"),
        )
        .unwrap();
        fs::create_dir_all(root.join("debug/build/pkg/0123456789abcdef/fingerprint")).unwrap();
        let fs = cargo_hive_platform::current().unwrap().fs;
        match acquire(fs.as_ref(), &root, false) {
            Acquired::Skipped(Reason::Missing) => {}
            Acquired::Skipped(reason) => panic!("expected Missing, got {reason:?}"),
            Acquired::Locked(_) => panic!("expected Skipped(Missing), got Locked"),
            Acquired::Failed(why) => panic!("expected Skipped(Missing), got Failed({why})"),
        }
        let _ = fs::remove_dir_all(root);
    }
}
