//! Runs the planner over selected targets. Every effect goes through `Fs` in a fixed order:
//! lock, re-check under the lock, measure, remove the dir, remove the lock file, release. A
//! plan made from a stale listing therefore never deletes what replaced its entry.

use super::is_temp_entry;
use super::ledger::{Bytes, Ledger};
use super::plan::{self, Action};
use crate::cargo_target::{Owner, TargetDir};
use crate::fingerprint::Format;
use crate::platform::{EntryKind, EntryStat, Fs, HeldLock, Sharing, TryLock};
use crate::size::MAX_DEPTH;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Most entries listed from one `incremental/` level, crate dir or scanned directory.
pub const CRATE_DIR_CAP: usize = 100_000;
/// Most entries listed from one `deps/` directory.
pub const DEPS_CAP: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every target the scan found.
    All,
    /// Targets owned by, or lying inside, these checkouts (realpaths).
    Checkouts(Vec<PathBuf>),
}

#[derive(Debug, Clone, Copy)]
pub struct RunOpts {
    pub dry_run: bool,
    pub now: SystemTime,
    /// False when doctor check (c) has not passed; the byte counts are then left out.
    pub account_bytes: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    pub sessions_deleted: u64,
    pub session_lock_files_deleted: u64,
    pub temp_entries_deleted: u64,
    pub skipped_held: u64,
    pub skipped_lock_unopenable: u64,
    /// Both byte totals, or neither. Absent when doctor check (c) has not passed.
    pub bytes: Option<Bytes>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct GcReport {
    pub per_target: Vec<(PathBuf, Counts)>,
    pub totals: Counts,
    pub skipped: Vec<Skip>,
    pub unmeasured: Vec<(PathBuf, String)>,
    pub unmeasured_more: u64,
    pub cancelled: bool,
}

type Scanned = Vec<(PathBuf, io::Result<(EntryStat, Option<Sharing>)>)>;

struct Runner<'a> {
    fs: &'a dyn Fs,
    opts: &'a RunOpts,
    cancel: &'a dyn Fn() -> bool,
    ledger: Ledger,
    skipped: Vec<Skip>,
    cancelled: bool,
}

fn in_scope(t: &TargetDir, scope: &Scope) -> bool {
    match scope {
        Scope::All => true,
        Scope::Checkouts(checkouts) => {
            matches!(&t.owner, Owner::Owned { checkout } if checkouts.contains(checkout))
                || checkouts.iter().any(|c| t.path.starts_with(c))
        }
    }
}

pub fn run(
    fs: &dyn Fs,
    targets: &[TargetDir],
    scope: &Scope,
    opts: &RunOpts,
    cancel: &dyn Fn() -> bool,
) -> GcReport {
    let mut r = Runner {
        fs,
        opts,
        cancel,
        ledger: Ledger::new(),
        skipped: Vec::new(),
        cancelled: false,
    };
    let mut per_target = Vec::new();
    for t in targets.iter().filter(|t| in_scope(t, scope)) {
        if r.stop() {
            break;
        }
        if let Some(why) = &t.unreadable {
            r.skip(&t.path, format!("the target dir is unreadable: {why}"));
            continue;
        }
        match fs.volume(&t.path) {
            Ok(v) if v.fs_type == "apfs" => {}
            Ok(v) => {
                r.skip(
                    &t.path,
                    format!("not on an APFS volume (filesystem type {})", v.fs_type),
                );
                continue;
            }
            Err(e) => {
                r.skip(
                    &t.path,
                    format!("its filesystem type could not be read: {e}"),
                );
                continue;
            }
        }
        let mut counts = Counts::default();
        for p in &t.profiles {
            if r.stop() {
                break;
            }
            if let Format::Unsupported(why) = &p.format {
                r.skip(&p.path, format!("unsupported fingerprint format: {why}"));
                continue;
            }
            r.sessions(&t.path, &p.path, &mut counts);
            r.temp_entries(&t.path, &p.path, &mut counts);
        }
        per_target.push((t.path.clone(), counts));
    }
    let Runner {
        ledger,
        skipped,
        cancelled,
        ..
    } = r;
    let tally = ledger.finish();
    let mut totals = Counts::default();
    for (path, c) in &mut per_target {
        if opts.account_bytes {
            c.bytes = Some(tally.per_target.get(path).copied().unwrap_or_default());
        }
        totals.sessions_deleted += c.sessions_deleted;
        totals.session_lock_files_deleted += c.session_lock_files_deleted;
        totals.temp_entries_deleted += c.temp_entries_deleted;
        totals.skipped_held += c.skipped_held;
        totals.skipped_lock_unopenable += c.skipped_lock_unopenable;
    }
    if opts.account_bytes {
        totals.bytes = Some(tally.total);
    }
    GcReport {
        per_target,
        totals,
        skipped,
        unmeasured: tally.unmeasured,
        unmeasured_more: tally.unmeasured_more,
        cancelled,
    }
}

