//! rustc's incremental-session rules for one crate directory, as a pure function of the
//! directory's listing and the current time. They mirror rustc 1.97.1's own session GC
//! (`rustc_incremental/src/persist/fs.rs`). This module decides; it never touches a file.

use crate::platform::EntryKind;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A `-working` session that still has its lock file, or an orphan lock file, must be at
/// least this old by its name. A session directory with no lock file has no age gate.
pub const MIN_AGE: Duration = Duration::from_secs(10);

/// A session directory and the lock file of that same session key.
///
/// Built only here. `lock` is `<key>.lock` and `dir` is `<key>-<suffix>`, so a directory
/// and a lock from different sessions cannot be paired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionPair {
    dir: String,
    lock: String,
}

impl SessionPair {
    pub(crate) fn dir(&self) -> &str {
        &self.dir
    }

    pub(crate) fn lock(&self) -> &str {
        &self.lock
    }

    /// `None` when `lock` is not the lock file for `dir`'s session key.
    pub(crate) fn from_names(dir: &str, lock: &str) -> Option<SessionPair> {
        let key = lock.strip_suffix(".lock")?;
        let rest = dir.strip_prefix(key)?;
        let suffix = rest.strip_prefix('-')?;
        (!suffix.is_empty()).then(|| SessionPair {
            dir: dir.to_string(),
            lock: lock.to_string(),
        })
    }
}

/// A session lock file name that `parse_lock` accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrphanLock {
    name: String,
}