impl Runner<'_> {
    fn stop(&mut self) -> bool {
        if !self.cancelled && (self.cancel)() {
            self.cancelled = true;
        }
        self.cancelled
    }

    fn skip(&mut self, path: &Path, reason: String) {
        self.skipped.push(Skip {
            path: path.to_path_buf(),
            reason,
        });
    }

    /// Takes a session lock, counting why not when it cannot.
    fn take(&mut self, lock: &Path, c: &mut Counts) -> Option<HeldLock> {
        match self.fs.try_lock_exclusive(lock) {
            TryLock::Acquired(guard) => Some(guard),
            TryLock::Held => {
                c.skipped_held += 1;
                None
            }
            TryLock::Gone => None,
            TryLock::Unopenable(_) => {
                c.skipped_lock_unopenable += 1;
                None
            }
            TryLock::Unlockable(e) => {
                self.skip(lock, format!("the lock could not be taken: {e}"));
                None
            }
        }
    }

    fn sessions(&mut self, target: &Path, profile: &Path, c: &mut Counts) {
        let incremental = profile.join("incremental");
        let crates = match self.fs.read_dir_bounded(&incremental, CRATE_DIR_CAP) {
            Ok(v) => v,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(e) => return self.skip(&incremental, format!("could not list it: {e}")),
        };
        for (name, kind) in crates {
            if kind != EntryKind::Dir {
                continue;
            }
            if self.stop() {
                return;
            }
            let crate_dir = incremental.join(&name);
            let listing = match self.fs.read_dir_bounded(&crate_dir, CRATE_DIR_CAP) {
                Ok(v) => v,
                Err(e) => {
                    self.skip(&crate_dir, format!("could not list it: {e}"));
                    continue;
                }
            };
            for action in plan::crate_dir(&listing, self.opts.now) {
                if self.stop() {
                    return;
                }
                match action {
                    Action::DeleteLocked(pair) => self.delete_locked(
                        target,
                        &crate_dir.join(pair.dir()),
                        &crate_dir.join(pair.lock()),
                        c,
                    ),
                    Action::DeleteOrphanLock(lock) => {
                        self.delete_orphan_lock(&crate_dir, lock.name(), c)
                    }
                    Action::DeleteLockless(pair) => self.delete_lockless(
                        target,
                        &crate_dir.join(pair.dir()),
                        &crate_dir.join(pair.lock()),
                        c,
                    ),
                }
            }
        }
    }

    fn delete_locked(&mut self, target: &Path, dir: &Path, lock: &Path, c: &mut Counts) {
        let Some(guard) = self.take(lock, c) else {
            return;
        };
        // Re-check under the lock. A session that finalized after the listing has a new name,
        // and this lock file now belongs to it, so neither is touched. A stat error other than
        // a missing path is a failure: the entry stays, and the receipt says why.
        match self.fs.entry_stat(dir) {
            Ok(s) if s.kind == EntryKind::Dir => {}
            Ok(_) => {
                drop(guard);
                return;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                drop(guard);
                return;
            }
            Err(e) => {
                self.skip(dir, format!("could not stat it: {e}"));
                drop(guard);
                return;
            }
        }
        if self.remove_measured(target, dir) {
            c.sessions_deleted += 1;
            self.remove_lock_file(lock, c);
        }
        drop(guard);
    }

    fn delete_orphan_lock(&mut self, crate_dir: &Path, lock_name: &str, c: &mut Counts) {
        let lock = crate_dir.join(lock_name);
        let Some(guard) = self.take(&lock, c) else {
            return;
        };
        let prefix = format!("{}-", lock_name.trim_end_matches(".lock"));
        // Re-check under the lock: a session that appeared since the listing owns this file.
        // A list error leaves the lock in place and is reported; it is not "a session appeared".
        let claimed = match self.fs.read_dir_bounded(crate_dir, CRATE_DIR_CAP) {
            Ok(entries) => entries.iter().any(|(n, _)| n.starts_with(&prefix)),
            Err(e) => {
                self.skip(crate_dir, format!("could not list it: {e}"));
                drop(guard);
                return;
            }
        };
        if !claimed {
            self.remove_lock_file(&lock, c);
        }
        drop(guard);
    }

    fn delete_lockless(&mut self, target: &Path, dir: &Path, lock: &Path, c: &mut Counts) {
        // rustc creates a session's lock file before its dir, so a lock file that exists now
        // means the listing missed it: leave the dir for a later run. A stat error other than
        // a missing path is a failure, reported the same way, and the dir stays.
        match self.fs.entry_stat(lock) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return self.skip(lock, format!("could not stat it: {e}")),
            Ok(_) => return,
        }
        if self.remove_measured(target, dir) {
            c.sessions_deleted += 1;
        }
    }

    fn remove_lock_file(&mut self, lock: &Path, c: &mut Counts) {
        if self.opts.dry_run {
            c.session_lock_files_deleted += 1;
            return;
        }
        match self.fs.remove_file(lock) {
            Ok(()) => c.session_lock_files_deleted += 1,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => self.skip(lock, format!("could not remove the lock file: {e}")),
        }
    }

    fn temp_entries(&mut self, target: &Path, profile: &Path, c: &mut Counts) {
        let lock = profile.join(".cargo-build-lock");
        let guard = match self.fs.try_lock_exclusive(&lock) {
            TryLock::Acquired(guard) => guard,
            TryLock::Held => {
                c.skipped_held += 1;
                return;
            }
            TryLock::Gone => {
                return self.skip(
                    profile,
                    "it has no .cargo-build-lock, so its temp entries are left alone".into(),
                )
            }
            TryLock::Unopenable(e) => return self.skip(&lock, format!("could not open it: {e}")),
            TryLock::Unlockable(e) => {
                return self.skip(&lock, format!("the lock could not be taken: {e}"))
            }
        };
        let deps = profile.join("deps");
        match self.fs.read_dir_bounded(&deps, DEPS_CAP) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => self.skip(&deps, format!("could not list it: {e}")),
            Ok(entries) => {
                for (name, kind) in entries {
                    if !is_temp_entry(&name, kind) {
                        continue;
                    }
                    if self.stop() {
                        break;
                    }
                    if self.remove_measured(target, &deps.join(&name)) {
                        c.temp_entries_deleted += 1;
                    }
                }
            }
        }
        drop(guard);
    }

    /// Measures `path` for the ledger, then removes it (a dry run only measures). True when
    /// the entry is, or in a dry run would be, gone. A vanished entry counts nothing.
    fn remove_measured(&mut self, target: &Path, path: &Path) -> bool {
        let files = match self.scan(path) {
            Ok(None) => return false,
            Ok(Some(files)) => files,
            Err(why) => {
                self.skip(path, why);
                return false;
            }
        };
        if !self.opts.dry_run {
            match self.fs.remove_tree(path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => return false,
                Err(e) => {
                    self.skip(path, format!("could not remove it: {e}"));
                    self.ledger
                        .unmeasured(path.to_path_buf(), format!("removal failed part-way: {e}"));
                    return false;
                }
            }
        }
        for (file, measured) in files {
            match measured {
                Ok((stat, sharing)) => self.ledger.file(target, &stat, sharing.as_ref()),
                Err(e) => self.ledger.unmeasured(file, e.to_string()),
            }
        }
        true
    }

    /// The regular files under `root`, without following links and at most `MAX_DEPTH` deep.
    /// `Ok(None)` means the root vanished between listing and measure.
    fn scan(&self, root: &Path) -> Result<Option<Scanned>, String> {
        let mut files = Vec::new();
        let mut stack = vec![(root.to_path_buf(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let entries = match self.fs.read_dir_bounded(&dir, CRATE_DIR_CAP) {
                Ok(v) => v,
                Err(e) if e.kind() == io::ErrorKind::NotFound && dir == root => return Ok(None),
                Err(e) => return Err(format!("could not list {}: {e}", dir.display())),
            };
            for (name, kind) in entries {
                let path = dir.join(&name);
                match kind {
                    EntryKind::Dir if depth + 1 > MAX_DEPTH => {
                        return Err(format!(
                            "{} is deeper than {MAX_DEPTH} levels",
                            path.display()
                        ))
                    }
                    EntryKind::Dir => stack.push((path, depth + 1)),
                    EntryKind::File => {
                        let measured = self.fs.entry_stat(&path).and_then(|stat| {
                            let sharing = if self.opts.account_bytes {
                                Some(self.fs.sharing(&path)?)
                            } else {
                                None
                            };
                            Ok((stat, sharing))
                        });
                        files.push((path, measured));
                    }
                    EntryKind::Symlink | EntryKind::Other => {}
                }
            }
        }
        Ok(Some(files))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::ProfileReport;
    use crate::platform::{
        CompressOutcome, HeldLock, LockHolder, LockKind, LockState, TempDir, VolumeInfo,
    };
    use std::collections::{BTreeSet, HashMap};
    use std::os::unix::fs::MetadataExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, UNIX_EPOCH};

    #[derive(Clone, Copy)]
    enum Scripted {
        Held,
        Unopenable,
        Unlockable,
    }

    type Hook = Box<dyn FnOnce() + Send>;

    /// Real files in a scratch dir. Every effect is recorded, and lock outcomes and hooks are scripted by file name.
    struct Rec {
        events: Arc<Mutex<Vec<String>>>,
        scripted: Mutex<HashMap<String, Scripted>>,
        on_lock: Mutex<HashMap<String, Hook>>,
        on_stat: Mutex<HashMap<String, Hook>>,
        caps: Mutex<BTreeSet<usize>>,
        stat_fails: Mutex<BTreeSet<String>>,
        cap_override: Option<usize>,
        fs_type: String,
        /// Sharing attributes returned for a file name. Absent names are unshared.
        shares: Mutex<HashMap<String, Sharing>>,
    }

    fn name(p: &Path) -> String {
        p.file_name().unwrap().to_string_lossy().into_owned()
    }

    fn kind_of(m: &std::fs::Metadata) -> EntryKind {
        if m.file_type().is_symlink() {
            EntryKind::Symlink
        } else if m.is_dir() {
            EntryKind::Dir
        } else if m.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        }
    }

    impl Rec {
        fn new() -> Rec {
            Rec {
                events: Arc::new(Mutex::new(Vec::new())),
                scripted: Mutex::new(HashMap::new()),
                on_lock: Mutex::new(HashMap::new()),
                on_stat: Mutex::new(HashMap::new()),
                caps: Mutex::new(BTreeSet::new()),
                stat_fails: Mutex::new(BTreeSet::new()),
                cap_override: None,
                fs_type: "apfs".into(),
                shares: Mutex::new(HashMap::new()),
            }
        }
        fn log(&self, e: String) {
            self.events.lock().unwrap().push(e);
        }
        /// The recorded events that mention one of `names`, in order.
        fn about(&self, names: &[&str]) -> Vec<String> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .filter(|e| names.iter().any(|n| e.ends_with(&format!(" {n}"))))
                .cloned()
                .collect()
        }
        fn removals(&self) -> usize {
            self.events
                .lock()
                .unwrap()
                .iter()
                .filter(|e| e.starts_with("remove_"))
                .count()
        }
    }

    impl Fs for Rec {
        fn temp_dir(&self, _: &Path, _: &str) -> io::Result<TempDir> {
            Err(io::Error::other("not used by gc"))
        }
        fn write_file(&self, _: &Path, _: &[u8]) -> io::Result<()> {
            Err(io::Error::other("not used by gc"))
        }
        fn read(&self, _: &Path) -> io::Result<Vec<u8>> {
            Err(io::Error::other("not used by gc"))
        }
        fn hardlink(&self, _: &Path, _: &Path) -> io::Result<()> {
            Err(io::Error::other("not used by gc"))
        }
        fn set_mtime(&self, _: &Path, _: SystemTime) -> io::Result<()> {
            Err(io::Error::other("not used by gc"))
        }
        fn mtime(&self, _: &Path) -> io::Result<SystemTime> {
            Err(io::Error::other("not used by gc"))
        }
        fn inode(&self, _: &Path) -> io::Result<u64> {
            Err(io::Error::other("not used by gc"))
        }
        fn clone_file(&self, _: &Path, _: &Path) -> io::Result<()> {
            Err(io::Error::other("not used by gc"))
        }
        fn compress(&self, _: &Path) -> CompressOutcome {
            CompressOutcome::Failed("not used by gc".into())
        }
        fn hold_lock(&self, _: LockKind, _: &Path) -> io::Result<LockHolder> {
            Err(io::Error::other("not used by gc"))
        }
        fn try_lock_elsewhere(&self, _: LockKind, _: &Path) -> io::Result<LockState> {
            Err(io::Error::other("not used by gc"))
        }
        fn sharing(&self, path: &Path) -> io::Result<Sharing> {
            Ok(self
                .shares
                .lock()
                .unwrap()
                .get(&name(path))
                .copied()
                .unwrap_or(Sharing {
                    private_bytes: 0,
                    clone_id: 0,
                    may_share: false,
                    shares_all: false,
                }))
        }
        fn try_lock_exclusive(&self, path: &Path) -> TryLock {
            let n = name(path);
            self.log(format!("lock {n}"));
            if let Some(s) = self.scripted.lock().unwrap().get(&n).copied() {
                return match s {
                    Scripted::Held => TryLock::Held,
                    Scripted::Unopenable => TryLock::Unopenable(io::Error::other("scripted")),
                    Scripted::Unlockable => TryLock::Unlockable(io::Error::other("scripted")),
                };
            }
            if std::fs::symlink_metadata(path).is_err() {
                return TryLock::Gone;
            }
            if let Some(hook) = self.on_lock.lock().unwrap().remove(&n) {
                hook();
            }
            let events = Arc::clone(&self.events);
            TryLock::Acquired(HeldLock::new(move || {
                events.lock().unwrap().push(format!("unlock {n}"))
            }))
        }
        fn entry_stat(&self, path: &Path) -> io::Result<EntryStat> {
            let n = name(path);
            if let Some(hook) = self.on_stat.lock().unwrap().remove(&n) {
                hook();
            }
            self.log(format!("stat {n}"));
            if self.stat_fails.lock().unwrap().contains(&n) {
                return Err(io::Error::other("scripted stat failure"));
            }
            let m = std::fs::symlink_metadata(path)?;
            Ok(EntryStat {
                dev: m.dev(),
                ino: m.ino(),
                nlink: m.nlink(),
                blocks: m.blocks(),
                kind: kind_of(&m),
                compressed: false,
            })
        }
        fn read_dir_bounded(&self, dir: &Path, cap: usize) -> io::Result<Vec<(String, EntryKind)>> {
            self.caps.lock().unwrap().insert(cap);
            let entries: Vec<(String, EntryKind)> = std::fs::read_dir(dir)?
                .map(|e| {
                    let e = e?;
                    Ok((
                        e.file_name().to_string_lossy().into_owned(),
                        kind_of(&std::fs::symlink_metadata(e.path())?),
                    ))
                })
                .collect::<io::Result<_>>()?;
            if entries.len() > self.cap_override.unwrap_or(cap) {
                return Err(io::Error::other(format!(
                    "{} has more than {cap} entries",
                    dir.display()
                )));
            }
            Ok(entries)
        }
        fn remove_tree(&self, path: &Path) -> io::Result<()> {
            self.log(format!("remove_tree {}", name(path)));
            std::fs::remove_dir_all(path)
        }
        fn remove_file(&self, path: &Path) -> io::Result<()> {
            self.log(format!("remove_file {}", name(path)));
            std::fs::remove_file(path)
        }
        fn volume(&self, _: &Path) -> io::Result<VolumeInfo> {
            Ok(VolumeInfo {
                fs_type: self.fs_type.clone(),
                container: "disk1".into(),
                mount: PathBuf::from("/"),
                available_bytes: 0,
            })
        }
    }

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

    struct Fixture {
        root: PathBuf,
        profile: PathBuf,
        krate: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn fixture(label: &str) -> Fixture {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let root = PathBuf::from(format!(
            "/private/tmp/hive-it-gcrun-{label}-{}-{n}",
            std::process::id()
        ));
        let profile = root.join("target/debug");
        let krate = profile.join("incremental/fixture-0abc");
        std::fs::create_dir_all(&krate).unwrap();
        std::fs::create_dir_all(profile.join("deps")).unwrap();
        std::fs::write(profile.join(".cargo-build-lock"), b"").unwrap();
        Fixture {
            root: root.canonicalize().unwrap(),
            profile: profile.canonicalize().unwrap(),
            krate: krate.canonicalize().unwrap(),
        }
    }

    /// A session `secs_ago` old with one 8 KiB file and (unless `lockless`) its lock file.
    fn session(
        f: &Fixture,
        secs_ago: u64,
        rand: &str,
        last: &str,
        lockless: bool,
    ) -> (String, String) {
        let ts = (SystemTime::now().duration_since(UNIX_EPOCH).unwrap()
            - Duration::from_secs(secs_ago))
        .as_micros() as u64;
        let key = format!("s-{}-{rand}", b36(ts));
        let dir = format!("{key}-{last}");
        std::fs::create_dir_all(f.krate.join(&dir)).unwrap();
        std::fs::write(f.krate.join(&dir).join("query-cache.bin"), vec![1u8; 8192]).unwrap();
        let lock = format!("{key}.lock");
        if !lockless {
            std::fs::write(f.krate.join(&lock), b"").unwrap();
        }
        (dir, lock)
    }

    fn target(path: &Path, profile: &Path, owner: Owner) -> TargetDir {
        TargetDir {
            path: path.to_path_buf(),
            owner,
            profiles: vec![ProfileReport {
                path: profile.to_path_buf(),
                format: Format::Supported,
                units: 0,
                edges: None,
                graph: None,
            }],
            size: None,
            unreadable: None,
        }
    }

    fn orphan(f: &Fixture) -> TargetDir {
        target(
            &f.root.join("target"),
            &f.profile,
            Owner::OrphanCandidate {
                owned_under_caller_env: None,
            },
        )
    }

    fn opts(dry_run: bool) -> RunOpts {
        RunOpts {
            dry_run,
            now: SystemTime::now(),
            account_bytes: true,
        }
    }

    fn never() -> impl Fn() -> bool {
        || false
    }

    #[test]
    fn a_superseded_session_is_removed_dir_first_under_its_lock() {
        let f = fixture("order");
        let (old, old_lock) = session(&f, 3600, "aa", "svh1", false);
        let (new, _) = session(&f, 60, "bb", "svh2", false);
        let rec = Rec::new();
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert_eq!(
            rec.about(&[&old, &old_lock]),
            vec![
                format!("lock {old_lock}"),
                format!("stat {old}"),
                format!("remove_tree {old}"),
                format!("remove_file {old_lock}"),
                format!("unlock {old_lock}")
            ]
        );
        assert!(f.krate.join(&new).exists() && !f.krate.join(&old).exists());
        assert_eq!(
            (
                report.totals.sessions_deleted,
                report.totals.session_lock_files_deleted
            ),
            (1, 1)
        );
        assert!(report.totals.bytes.unwrap().freed > 0);
    }

    #[test]
    fn a_cloned_file_frees_only_its_private_bytes() {
        let f = fixture("clone");
        let (old, _) = session(&f, 3600, "aa", "svh1", false);
        session(&f, 60, "bb", "svh2", false);
        let file = f.krate.join(&old).join("query-cache.bin");
        let all = std::fs::symlink_metadata(&file).unwrap().blocks() * 512;
        let private = 1024;
        assert!(
            all > private,
            "the fixture file allocates {all} bytes, which is not above the private size"
        );
        let rec = Rec::new();
        rec.shares.lock().unwrap().insert(
            "query-cache.bin".into(),
            Sharing {
                private_bytes: private,
                clone_id: 7,
                may_share: true,
                shares_all: false,
            },
        );
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert_eq!(report.totals.sessions_deleted, 1);
        let bytes = report.totals.bytes.unwrap();
        assert_eq!(bytes.freed, private);
        assert_eq!(bytes.still_shared, all - private);
    }

    #[test]
    fn a_session_that_finalizes_after_the_listing_keeps_its_lock_file() {
        let f = fixture("race");
        let (working, lock) = session(&f, 3600, "cc", "working", false);
        let finalized = working.replace("-working", "-svh9");
        let (from, to) = (f.krate.join(&working), f.krate.join(&finalized));
        let rec = Rec::new();
        rec.on_lock.lock().unwrap().insert(
            lock.clone(),
            Box::new(move || std::fs::rename(from, to).unwrap()),
        );
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert!(
            f.krate.join(&lock).exists(),
            "the finalized session lost its lock file"
        );
        assert!(f.krate.join(&finalized).exists());
        assert_eq!(rec.removals(), 0);
        assert_eq!(report.totals.sessions_deleted, 0);
        assert!(
            report.skipped.is_empty(),
            "a vanished session is not a failure: {:?}",
            report.skipped
        );
    }

    #[test]
    fn a_locked_session_whose_directory_cannot_be_statted_is_left_and_reported() {
        let f = fixture("stat-fail");
        let (old, old_lock) = session(&f, 3600, "aa", "svh1", false);
        session(&f, 60, "bb", "svh2", false);
        let rec = Rec::new();
        rec.stat_fails.lock().unwrap().insert(old.clone());
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert!(f.krate.join(&old).exists(), "the session was deleted");
        assert!(
            f.krate.join(&old_lock).exists(),
            "the lock file was removed"
        );
        assert_eq!(report.totals.sessions_deleted, 0);
        assert!(
            report
                .skipped
                .iter()
                .any(|s| s.path.ends_with(&old) && s.reason.contains("could not stat it")),
            "{:?}",
            report.skipped
        );
    }

    #[test]
    fn held_unopenable_and_unlockable_locks_leave_their_sessions() {
        let f = fixture("refusals");
        let s1 = session(&f, 3600, "a1", "svh1", false);
        let s2 = session(&f, 3500, "a2", "svh2", false);
        let s3 = session(&f, 3400, "a3", "svh3", false);
        session(&f, 60, "bb", "svh4", false);
        let rec = Rec::new();
        rec.scripted.lock().unwrap().extend([
            (s1.1.clone(), Scripted::Held),
            (s2.1.clone(), Scripted::Unopenable),
            (s3.1.clone(), Scripted::Unlockable),
        ]);
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert_eq!(
            (
                report.totals.skipped_held,
                report.totals.skipped_lock_unopenable
            ),
            (1, 1)
        );
        assert!(
            report
                .skipped
                .iter()
                .any(|s| s.path.ends_with(&s3.1) && s.reason.contains("could not be taken")),
            "{:?}",
            report.skipped
        );
        for (dir, _) in [&s1, &s2, &s3] {
            assert!(f.krate.join(dir).exists(), "{dir}");
        }
    }

    #[test]
    fn a_lockless_dir_is_removed_unless_its_lock_appears() {
        let f = fixture("lockless");
        let (gone, _) = session(&f, 3600, "d1", "svh1", true);
        let (kept, kept_lock) = session(&f, 3500, "d2", "svh2", true);
        let appears = f.krate.join(&kept_lock);
        let rec = Rec::new();
        rec.on_stat.lock().unwrap().insert(
            kept_lock.clone(),
            Box::new(move || std::fs::write(appears, b"").unwrap()),
        );
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert!(!f.krate.join(&gone).exists());
        assert!(f.krate.join(&kept).exists());
        assert_eq!(report.totals.sessions_deleted, 1);
    }

    #[test]
    fn a_lockless_session_whose_lock_cannot_be_statted_is_left_and_reported() {
        let f = fixture("lockless-stat");
        let (dir, lock) = session(&f, 3600, "d1", "svh1", true);
        session(&f, 60, "bb", "svh2", false);
        let rec = Rec::new();
        rec.stat_fails.lock().unwrap().insert(lock.clone());
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert!(f.krate.join(&dir).exists(), "the session was deleted");
        assert_eq!(report.totals.sessions_deleted, 0);
        assert!(
            report
                .skipped
                .iter()
                .any(|s| s.path.ends_with(&lock) && s.reason.contains("could not stat it")),
            "{:?}",
            report.skipped
        );
    }

    #[test]
    fn a_dry_run_counts_nothing_when_a_lockless_dir_vanishes() {
        let f = fixture("dry-vanish");
        let (gone, gone_lock) = session(&f, 3600, "v1", "svh1", true);
        session(&f, 60, "bb", "svh2", false);
        let vanish = f.krate.join(&gone);
        let rec = Rec::new();
        rec.on_stat.lock().unwrap().insert(
            gone_lock,
            Box::new(move || {
                let _ = std::fs::remove_dir_all(&vanish);
            }),
        );
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(true), &never());
        assert_eq!(rec.removals(), 0);
        assert_eq!(report.totals.sessions_deleted, 0);
    }

    #[test]
    fn an_orphan_lock_is_removed_unless_a_session_claims_it() {
        let f = fixture("orphan");
        let (d1, l1) = session(&f, 3600, "e1", "svh1", false);
        let (d2, l2) = session(&f, 3500, "e2", "svh2", false);
        std::fs::remove_dir_all(f.krate.join(&d1)).unwrap();
        std::fs::remove_dir_all(f.krate.join(&d2)).unwrap();
        let claimant = f.krate.join(d2.replace("-svh2", "-working"));
        let rec = Rec::new();
        rec.on_lock.lock().unwrap().insert(
            l2.clone(),
            Box::new(move || std::fs::create_dir_all(claimant).unwrap()),
        );
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert!(!f.krate.join(&l1).exists());
        assert!(
            f.krate.join(&l2).exists(),
            "a lock file a session claims was removed"
        );
        assert_eq!(report.totals.session_lock_files_deleted, 1);
    }

    #[test]
    fn temp_entries_go_under_the_build_lock_and_look_alikes_stay() {
        let f = fixture("temps");
        let deps = f.profile.join("deps");
        for d in ["rmetaAb1", "rustcXyz"] {
            std::fs::create_dir_all(deps.join(d)).unwrap();
            std::fs::write(deps.join(d).join("lib.rmeta"), vec![2u8; 4096]).unwrap();
        }
        std::fs::write(deps.join("rustc_hash-0123456789abcdef.d"), b"x").unwrap();
        std::fs::create_dir_all(deps.join("rustc_hash-0123456789abcdef")).unwrap();
        std::os::unix::fs::symlink(&f.root, deps.join("rmetaLnk")).unwrap();
        let rec = Rec::new();
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert!(!deps.join("rmetaAb1").exists() && !deps.join("rustcXyz").exists());
        assert!(
            deps.join("rustc_hash-0123456789abcdef.d").exists()
                && deps.join("rustc_hash-0123456789abcdef").exists()
        );
        assert!(std::fs::symlink_metadata(deps.join("rmetaLnk")).is_ok() && f.root.exists());
        assert_eq!(report.totals.temp_entries_deleted, 2);
        let around = rec.about(&[".cargo-build-lock", "rmetaAb1", "rustcXyz"]);
        assert_eq!(around.first().unwrap(), "lock .cargo-build-lock");
        assert_eq!(around.last().unwrap(), "unlock .cargo-build-lock");
    }

    #[test]
    fn a_held_or_missing_build_lock_leaves_temp_entries() {
        let f = fixture("buildlock");
        std::fs::create_dir_all(f.profile.join("deps/rmetaQ1")).unwrap();
        let rec = Rec::new();
        rec.scripted
            .lock()
            .unwrap()
            .insert(".cargo-build-lock".into(), Scripted::Held);
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert_eq!(report.totals.skipped_held, 1);
        assert!(f.profile.join("deps/rmetaQ1").exists());
        std::fs::remove_file(f.profile.join(".cargo-build-lock")).unwrap();
        let report = run(
            &Rec::new(),
            &[orphan(&f)],
            &Scope::All,
            &opts(false),
            &never(),
        );
        assert!(f.profile.join("deps/rmetaQ1").exists());
        assert!(
            report
                .skipped
                .iter()
                .any(|s| s.reason.contains(".cargo-build-lock")),
            "{:?}",
            report.skipped
        );
    }

    #[test]
    fn an_unopenable_build_lock_leaves_temp_entries() {
        let f = fixture("buildlock-unopen");
        std::fs::create_dir_all(f.profile.join("deps/rmetaQ1")).unwrap();
        let rec = Rec::new();
        rec.scripted
            .lock()
            .unwrap()
            .insert(".cargo-build-lock".into(), Scripted::Unopenable);
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert!(f.profile.join("deps/rmetaQ1").exists());
        assert!(
            report.skipped.iter().any(|s| {
                s.path.ends_with(".cargo-build-lock") && s.reason.contains("could not open")
            }),
            "{:?}",
            report.skipped
        );
    }

    #[test]
    fn a_dry_run_counts_what_a_real_run_deletes_and_removes_nothing() {
        let build = |label: &str| {
            let f = fixture(label);
            session(&f, 3600, "aa", "svh1", false);
            session(&f, 60, "bb", "svh2", false);
            session(&f, 3500, "cc", "svh3", true);
            std::fs::create_dir_all(f.profile.join("deps/rmetaZ9")).unwrap();
            std::fs::write(f.profile.join("deps/rmetaZ9/x"), vec![3u8; 4096]).unwrap();
            f
        };
        let (a, b) = (build("dry"), build("real"));
        let rec = Rec::new();
        let dry = run(&rec, &[orphan(&a)], &Scope::All, &opts(true), &never());
        assert_eq!(rec.removals(), 0);
        let real = run(
            &Rec::new(),
            &[orphan(&b)],
            &Scope::All,
            &opts(false),
            &never(),
        );
        assert_eq!(dry.totals, real.totals);
        assert!(
            dry.totals.sessions_deleted == 2 && dry.totals.temp_entries_deleted == 1,
            "{:?}",
            dry.totals
        );
    }

    #[test]
    fn scope_selects_owned_or_contained_targets_and_skips_what_it_cannot_judge() {
        let f = fixture("scope");
        let checkout = f.root.clone();
        let mut unreadable = target(
            &f.root.join("u"),
            &f.profile,
            Owner::Unresolved { reason: "x".into() },
        );
        unreadable.unreadable = Some("permission denied".into());
        let mut unsupported = target(
            &f.root.join("n"),
            &f.profile,
            Owner::Owned {
                checkout: checkout.clone(),
            },
        );
        unsupported.profiles[0].format = Format::Unsupported("no .fingerprint/".into());
        let targets = vec![
            target(
                Path::new("/private/tmp/elsewhere-owned"),
                &f.profile,
                Owner::Owned {
                    checkout: checkout.clone(),
                },
            ),
            orphan(&f),
            target(
                Path::new("/private/tmp/cache-root/t"),
                &f.profile,
                Owner::Unresolved {
                    reason: "scope".into(),
                },
            ),
            unreadable,
            unsupported,
        ];
        let paths = |r: &GcReport| {
            r.per_target
                .iter()
                .map(|(p, _)| p.clone())
                .collect::<Vec<_>>()
        };
        let scoped = run(
            &Rec::new(),
            &targets,
            &Scope::Checkouts(vec![checkout.clone()]),
            &opts(true),
            &never(),
        );
        assert_eq!(
            paths(&scoped),
            vec![
                PathBuf::from("/private/tmp/elsewhere-owned"),
                f.root.join("target"),
                f.root.join("n")
            ]
        );
        assert!(scoped
            .skipped
            .iter()
            .any(|s| s.path == f.root.join("u") && s.reason.contains("unreadable")));
        assert!(scoped
            .skipped
            .iter()
            .any(|s| s.path == f.profile && s.reason.contains("unsupported")));
        let all = run(&Rec::new(), &targets, &Scope::All, &opts(true), &never());
        assert!(paths(&all).contains(&PathBuf::from("/private/tmp/cache-root/t")));
        let mut hfs = Rec::new();
        hfs.fs_type = "hfs".into();
        let skipped = run(&hfs, &[orphan(&f)], &Scope::All, &opts(true), &never());
        assert!(skipped.per_target.is_empty());
        assert!(skipped.skipped.iter().any(|s| s
            .reason
            .contains("not on an APFS volume (filesystem type hfs)")));
    }

    #[test]
    fn a_cancel_stops_before_the_next_entry() {
        let f = fixture("cancel");
        for (i, r) in ["c1", "c2", "c3"].iter().enumerate() {
            session(&f, 3600 - i as u64 * 10, r, "svh1", false);
        }
        session(&f, 60, "zz", "svh9", false);
        let calls = AtomicUsize::new(0);
        let cancel = || calls.fetch_add(1, Ordering::SeqCst) >= 3;
        let report = run(
            &Rec::new(),
            &[orphan(&f)],
            &Scope::All,
            &opts(false),
            &cancel,
        );
        assert!(report.cancelled);
        assert!(report.totals.sessions_deleted < 3, "{:?}", report.totals);
    }

    #[test]
    fn listings_use_the_designed_caps() {
        assert_eq!((CRATE_DIR_CAP, DEPS_CAP), (100_000, 1_000_000));
        let f = fixture("caps");
        let rec = Rec::new();
        run(&rec, &[orphan(&f)], &Scope::All, &opts(true), &never());
        let caps = rec.caps.lock().unwrap().clone();
        assert_eq!(caps, BTreeSet::from([CRATE_DIR_CAP, DEPS_CAP]));
    }

    #[test]
    fn without_byte_accounting_the_byte_counts_are_absent() {
        let f = fixture("noaccount");
        session(&f, 3600, "aa", "svh1", false);
        session(&f, 60, "bb", "svh2", false);
        let off = RunOpts {
            account_bytes: false,
            ..opts(true)
        };
        let report = run(&Rec::new(), &[orphan(&f)], &Scope::All, &off, &never());
        assert_eq!(report.totals.bytes, None);
        assert_eq!(report.totals.sessions_deleted, 1);
    }

    #[test]
    fn a_session_nested_past_the_depth_limit_is_left_with_a_finding() {
        let nest = |levels: usize| {
            let f = fixture("depth");
            let (old, _) = session(&f, 3600, "aa", "svh1", false);
            session(&f, 60, "bb", "svh2", false);
            let mut deep = f.krate.join(&old);
            for i in 0..levels {
                deep = deep.join(format!("d{i}"));
            }
            std::fs::create_dir_all(&deep).unwrap();
            let report = run(
                &Rec::new(),
                &[orphan(&f)],
                &Scope::All,
                &opts(false),
                &never(),
            );
            (f.krate.join(&old).exists(), report)
        };
        let (left, report) = nest(MAX_DEPTH);
        assert!(
            !left && report.totals.sessions_deleted == 1,
            "{:?}",
            report.skipped
        );
        let (left, report) = nest(MAX_DEPTH + 1);
        assert!(left && report.totals.sessions_deleted == 0);
        assert!(
            report
                .skipped
                .iter()
                .any(|s| s.reason.contains("deeper than")),
            "{:?}",
            report.skipped
        );
    }

    #[test]
    fn a_listing_one_past_its_cap_deletes_nothing_and_is_reported() {
        let build = |label: &str| {
            let f = fixture(label);
            session(&f, 3600, "aa", "svh1", false);
            session(&f, 60, "bb", "svh2", false);
            f
        };
        // The crate dir holds 4 entries: two sessions and their lock files.
        let at = build("cap-at");
        let mut rec = Rec::new();
        rec.cap_override = Some(4);
        let report = run(&rec, &[orphan(&at)], &Scope::All, &opts(false), &never());
        assert_eq!(report.totals.sessions_deleted, 1);
        let past = build("cap-past");
        let mut rec = Rec::new();
        rec.cap_override = Some(3);
        let report = run(&rec, &[orphan(&past)], &Scope::All, &opts(false), &never());
        assert_eq!(report.totals.sessions_deleted, 0);
        assert_eq!(rec.removals(), 0);
        assert!(
            report
                .skipped
                .iter()
                .any(|s| s.reason.contains("could not list it")),
            "{:?}",
            report.skipped
        );
    }

    #[test]
    fn an_orphan_lock_is_kept_when_its_crate_dir_cannot_be_relisted() {
        use std::os::unix::fs::PermissionsExt;
        let f = fixture("orphan-unlistable");
        let (dir, lock) = session(&f, 3600, "e1", "svh1", false);
        std::fs::remove_dir_all(f.krate.join(&dir)).unwrap();
        let krate = f.krate.clone();
        let rec = Rec::new();
        rec.on_lock.lock().unwrap().insert(
            lock.clone(),
            Box::new(move || {
                std::fs::set_permissions(krate, std::fs::Permissions::from_mode(0o000)).unwrap()
            }),
        );
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        std::fs::set_permissions(&f.krate, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            f.krate.join(&lock).exists(),
            "a lock file was removed although the re-check could not list its dir"
        );
        assert_eq!(report.totals.session_lock_files_deleted, 0);
        assert!(
            report
                .skipped
                .iter()
                .any(|s| s.path == f.krate && s.reason.contains("could not list it")),
            "{:?}",
            report.skipped
        );
    }

    #[test]
    fn a_file_that_cannot_be_measured_is_counted_in_neither_byte_count() {
        let f = fixture("unmeasured");
        let (old, _) = session(&f, 3600, "aa", "svh1", false);
        session(&f, 60, "bb", "svh2", false);
        std::fs::write(f.krate.join(&old).join("opaque.bin"), vec![4u8; 65536]).unwrap();
        let rec = Rec::new();
        rec.stat_fails.lock().unwrap().insert("opaque.bin".into());
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert!(
            report
                .unmeasured
                .iter()
                .any(|(p, _)| p.ends_with("opaque.bin")),
            "{:?}",
            report.unmeasured
        );
        let bytes = report.totals.bytes.unwrap();
        let counted = bytes.freed + bytes.still_shared;
        assert!(
            counted < 65536,
            "the unmeasured file was counted: {counted}"
        );
        assert_eq!(report.totals.sessions_deleted, 1);
    }

    #[test]
    fn nothing_outside_the_allowed_entries_changes() {
        let f = fixture("allowed");
        let (old, old_lock) = session(&f, 3600, "aa", "svh1", false);
        session(&f, 60, "bb", "svh2", false);
        for p in [
            ".fingerprint/fixture-0abc/lib-fixture",
            "build/fixture-0abc/output",
            "deps/libfixture-0abc.rlib",
            "fixture",
        ] {
            std::fs::create_dir_all(f.profile.join(p).parent().unwrap()).unwrap();
            std::fs::write(f.profile.join(p), b"keep").unwrap();
        }
        std::fs::create_dir_all(f.profile.join("deps/rmetaT1")).unwrap();
        let listing = |root: &Path| {
            let mut out = BTreeSet::new();
            let mut stack = vec![root.to_path_buf()];
            while let Some(d) = stack.pop() {
                for e in std::fs::read_dir(&d).unwrap() {
                    let p = e.unwrap().path();
                    if std::fs::symlink_metadata(&p).unwrap().is_dir() {
                        stack.push(p.clone());
                    }
                    out.insert(p);
                }
            }
            out
        };
        let before = listing(&f.root);
        run(
            &Rec::new(),
            &[orphan(&f)],
            &Scope::All,
            &opts(false),
            &never(),
        );
        let after = listing(&f.root);
        let removed: BTreeSet<PathBuf> = before.difference(&after).cloned().collect();
        let allowed = |p: &Path| {
            p.starts_with(f.krate.join(&old))
                || p == f.krate.join(&old_lock)
                || p.starts_with(f.profile.join("deps/rmetaT1"))
        };
        assert!(removed.iter().all(|p| allowed(p)), "{removed:?}");
        assert!(after.is_subset(&before));
    }
}