impl OrphanLock {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn parse(name: &str) -> Option<OrphanLock> {
        parse_lock(name).map(|_| OrphanLock {
            name: name.to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum Action {
    /// A superseded finalized session, or a `-working` one old enough; delete it under its lock.
    DeleteLocked(SessionPair),
    /// A lock file with no session; delete it under itself.
    DeleteOrphanLock(OrphanLock),
    /// A session directory with no lock file; the pair's lock is re-checked before deleting.
    DeleteLockless(SessionPair),
}

fn base36_lower(field: &str) -> bool {
    !field.is_empty()
        && field
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
}

/// The name timestamp, in microseconds since the epoch. Anything but lowercase base 36
/// that fits a u64 is malformed.
fn timestamp(field: &str) -> Option<u64> {
    base36_lower(field)
        .then(|| u64::from_str_radix(field, 36).ok())
        .flatten()
}

/// `s-<ts>-<rand>-<working|svh>` → (key `s-<ts>-<rand>`, timestamp, finalized).
fn parse_session(name: &str) -> Option<(String, u64, bool)> {
    let parts: Vec<&str> = name.split('-').collect();
    let [s, ts, rand, last] = parts.as_slice() else {
        return None;
    };
    if *s != "s" || !base36_lower(rand) || !base36_lower(last) {
        return None;
    }
    Some((format!("s-{ts}-{rand}"), timestamp(ts)?, *last != "working"))
}

/// `s-<ts>-<rand>.lock` → (key, timestamp).
fn parse_lock(name: &str) -> Option<(String, u64)> {
    let stem = name.strip_suffix(".lock")?;
    let parts: Vec<&str> = stem.split('-').collect();
    let [s, ts, rand] = parts.as_slice() else {
        return None;
    };
    if *s != "s" || !base36_lower(rand) {
        return None;
    }
    Some((stem.to_string(), timestamp(ts)?))
}

pub(crate) fn crate_dir(entries: &[(String, EntryKind)], now: SystemTime) -> Vec<Action> {
    let now_us = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX));
    let min_age_us = u64::try_from(MIN_AGE.as_micros()).expect("ten seconds fit in u64");
    let old_enough = |ts: u64| ts.checked_add(min_age_us).is_some_and(|t| t <= now_us);

    let mut sessions: BTreeMap<String, (String, u64, bool)> = BTreeMap::new();
    let mut ambiguous: BTreeSet<String> = BTreeSet::new();
    let mut locks: BTreeMap<String, (String, u64)> = BTreeMap::new();
    for (name, kind) in entries {
        match kind {
            EntryKind::Dir => {
                if let Some((key, ts, finalized)) = parse_session(name) {
                    if sessions
                        .insert(key.clone(), (name.clone(), ts, finalized))
                        .is_some()
                    {
                        ambiguous.insert(key);
                    }
                }
            }
            EntryKind::File => {
                if let Some((key, ts)) = parse_lock(name) {
                    locks.insert(key, (name.clone(), ts));
                }
            }
            EntryKind::Symlink | EntryKind::Other => {}
        }
    }
    let newest = sessions
        .iter()
        .filter(|(key, (_, _, finalized))| {
            *finalized && locks.contains_key(*key) && !ambiguous.contains(*key)
        })
        .map(|(_, (_, ts, _))| *ts)
        .max();

    let mut actions = Vec::new();
    for (key, (dir, ts, finalized)) in &sessions {
        if ambiguous.contains(key) {
            continue;
        }
        match locks.get(key) {
            Some((lock, _)) => {
                let superseded = *finalized && newest.is_some_and(|n| *ts < n);
                let abandoned = !*finalized && old_enough(*ts);
                if superseded || abandoned {
                    actions.push(Action::DeleteLocked(
                        SessionPair::from_names(dir, lock)
                            .expect("a session dir and its lock share a key"),
                    ));
                }
            }
            None => actions.push(Action::DeleteLockless(
                SessionPair::from_names(dir, &format!("{key}.lock"))
                    .expect("a lockless session's re-check names its own lock"),
            )),
        }
    }
    for (key, (lock, ts)) in &locks {
        if !sessions.contains_key(key) && old_enough(*ts) {
            actions.push(Action::DeleteOrphanLock(
                OrphanLock::parse(lock).expect("the lock name was parsed"),
            ));
        }
    }
    actions
}

/// A rustc `tempfile` staging directory in `deps/`: `rustc` or `rmeta` followed only by ASCII
/// letters and digits. Real artifacts always contain `-`, `_` or `.` in their names.
pub fn is_temp_entry(name: &str, kind: EntryKind) -> bool {
    kind == EntryKind::Dir
        && ["rustc", "rmeta"].iter().any(|prefix| {
            name.strip_prefix(prefix).is_some_and(|rest| {
                !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric())
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Base-36 lowercase, as rustc writes name timestamps.
    fn b36(mut n: u64) -> String {
        let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut out = Vec::new();
        loop {
            out.push(digits[(n % 36) as usize]);
            n /= 36;
            if n == 0 {
                break;
            }
        }
        out.reverse();
        String::from_utf8(out).unwrap()
    }

    const NOW_S: u64 = 1_000_000_000;

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(NOW_S)
    }

    /// Microseconds `secs_ago` before `now()`, minus `minus_us` more.
    fn ts(secs_ago: u64, minus_us: u64) -> u64 {
        (NOW_S - secs_ago) * 1_000_000 - minus_us
    }

    fn session(t: u64, rand: &str, last: &str) -> (String, EntryKind) {
        (format!("s-{}-{rand}-{last}", b36(t)), EntryKind::Dir)
    }

    fn lock(t: u64, rand: &str) -> (String, EntryKind) {
        (format!("s-{}-{rand}.lock", b36(t)), EntryKind::File)
    }

    fn deleted(actions: &[Action]) -> Vec<String> {
        let mut names: Vec<String> = actions
            .iter()
            .map(|a| match a {
                Action::DeleteLocked(pair) | Action::DeleteLockless(pair) => pair.dir().to_string(),
                Action::DeleteOrphanLock(lock) => lock.name().to_string(),
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_planner_follows_rustcs_session_rules() {
        let old = ts(3600, 0);
        let new = ts(60, 0);
        type Case = (&'static str, Vec<(String, EntryKind)>, Vec<String>);
        let cases: Vec<Case> = vec![
            (
                "the older finalized session goes, the newest stays",
                vec![
                    session(old, "aa", "svh1"),
                    lock(old, "aa"),
                    session(new, "bb", "svh2"),
                    lock(new, "bb"),
                ],
                vec![session(old, "aa", "svh1").0],
            ),
            (
                "equal timestamps keep both",
                vec![
                    session(new, "aa", "svh1"),
                    lock(new, "aa"),
                    session(new, "bb", "svh2"),
                    lock(new, "bb"),
                ],
                vec![],
            ),
            (
                "a -working session exactly 10 s old goes",
                vec![session(ts(10, 0), "cc", "working"), lock(ts(10, 0), "cc")],
                vec![session(ts(10, 0), "cc", "working").0],
            ),
            (
                "a -working session one microsecond younger than 10 s stays",
                vec![
                    session(ts(10, 0) + 1, "cc", "working"),
                    lock(ts(10, 0) + 1, "cc"),
                ],
                vec![],
            ),
            (
                "a -working session from the future stays",
                vec![
                    session(ts(0, 0) + 5_000_000, "dd", "working"),
                    lock(ts(0, 0) + 5_000_000, "dd"),
                ],
                vec![],
            ),
            (
                "an orphan lock 10 s old goes, a younger one stays",
                vec![lock(ts(10, 0), "ee"), lock(ts(9, 0), "ff")],
                vec![lock(ts(10, 0), "ee").0],
            ),
            (
                "a lockless session goes even when it is the newest",
                vec![
                    session(old, "aa", "svh1"),
                    lock(old, "aa"),
                    session(new, "bb", "svh2"),
                ],
                vec![session(new, "bb", "svh2").0],
            ),
            (
                "malformed and foreign names are never touched",
                vec![
                    ("s-ABC-aa-svh".into(), EntryKind::Dir),
                    ("s-+1-aa-svh".into(), EntryKind::Dir),
                    ("s-zzzzzzzzzzzzzzzzzz-aa-svh".into(), EntryKind::Dir),
                    ("s-1-2".into(), EntryKind::Dir),
                    ("query-cache.bin".into(), EntryKind::File),
                    ("foo.lock".into(), EntryKind::File),
                    (session(old, "aa", "svh1").0, EntryKind::File),
                    (session(old, "gg", "svh1").0, EntryKind::Symlink),
                ],
                vec![],
            ),
        ];
        for (name, entries, want) in cases {
            let mut want = want;
            want.sort();
            assert_eq!(deleted(&crate_dir(&entries, now())), want, "{name}");
        }
    }

    #[test]
    fn locked_and_lockless_actions_name_the_lock_to_use_or_recheck() {
        let old = ts(3600, 0);
        let new = ts(60, 0);
        let entries = vec![
            session(old, "aa", "svh1"),
            lock(old, "aa"),
            session(new, "bb", "svh2"),
            lock(new, "bb"),
            session(old, "cc", "svh3"),
        ];
        let actions = crate_dir(&entries, now());
        assert!(actions.contains(&Action::DeleteLocked(
            SessionPair::from_names(&session(old, "aa", "svh1").0, &lock(old, "aa").0).unwrap()
        )));
        assert!(actions.contains(&Action::DeleteLockless(
            SessionPair::from_names(&session(old, "cc", "svh3").0, &lock(old, "cc").0).unwrap()
        )));
    }

    #[test]
    fn a_session_pair_rejects_a_lock_from_another_session() {
        let dir = "s-10-aa-svh1";
        assert!(SessionPair::from_names(dir, "s-10-aa.lock").is_some());
        assert!(SessionPair::from_names(dir, "s-10-bb.lock").is_none());
        assert!(SessionPair::from_names(dir, "s-10-aa").is_none());
        assert!(OrphanLock::parse("s-10-aa.lock").is_some());
        assert!(OrphanLock::parse("not-a-lock").is_none());
    }

    #[test]
    fn the_minimum_age_is_ten_seconds() {
        assert_eq!(MIN_AGE, Duration::from_secs(10));
    }

    #[test]
    fn temp_entries_are_rustc_tempfile_dirs_and_nothing_else() {
        let cases = [
            ("rmeta9Ab3x", EntryKind::Dir, true),
            ("rustcAbC123", EntryKind::Dir, true),
            ("rmeta", EntryKind::Dir, false),
            ("rustc_hash-0123456789abcdef.d", EntryKind::File, false),
            ("rustc_hash-0123456789abcdef", EntryKind::Dir, false),
            ("rustcfoo-1.dSYM", EntryKind::Dir, false),
            ("rmetaABC", EntryKind::Symlink, false),
            ("rmeta1", EntryKind::File, false),
        ];
        for (name, kind, want) in cases {
            assert_eq!(is_temp_entry(name, kind), want, "{name} {kind:?}");
        }
    }
}
