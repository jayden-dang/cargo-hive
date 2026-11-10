//! Complete, compressed registry units under `store/`, and the `store.db` rows
//! that make an entry available. A temp directory is renamed into place only
//! after every file is compressed; `store.db` is merged under the state lock.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::fingerprint;
use crate::gc::CRATE_DIR_CAP;
use crate::platform::{CompressOutcome, EntryKind, Fs};
use crate::size::{allocated_bytes, SizeError, MAX_DEPTH};
use crate::state::{State, StateError};

pub const STORE_FILE: &str = "store.db";
pub const MAX_STORE_BYTES: u64 = 32 * 1024 * 1024;
const TMP_KEEP: Duration = Duration::from_secs(86_400);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StoreKey {
    pub toolchain_value: u64,
    pub name_meta: String,
    pub fingerprint_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub toolchain_value: u64,
    pub name_meta: String,
    pub fingerprint_hash: String,
    pub kind: String,
    pub rel_profile: PathBuf,
    pub size: u64,
    pub filled_at: u64,
    pub last_live_at: u64,
    pub repos: Vec<PathBuf>,
    pub registry_deps: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreFile {
    pub units: Vec<Row>,
    pub toolchains: BTreeMap<String, u64>,
}

pub fn store_dir(state: &State) -> PathBuf {
    state.root().join("store")
}

pub fn entry_dir(state: &State, row: &Row) -> PathBuf {
    key_dir(state, &row_key(row))
}

pub fn tag_path(state: &State) -> PathBuf {
    store_dir(state).join("CACHEDIR.TAG")
}

pub fn tmp_dir(state: &State, key: &str) -> PathBuf {
    store_dir(state)
        .join(".tmp")
        .join(format!("{}-{key}", std::process::id()))
}

pub fn evict_dir(state: &State, pid: u32, seq: usize) -> PathBuf {
    store_dir(state)
        .join(".tmp")
        .join(format!("evict-{pid}-{seq}"))
}

pub(crate) fn key_dir(state: &State, key: &StoreKey) -> PathBuf {
    store_dir(state)
        .join(key.toolchain_value.to_string())
        .join(format!("{}-{}", key.name_meta, key.fingerprint_hash))
}

pub fn rename_out(state: &State, key: &StoreKey, to: &Path) -> io::Result<()> {
    fs::create_dir_all(store_dir(state).join(".tmp"))?;
    fs::rename(key_dir(state, key), to)
}

pub fn sweep_tmp(state: &State, now: SystemTime) -> Result<(), String> {
    sweep_tmp_bounded(state, now, CRATE_DIR_CAP)
}

fn sweep_tmp_bounded(state: &State, now: SystemTime, cap: usize) -> Result<(), String> {
    let dir = store_dir(state).join(".tmp");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(format!(
                "{}: {err}; remove the temp directory and run the command again",
                dir.display()
            ));
        }
    };
    for (seen, entry) in entries.enumerate() {
        if seen >= cap {
            return Err(format!(
                "{} has more than {cap} entries; remove temp entries until fewer remain and run the command again",
                dir.display()
            ));
        }
        let entry = entry.map_err(|err| {
            format!(
                "{}: {err}; remove the temp directory and run the command again",
                dir.display()
            )
        })?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).map_err(|err| {
            format!(
                "{}: {err}; remove that temp entry and run the command again",
                path.display()
            )
        })?;
        let modified = meta.modified().map_err(|err| {
            format!(
                "{}: {err}; remove that temp entry and run the command again",
                path.display()
            )
        })?;
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age <= TMP_KEEP {
            continue;
        }
        remove_aged(&path, meta.file_type().is_dir())?;
    }
    Ok(())
}

fn remove_aged(path: &Path, is_dir: bool) -> Result<(), String> {
    let removed = if is_dir {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    removed.map_err(|err| {
        format!(
            "{}: {err}; remove that temp entry and run the command again",
            path.display()
        )
    })
}

pub fn read(state: &State) -> Result<StoreFile, StateError> {
    let mut file: StoreFile = state.read_bounded(STORE_FILE, MAX_STORE_BYTES)?;
    let mut units = Vec::with_capacity(file.units.len());
    for row in file.units.drain(..) {
        if directory_present(&entry_dir(state, &row))? {
            units.push(row);
        }
    }
    file.units = units;
    Ok(file)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Published {
    Added,
    Held,
}

#[derive(Debug)]
pub enum PublishFailed {
    Compress(String),
    DiskFull(String),
    Stopped(String),
}

impl fmt::Display for PublishFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PublishFailed::Compress(why) => {
                write!(
                    f,
                    "could not publish the entry ({why}); it was left unavailable"
                )
            }
            PublishFailed::DiskFull(why) => f.write_str(&super::disk_full(why)),
            PublishFailed::Stopped(path) => {
                write!(
                    f,
                    "stopped while publishing an entry ({path}); run the command again"
                )
            }
        }
    }
}

impl std::error::Error for PublishFailed {}

pub fn compress_and_publish(
    fs: &dyn Fs,
    state: &State,
    tmp: &Path,
    row: &Row,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Published, PublishFailed> {
    let mut files = Vec::new();
    list_files(fs, tmp, 0, &mut files)?;
    files.sort();
    for path in &files {
        if cancel() {
            return Err(PublishFailed::Stopped(tmp.display().to_string()));
        }
        compress_one(fs, tmp, path)?;
    }
    if cancel() {
        return Err(PublishFailed::Stopped(tmp.display().to_string()));
    }
    publish(state, tmp, row)
}

pub fn merge(
    state: &State,
    rows: Vec<Row>,
    table: BTreeMap<String, u64>,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<(), StateError> {
    state.locked_or_cancelled(cancel, |tx| {
        let mut file: StoreFile = tx.read_bounded(STORE_FILE, MAX_STORE_BYTES)?;
        let mut kept = Vec::with_capacity(file.units.len());
        for row in file.units.drain(..) {
            if directory_present(&entry_dir(state, &row))? {
                kept.push(row);
            }
        }
        file.units = kept;
        for row in rows {
            if !directory_present(&entry_dir(state, &row))? {
                continue;
            }
            if let Some(existing) = file.units.iter_mut().find(|kept| same_key(kept, &row)) {
                for repo in row.repos {
                    if !existing.repos.contains(&repo) {
                        existing.repos.push(repo);
                    }
                }
                if row.last_live_at > existing.last_live_at {
                    existing.last_live_at = row.last_live_at;
                }
            } else {
                file.units.push(row);
            }
        }
        for (text, value) in table {
            file.toolchains.entry(text).or_insert(value);
        }
        tx.write_bounded(STORE_FILE, &file, MAX_STORE_BYTES)
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inventory {
    pub rows: Vec<Row>,
    pub rowless: Vec<(StoreKey, PathBuf)>,
    pub tmp_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitPlan {
    pub candidates: Vec<(StoreKey, Option<u64>)>,
    pub refreshed: Vec<StoreKey>,
    pub now: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed {
    pub renamed: Vec<(StoreKey, PathBuf)>,
    pub rename_failures: Vec<(StoreKey, String)>,
    pub kept_changed: usize,
    pub dropped_values: Vec<u64>,
}

pub fn inventory(fs: &dyn Fs, state: &State) -> Result<Inventory, StateError> {
    let file = read(state)?;
    let mut keyed = BTreeMap::new();
    for row in &file.units {
        keyed.insert(row_key(row), ());
    }
    let mut rowless = Vec::new();
    let root = store_dir(state);
    let top = match fs.read_dir_bounded(&root, CRATE_DIR_CAP) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(err) => {
            return Err(StateError::Io {
                path: root,
                source: err,
            });
        }
    };
    for (name, kind) in top {
        if kind != EntryKind::Dir || name == ".tmp" {
            continue;
        }
        let Ok(value) = name.parse::<u64>() else {
            continue;
        };
        let value_dir = root.join(&name);
        let children = fs
            .read_dir_bounded(&value_dir, CRATE_DIR_CAP)
            .map_err(|err| StateError::Io {
                path: value_dir.clone(),
                source: err,
            })?;
        for (child, child_kind) in children {
            if child_kind != EntryKind::Dir {
                continue;
            }
            let Some(key) = parse_entry_key(value, &child) else {
                continue;
            };
            if keyed.contains_key(&key) {
                continue;
            }
            let path = value_dir.join(&child);
            rowless.push((key, path));
        }
    }
    Ok(Inventory {
        rows: file.units,
        rowless,
        tmp_bytes: tmp_allocated(state)?,
    })
}

pub fn commit_eviction(
    state: &State,
    plan: &CommitPlan,
    cancel: &dyn Fn() -> bool,
    mut rename: impl FnMut(&StoreKey) -> io::Result<Option<PathBuf>>,
) -> Result<Committed, StateError> {
    state.locked_or_cancelled(cancel, |tx| {
        let mut file: StoreFile = tx.read_bounded(STORE_FILE, MAX_STORE_BYTES)?;
        let mut kept = Vec::with_capacity(file.units.len());
        for row in file.units.drain(..) {
            if directory_present(&entry_dir(state, &row))? {
                kept.push(row);
            }
        }
        file.units = kept;

        let mut survivors = Vec::new();
        let mut kept_changed = 0usize;
        for (key, planned) in &plan.candidates {
            match planned {
                Some(planned_at) => match file.units.iter().find(|row| row_matches(row, key)) {
                    Some(row) if row.last_live_at > *planned_at => kept_changed += 1,
                    Some(_) => survivors.push(key.clone()),
                    None => {}
                },
                None => {
                    if file.units.iter().any(|row| row_matches(row, key)) {
                        kept_changed += 1;
                    } else {
                        survivors.push(key.clone());
                    }
                }
            }
        }

        let mut renamed = Vec::new();
        let mut rename_failures = Vec::new();
        for key in survivors {
            match rename(&key) {
                Ok(None) => break,
                Ok(Some(path)) => renamed.push((key, path)),
                Err(err) => rename_failures.push((key, err.to_string())),
            }
        }

        let renamed_keys: BTreeMap<_, ()> = renamed.iter().map(|(k, _)| (k.clone(), ())).collect();
        file.units
            .retain(|row| !renamed_keys.contains_key(&row_key(row)));
        for key in &plan.refreshed {
            if let Some(row) = file.units.iter_mut().find(|row| row_matches(row, key)) {
                if plan.now > row.last_live_at {
                    row.last_live_at = plan.now;
                }
            }
        }

        let mut dropped_values = Vec::new();
        let mut values = BTreeMap::new();
        for value in file.toolchains.values().copied() {
            values.insert(value, ());
        }
        for value in values.into_keys() {
            let has_row = file.units.iter().any(|row| row.toolchain_value == value);
            if has_row || value_has_entry_dir(state, value)? {
                continue;
            }
            file.toolchains.retain(|_, v| *v != value);
            dropped_values.push(value);
        }

        tx.write_bounded(STORE_FILE, &file, MAX_STORE_BYTES)?;
        Ok(Committed {
            renamed,
            rename_failures,
            kept_changed,
            dropped_values,
        })
    })
}

fn same_key(left: &Row, right: &Row) -> bool {
    left.toolchain_value == right.toolchain_value
        && left.name_meta == right.name_meta
        && left.fingerprint_hash == right.fingerprint_hash
}

pub(crate) fn row_key(row: &Row) -> StoreKey {
    StoreKey {
        toolchain_value: row.toolchain_value,
        name_meta: row.name_meta.clone(),
        fingerprint_hash: row.fingerprint_hash.clone(),
    }
}

pub(crate) fn row_matches(row: &Row, key: &StoreKey) -> bool {
    row.toolchain_value == key.toolchain_value
        && row.name_meta == key.name_meta
        && row.fingerprint_hash == key.fingerprint_hash
}

fn parse_entry_key(toolchain_value: u64, name: &str) -> Option<StoreKey> {
    let hash = fingerprint::unit_hash(name)?;
    let name_meta = name[..name.len() - hash.len() - 1].to_string();
    Some(StoreKey {
        toolchain_value,
        name_meta,
        fingerprint_hash: hash.to_string(),
    })
}

fn tmp_allocated(state: &State) -> Result<u64, StateError> {
    let dir = store_dir(state).join(".tmp");
    match allocated_bytes(&dir, MAX_DEPTH) {
        Ok(bytes) => Ok(bytes),
        Err(SizeError::Io(_, err)) if err.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(SizeError::Io(path, err)) => Err(StateError::Io { path, source: err }),
        Err(SizeError::DepthLimit(path)) => Err(StateError::Io {
            path,
            source: io::Error::other("deeper than the size walk's limit"),
        }),
    }
}

fn value_has_entry_dir(state: &State, value: u64) -> Result<bool, StateError> {
    let dir = store_dir(state).join(value.to_string());
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => {
            return Err(StateError::Io {
                path: dir,
                source: err,
            });
        }
    };
    for (seen, entry) in entries.enumerate() {
        if seen >= CRATE_DIR_CAP {
            return Err(StateError::Io {
                path: dir.clone(),
                source: io::Error::other(format!(
                    "{} has more than {CRATE_DIR_CAP} entries",
                    dir.display()
                )),
            });
        }
        let entry = entry.map_err(|err| StateError::Io {
            path: dir.clone(),
            source: err,
        })?;
        let os_name = entry.file_name();
        let Some(name) = os_name.to_str() else {
            continue;
        };
        if fingerprint::unit_hash(name).is_none() {
            continue;
        }
        let file_type = entry.file_type().map_err(|err| StateError::Io {
            path: entry.path(),
            source: err,
        })?;
        if file_type.is_dir() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn directory_present(path: &Path) -> Result<bool, StateError> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(meta.file_type().is_dir()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(StateError::Io {
            path: path.to_path_buf(),
            source: err,
        }),
    }
}

fn list_files(
    fs: &dyn Fs,
    dir: &Path,
    depth: usize,
    out: &mut Vec<PathBuf>,
) -> Result<(), PublishFailed> {
    if depth > MAX_DEPTH {
        return Err(PublishFailed::Compress(format!(
            "{} is deeper than {MAX_DEPTH} levels; remove directories past that depth and run the command again",
            dir.display()
        )));
    }
    let entries = fs
        .read_dir_bounded(dir, CRATE_DIR_CAP)
        .map_err(|err| PublishFailed::Compress(format!("{}: {err}", dir.display())))?;
    for (name, kind) in entries {
        let path = dir.join(name);
        match kind {
            EntryKind::Dir => list_files(fs, &path, depth + 1, out)?,
            EntryKind::File => out.push(path),
            EntryKind::Symlink | EntryKind::Other => {
                return Err(PublishFailed::Compress(format!(
                    "{} is not a regular file; remove it and run the command again",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

fn compress_one(fs: &dyn Fs, tmp: &Path, path: &Path) -> Result<(), PublishFailed> {
    let stat = fs
        .entry_stat(path)
        .map_err(|err| PublishFailed::Compress(format!("{}: {err}", path.display())))?;
    if stat.kind != EntryKind::File {
        return Err(PublishFailed::Compress(format!(
            "{} is not a regular file; remove it and run the command again",
            path.display()
        )));
    }
    if stat.nlink != 1 {
        return Err(PublishFailed::Compress(format!(
            "{} has {} links; a store file has one, so this entry was left unpublished",
            path.display(),
            stat.nlink
        )));
    }
    if stat.compressed {
        return Ok(());
    }
    match fs.compress(path) {
        CompressOutcome::Compressed | CompressOutcome::Incompressible => Ok(()),
        CompressOutcome::SkippedHardLink => Err(PublishFailed::Compress(format!(
            "{}: skipped hard link",
            path.display()
        ))),
        CompressOutcome::Failed(why) => Err(after_failed(fs, tmp, path, &why)),
    }
}

fn after_failed(fs: &dyn Fs, tmp: &Path, path: &Path, why: &str) -> PublishFailed {
    let size = match fs::symlink_metadata(path) {
        Ok(meta) => meta.len(),
        Err(err) => {
            return PublishFailed::Compress(format!("{}: {why} ({err})", path.display()));
        }
    };
    match fs.volume(tmp) {
        Ok(info) if info.available_bytes < size => {
            PublishFailed::DiskFull(format!("{}: {why}", path.display()))
        }
        Ok(_) => PublishFailed::Compress(format!("{}: {why}", path.display())),
        Err(err) => PublishFailed::Compress(format!("{}: {why} ({err})", path.display())),
    }
}

fn publish(state: &State, tmp: &Path, row: &Row) -> Result<Published, PublishFailed> {
    let dest = entry_dir(state, row);
    let Some(parent) = dest.parent() else {
        return Err(PublishFailed::Compress(format!(
            "{} has no parent",
            dest.display()
        )));
    };
    fs::create_dir_all(parent)
        .map_err(|err| PublishFailed::Compress(format!("{}: {err}", parent.display())))?;
    match fs::rename(tmp, &dest) {
        Ok(()) => Ok(Published::Added),
        Err(err) if dest_present(&err) => {
            discard_tmp(tmp)?;
            Ok(Published::Held)
        }
        Err(err) => Err(PublishFailed::Compress(format!(
            "{}: {err}",
            dest.display()
        ))),
    }
}

fn dest_present(err: &io::Error) -> bool {
    err.raw_os_error() == Some(libc::EEXIST)
        || err.raw_os_error() == Some(libc::ENOTEMPTY)
        || err.kind() == io::ErrorKind::AlreadyExists
        || err.kind() == io::ErrorKind::DirectoryNotEmpty
}

fn discard_tmp(tmp: &Path) -> Result<(), PublishFailed> {
    match fs::remove_dir_all(tmp) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(PublishFailed::Compress(format!("{}: {err}", tmp.display()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{CompressOutcome, Fs, Hooked, VolumeInfo};
    use crate::state::{State, StateError};
    use std::collections::BTreeMap;
    use std::os::macos::fs::MetadataExt as _;
    use std::os::unix::io::AsRawFd;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, SystemTime};

    fn scratch_state(label: &str) -> (State, Box<dyn crate::platform::Fs>) {
        let dir = PathBuf::from(format!(
            "/private/tmp/hive-it-store-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let state = State::open_at(dir).unwrap();
        let fs = crate::platform::current()
            .expect("store tests run on macOS")
            .fs;
        (state, fs)
    }

    fn sample_row() -> Row {
        Row {
            toolchain_value: 7,
            name_meta: "d".into(),
            fingerprint_hash: "0123456789abcdef".into(),
            kind: "lib".into(),
            rel_profile: PathBuf::from("debug"),
            size: 4112,
            filled_at: 1_700_000_000,
            last_live_at: 1_700_000_000,
            repos: vec![PathBuf::from("/private/tmp/hive-it-repo")],
            registry_deps: Vec::new(),
        }
    }

    fn three_file_entry(state: &State, fs: &dyn crate::platform::Fs) -> (PathBuf, Row) {
        let row = sample_row();
        let key = format!("{}-{}", row.name_meta, row.fingerprint_hash);
        let tmp = tmp_dir(state, &key);
        let files = [
            (
                tmp.join("debug/deps/libd-0123456789abcdef.rlib"),
                vec![0u8; 4096],
            ),
            (
                tmp.join("debug/.fingerprint/d-0123456789abcdef/lib-d"),
                vec![0u8; 16],
            ),
            (
                tmp.join("debug/.fingerprint/d-0123456789abcdef/invoked.timestamp"),
                Vec::new(),
            ),
        ];
        for (path, bytes) in files {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs.write_file(&path, &bytes).unwrap();
        }
        (tmp, row)
    }

    fn rlib(entry: &Path) -> PathBuf {
        entry.join("debug/deps/libd-0123456789abcdef.rlib")
    }

    fn volume(available_bytes: u64) -> VolumeInfo {
        VolumeInfo {
            fs_type: "apfs".into(),
            container: "disk1".into(),
            mount: PathBuf::from("/"),
            available_bytes,
        }
    }

    fn finish(state: &State) {
        let _ = std::fs::remove_dir_all(state.root());
    }

    #[test]
    fn a_stop_at_any_step_leaves_nothing_available_and_a_later_attempt_finishes() {
        // Polls come before each of the three files and before the rename. A free state lock
        // does not poll, so the state "renamed, no row" is its own test below.
        for stop_after in 0..=3 {
            let (state, fs) = scratch_state(&format!("stop-{stop_after}"));
            let (tmp, row) = three_file_entry(&state, &*fs);
            let polls = AtomicUsize::new(0);
            let cancel = || polls.fetch_add(1, Ordering::SeqCst) >= stop_after;
            let published = compress_and_publish(&*fs, &state, &tmp, &row, &cancel);
            let err = published.unwrap_err();
            assert!(
                matches!(err, PublishFailed::Stopped(_)),
                "stop_after {stop_after}: {err}"
            );
            let text = err.to_string();
            assert!(
                text.contains(&tmp.display().to_string()),
                "stop_after {stop_after}: {text}"
            );
            assert!(read(&state).unwrap().units.is_empty());
            assert!(tmp.is_dir(), "a stopped publish leaves its temp directory");
            let (tmp, row) = three_file_entry(&state, &*fs);
            compress_and_publish(&*fs, &state, &tmp, &row, &|| false).unwrap();
            merge(&state, vec![row.clone()], Default::default(), &|| false).unwrap();
            assert_eq!(read(&state).unwrap().units.len(), 1);
            let entry = entry_dir(&state, &row);
            assert!(fs.entry_stat(&rlib(&entry)).unwrap().compressed);
            assert!(
                !fs.entry_stat(&entry.join("debug/.fingerprint/d-0123456789abcdef/lib-d"))
                    .unwrap()
                    .compressed
            );
            assert!(
                !fs.entry_stat(
                    &entry.join("debug/.fingerprint/d-0123456789abcdef/invoked.timestamp")
                )
                .unwrap()
                .compressed
            );
            finish(&state);
        }
    }

    #[test]
    fn a_directory_with_no_row_is_adopted_by_merge_without_cloning() {
        let (state, fs) = scratch_state("adopt");
        let (tmp, row) = three_file_entry(&state, &*fs);
        let dest = entry_dir(&state, &row);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::rename(&tmp, &dest).unwrap();
        let marker = rlib(&dest);
        let ino = fs.inode(&marker).unwrap();
        assert!(read(&state).unwrap().units.is_empty());
        merge(&state, vec![row.clone()], Default::default(), &|| false).unwrap();
        let got = read(&state).unwrap();
        assert_eq!(got.units.len(), 1);
        assert_eq!(got.units[0], row);
        assert_eq!(fs.inode(&marker).unwrap(), ino);
        assert!(!tmp.exists());
        finish(&state);
    }

    #[test]
    fn a_row_whose_directory_is_gone_is_not_read_and_is_pruned_at_the_next_merge() {
        let (state, fs) = scratch_state("prune");
        let (tmp, row) = three_file_entry(&state, &*fs);
        compress_and_publish(&*fs, &state, &tmp, &row, &|| false).unwrap();
        merge(&state, vec![row.clone()], Default::default(), &|| false).unwrap();
        assert_eq!(read(&state).unwrap().units.len(), 1);
        std::fs::remove_dir_all(entry_dir(&state, &row)).unwrap();
        assert!(read(&state).unwrap().units.is_empty());
        let raw = std::fs::read_to_string(state.root().join(STORE_FILE)).unwrap();
        assert!(raw.contains(&row.fingerprint_hash));
        merge(&state, Vec::new(), Default::default(), &|| false).unwrap();
        let raw = std::fs::read_to_string(state.root().join(STORE_FILE)).unwrap();
        assert!(!raw.contains(&row.fingerprint_hash));
        assert!(read(&state).unwrap().units.is_empty());
        finish(&state);
    }

    #[test]
    fn a_rename_that_finds_the_entry_present_counts_as_held_and_discards_its_copy() {
        let (state, fs) = scratch_state("held");
        let row = sample_row();
        let dest = entry_dir(&state, &row);
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("keep"), b"original").unwrap();
        let (tmp, row) = three_file_entry(&state, &*fs);
        let published = compress_and_publish(&*fs, &state, &tmp, &row, &|| false).unwrap();
        assert!(matches!(published, Published::Held));
        assert!(!tmp.exists(), "the losing copy is discarded");
        assert_eq!(std::fs::read(dest.join("keep")).unwrap(), b"original");
        assert!(!dest.join("debug").exists());
        finish(&state);
    }

    #[test]
    fn merge_adds_a_repo_and_moves_last_live_at_for_a_held_key() {
        let (state, fs) = scratch_state("repos");
        let (tmp, mut row) = three_file_entry(&state, &*fs);
        row.repos = vec![PathBuf::from("/repos/a")];
        row.filled_at = 10;
        row.last_live_at = 10;
        row.size = 4112;
        compress_and_publish(&*fs, &state, &tmp, &row, &|| false).unwrap();
        let mut table = BTreeMap::new();
        table.insert("rustc 1.97.0\n".to_string(), row.toolchain_value);
        merge(&state, vec![row.clone()], table, &|| false).unwrap();

        let mut again = row.clone();
        again.repos = vec![PathBuf::from("/repos/b")];
        again.last_live_at = 25;
        again.filled_at = 99;
        again.size = 1;
        merge(&state, vec![again], BTreeMap::new(), &|| false).unwrap();

        let got = read(&state).unwrap();
        assert_eq!(got.units.len(), 1);
        assert_eq!(
            got.units[0].repos,
            vec![PathBuf::from("/repos/a"), PathBuf::from("/repos/b")]
        );
        assert_eq!(got.units[0].last_live_at, 25);
        assert_eq!(got.units[0].filled_at, 10);
        assert_eq!(got.units[0].size, 4112);
        assert_eq!(
            got.toolchains.get("rustc 1.97.0\n"),
            Some(&row.toolchain_value)
        );

        let mut earlier = row.clone();
        earlier.last_live_at = 4;
        earlier.repos = vec![PathBuf::from("/repos/a")];
        merge(&state, vec![earlier], BTreeMap::new(), &|| false).unwrap();
        let got = read(&state).unwrap();
        assert_eq!(got.units[0].last_live_at, 25);
        assert_eq!(got.units[0].repos.len(), 2);
        finish(&state);
    }

    #[test]
    fn a_compress_failure_other_than_too_small_leaves_the_entry_unavailable() {
        let (state, inner) = scratch_state("compress-fail");
        let (tmp, row) = three_file_entry(&state, &*inner);
        let fs = Hooked {
            inner,
            compress: Some(Box::new(|path| {
                if path.extension().and_then(|ext| ext.to_str()) == Some("rlib") {
                    CompressOutcome::Failed("compressor broke".into())
                } else {
                    CompressOutcome::Incompressible
                }
            })),
            volume: Some(Box::new(|_| Ok(volume(4096)))),
            sharing: None,
            entry_stat: None,
        };
        let err = compress_and_publish(&fs, &state, &tmp, &row, &|| false).unwrap_err();
        assert!(
            matches!(err, PublishFailed::Compress(ref why) if why.contains("libd-0123456789abcdef.rlib") && why.contains("compressor broke")),
            "{err}"
        );
        assert!(!entry_dir(&state, &row).exists());
        assert!(read(&state).unwrap().units.is_empty());
        assert!(tmp.is_dir());
        finish(&state);
    }

    #[test]
    fn a_failure_with_little_free_space_is_disk_full() {
        let (state, inner) = scratch_state("disk-full");
        let (tmp, row) = three_file_entry(&state, &*inner);
        let fs = Hooked {
            inner,
            compress: Some(Box::new(|path| {
                if path.extension().and_then(|ext| ext.to_str()) == Some("rlib") {
                    CompressOutcome::Failed("no space".into())
                } else {
                    CompressOutcome::Incompressible
                }
            })),
            volume: Some(Box::new(|_| Ok(volume(4095)))),
            sharing: None,
            entry_stat: None,
        };
        let err = compress_and_publish(&fs, &state, &tmp, &row, &|| false).unwrap_err();
        let PublishFailed::DiskFull(why) = &err else {
            panic!("{err}");
        };
        assert!(why.contains("libd-0123456789abcdef.rlib"), "{err}");
        let sentence = format!("the disk is full ({why}); free space and run the command again");
        assert_eq!(err.to_string(), sentence);
        assert_eq!(
            crate::seed::Error::disk_full(why.clone()).to_string(),
            sentence
        );
        assert!(!entry_dir(&state, &row).exists());
        assert!(read(&state).unwrap().units.is_empty());
        finish(&state);
    }

    #[test]
    fn a_file_already_compressed_is_finished() {
        let (state, fs) = scratch_state("already");
        let (tmp, row) = three_file_entry(&state, &*fs);
        let source = rlib(&tmp);
        assert!(matches!(fs.compress(&source), CompressOutcome::Compressed));
        let fs = Hooked {
            inner: fs,
            compress: Some(Box::new(|path| {
                let flags = std::fs::symlink_metadata(path).unwrap().st_flags();
                if flags & libc::UF_COMPRESSED != 0 {
                    CompressOutcome::Failed(format!("compressed again: {}", path.display()))
                } else {
                    CompressOutcome::Incompressible
                }
            })),
            volume: None,
            sharing: None,
            entry_stat: None,
        };
        let published = compress_and_publish(&fs, &state, &tmp, &row, &|| false).unwrap();
        assert!(matches!(published, Published::Added));
        assert!(
            fs.entry_stat(&rlib(&entry_dir(&state, &row)))
                .unwrap()
                .compressed
        );
        finish(&state);
    }

    #[test]
    fn a_store_over_its_cap_reports_the_failure_and_leaves_the_file_unchanged() {
        let (state, fs) = scratch_state("cap");
        let (tmp, row) = three_file_entry(&state, &*fs);
        compress_and_publish(&*fs, &state, &tmp, &row, &|| false).unwrap();
        merge(&state, vec![row], BTreeMap::new(), &|| false).unwrap();
        let path = state.root().join(STORE_FILE);
        let before = std::fs::read(&path).unwrap();

        let mut fat = sample_row();
        fat.name_meta = "fat".into();
        fat.fingerprint_hash = "aaaaaaaaaaaaaaaa".into();
        fat.registry_deps = vec!["a".repeat(MAX_STORE_BYTES as usize)];
        std::fs::create_dir_all(entry_dir(&state, &fat)).unwrap();
        let err = merge(&state, vec![fat], BTreeMap::new(), &|| false).unwrap_err();
        assert!(
            matches!(err, StateError::Oversized { max, .. } if max == MAX_STORE_BYTES),
            "{err}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        finish(&state);
    }

    #[test]
    fn a_hard_linked_file_leaves_the_entry_unavailable() {
        let (state, fs) = scratch_state("link");
        let (tmp, row) = three_file_entry(&state, &*fs);
        let linked = tmp.join("debug/deps/also.rlib");
        fs.hardlink(&rlib(&tmp), &linked).unwrap();
        let err = compress_and_publish(&*fs, &state, &tmp, &row, &|| false).unwrap_err();
        assert!(
            matches!(err, PublishFailed::Compress(ref why) if why.contains("libd-0123456789abcdef.rlib") || why.contains("also.rlib")),
            "{err}"
        );
        assert!(!entry_dir(&state, &row).exists());
        finish(&state);
    }

    #[test]
    fn sweep_tmp_removes_an_entry_older_than_a_day_and_keeps_a_newer_one() {
        let (state, _) = scratch_state("sweep");
        let old = tmp_dir(&state, "old");
        let boundary = tmp_dir(&state, "boundary");
        let young = tmp_dir(&state, "young");
        for dir in [&old, &boundary, &young] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("f"), b"x").unwrap();
        }
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let day = Duration::from_secs(86_400);
        set_mtime(&old, now - day - Duration::from_secs(2));
        set_mtime(&boundary, now - day);
        set_mtime(&young, now - Duration::from_secs(30));
        sweep_tmp(&state, now).unwrap();
        assert!(!old.exists());
        assert!(boundary.is_dir());
        assert!(young.is_dir());
        finish(&state);
    }

    #[test]
    fn sweep_tmp_reports_a_listing_past_its_cap_and_leaves_the_rest() {
        let (state, _) = scratch_state("sweep-cap");
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let day = Duration::from_secs(86_400);
        let tmp = store_dir(&state).join(".tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let mut planted = Vec::new();
        for name in ["one", "two", "three", "four"] {
            let path = tmp.join(name);
            std::fs::create_dir(&path).unwrap();
            set_mtime(&path, now - day - Duration::from_secs(2));
            planted.push(path);
        }
        // `sweep_tmp` calls this with `CRATE_DIR_CAP`. A smaller cap still runs that check.
        let err = sweep_tmp_bounded(&state, now, 3)
            .expect_err("a listing past the cap is not a finished sweep");
        assert!(
            err.contains(&format!("{} has more than 3 entries", tmp.display())),
            "{err}"
        );
        assert!(
            err.contains("remove temp entries until fewer remain and run the command again"),
            "{err}"
        );
        assert_eq!(
            planted.iter().filter(|path| path.exists()).count(),
            1,
            "the entry past the cap stays"
        );
        finish(&state);
    }

    #[test]
    fn sweep_tmp_reports_a_removal_that_fails() {
        let (state, _) = scratch_state("sweep-remove");
        let old = tmp_dir(&state, "stuck");
        std::fs::create_dir_all(&old).unwrap();
        let file = old.join("f");
        std::fs::write(&file, b"x").unwrap();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let day = Duration::from_secs(86_400);
        set_mtime(&old, now - day - Duration::from_secs(2));
        {
            let _clear = ClearFlag(file.clone());
            assert!(set_user_immutable(&file, true));
            let err = sweep_tmp(&state, now).expect_err("a failed removal names the path");
            assert!(err.contains(&old.display().to_string()), "{err}");
            assert!(
                err.contains("remove that temp entry and run the command again"),
                "{err}"
            );
            assert!(file.exists(), "the entry that could not be removed stays");
        }
        finish(&state);
    }

    #[test]
    fn sweep_tmp_reports_when_tmp_is_not_a_directory() {
        let (state, _) = scratch_state("sweep-not-dir");
        let dir = store_dir(&state).join(".tmp");
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        std::fs::write(&dir, b"not a directory").unwrap();
        let err = sweep_tmp(&state, SystemTime::UNIX_EPOCH)
            .expect_err("a temp path that is not a directory is not a finished sweep");
        assert!(err.contains(&format!("{}:", dir.display())), "{err}");
        assert!(
            err.contains("remove the temp directory and run the command again"),
            "{err}"
        );
        finish(&state);
    }

    #[test]
    fn sweep_tmp_accepts_a_missing_tmp_directory() {
        let (state, _) = scratch_state("sweep-missing");
        sweep_tmp(&state, SystemTime::UNIX_EPOCH).unwrap();
        assert!(!store_dir(&state).join(".tmp").exists());
        finish(&state);
    }

    #[test]
    fn sweep_tmp_reports_a_stat_that_fails() {
        let prefix = PathBuf::from(format!(
            "/private/tmp/hive-it-store-sweep-stat-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&prefix);
        let mut root = prefix.clone();
        while root.join("store/.tmp").display().to_string().len() < 860 {
            root.push("d".repeat(40));
        }
        let state = State::open_at(root).unwrap();
        let tmp = store_dir(&state).join(".tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let name = "n".repeat(200);
        let child = tmp.join(&name);
        assert!(
            child.display().to_string().len() > 1024,
            "the child path has to be too long to stat"
        );
        {
            let _clear = UnlinkLong {
                dir: tmp.clone(),
                name: name.clone(),
            };
            create_long_name(&tmp, &name);
            let err = sweep_tmp(&state, SystemTime::UNIX_EPOCH)
                .expect_err("a failed stat names the path");
            assert!(err.contains(&child.display().to_string()), "{err}");
            assert!(
                err.contains("remove that temp entry and run the command again"),
                "{err}"
            );
        }
        finish(&state);
        let _ = std::fs::remove_dir_all(&prefix);
    }

    struct UnlinkLong {
        dir: PathBuf,
        name: String,
    }

    impl Drop for UnlinkLong {
        fn drop(&mut self) {
            unlink_long_name(&self.dir, &self.name);
        }
    }

    fn create_long_name(dir: &Path, name: &str) {
        let file = std::fs::File::open(dir).unwrap();
        let c_name = std::ffi::CString::new(name).unwrap();
        // SAFETY: `file` is an open directory and `c_name` is one NUL-terminated
        // component, so `openat` creates that name only inside this directory.
        let fd = unsafe {
            libc::openat(
                file.as_raw_fd(),
                c_name.as_ptr(),
                libc::O_CREAT | libc::O_WRONLY,
                0o644,
            )
        };
        assert!(fd >= 0, "{}", std::io::Error::last_os_error());
        // SAFETY: `fd` is the descriptor just returned by `openat` and is not used again.
        unsafe { libc::close(fd) };
    }

    fn unlink_long_name(dir: &Path, name: &str) {
        let Ok(file) = std::fs::File::open(dir) else {
            return;
        };
        let Ok(c_name) = std::ffi::CString::new(name) else {
            return;
        };
        // SAFETY: `file` is an open directory and `c_name` is the component created above.
        unsafe {
            libc::unlinkat(file.as_raw_fd(), c_name.as_ptr(), 0);
        }
    }

    struct ClearFlag(PathBuf);

    impl Drop for ClearFlag {
        fn drop(&mut self) {
            let _ = set_user_immutable(&self.0, false);
        }
    }

    fn set_user_immutable(path: &Path, on: bool) -> bool {
        let flag = if on { "uchg" } else { "nouchg" };
        std::process::Command::new("chflags")
            .arg(flag)
            .arg(path)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn set_mtime(path: &Path, t: SystemTime) {
        std::fs::File::open(path).unwrap().set_modified(t).unwrap();
    }

    fn store_key(value: u64, meta: &str, hash: &str) -> StoreKey {
        StoreKey {
            toolchain_value: value,
            name_meta: meta.into(),
            fingerprint_hash: hash.into(),
        }
    }

    fn row_for(key: &StoreKey, last_live_at: u64) -> Row {
        Row {
            toolchain_value: key.toolchain_value,
            name_meta: key.name_meta.clone(),
            fingerprint_hash: key.fingerprint_hash.clone(),
            kind: "lib".into(),
            rel_profile: PathBuf::from("debug"),
            size: 1,
            filled_at: last_live_at,
            last_live_at,
            repos: Vec::new(),
            registry_deps: Vec::new(),
        }
    }

    fn plant_entry(state: &State, key: &StoreKey) -> PathBuf {
        let dir = key_dir(state, key);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f"), b"x").unwrap();
        dir
    }

    fn write_store(state: &State, file: &StoreFile) {
        state
            .locked(|tx| tx.write_bounded(STORE_FILE, file, MAX_STORE_BYTES))
            .unwrap();
    }

    fn rename_with_out<'a>(
        state: &'a State,
        seq: &'a std::sync::atomic::AtomicUsize,
    ) -> impl FnMut(&StoreKey) -> io::Result<Option<PathBuf>> + 'a {
        move |key| {
            let n = seq.fetch_add(1, Ordering::SeqCst);
            let to = evict_dir(state, std::process::id(), n);
            rename_out(state, key, &to)?;
            Ok(Some(to))
        }
    }

    #[test]
    fn inventory_lists_rows_rowless_dirs_and_tmp_bytes_and_skips_other_names() {
        let (state, fs) = scratch_state("inv");
        let with_row = store_key(7, "d", "0123456789abcdef");
        let rowless = store_key(7, "orphan", "fedcba9876543210");
        let other_value = store_key(9, "e", "aaaaaaaaaaaaaaaa");
        plant_entry(&state, &with_row);
        plant_entry(&state, &rowless);
        plant_entry(&state, &other_value);
        std::fs::write(
            tag_path(&state),
            b"Signature: 8a477f597d28d172789f06886806bc55\n",
        )
        .unwrap();
        std::fs::create_dir_all(store_dir(&state).join("not-a-value")).unwrap();
        std::fs::write(store_dir(&state).join("7").join("skip-me"), b"x").unwrap();
        std::fs::create_dir_all(store_dir(&state).join("7").join("badname")).unwrap();
        let tmp = store_dir(&state).join(".tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("staging"), vec![0u8; 8192]).unwrap();
        let mut toolchains = BTreeMap::new();
        toolchains.insert("rustc 1.97.0\n".into(), 7u64);
        write_store(
            &state,
            &StoreFile {
                units: vec![row_for(&with_row, 10), row_for(&other_value, 11)],
                toolchains,
            },
        );

        let got = inventory(&*fs, &state).unwrap();
        assert_eq!(got.rows.len(), 2);
        assert!(got.rows.iter().any(|r| {
            r.toolchain_value == with_row.toolchain_value
                && r.name_meta == with_row.name_meta
                && r.fingerprint_hash == with_row.fingerprint_hash
        }));
        assert!(got.rows.iter().any(|r| {
            r.toolchain_value == other_value.toolchain_value
                && r.name_meta == other_value.name_meta
                && r.fingerprint_hash == other_value.fingerprint_hash
        }));
        assert_eq!(got.rowless.len(), 1);
        assert_eq!(got.rowless[0].0, rowless);
        assert_eq!(
            got.rowless[0].1,
            store_dir(&state).join("7").join("orphan-fedcba9876543210")
        );
        assert!(
            got.tmp_bytes >= 8192,
            "tmp staging file contributes allocated bytes, got {}",
            got.tmp_bytes
        );
        finish(&state);
    }

    #[test]
    fn commit_drops_a_renamed_row_and_raises_a_refreshed_one() {
        let (state, _) = scratch_state("commit-drop-raise");
        let drop_key = store_key(3, "old", "1111111111111111");
        let keep_key = store_key(3, "live", "2222222222222222");
        plant_entry(&state, &drop_key);
        plant_entry(&state, &keep_key);
        write_store(
            &state,
            &StoreFile {
                units: vec![row_for(&drop_key, 100), row_for(&keep_key, 100)],
                toolchains: BTreeMap::from([("rustc\n".into(), 3u64)]),
            },
        );
        let seq = AtomicUsize::new(0);
        let plan = CommitPlan {
            candidates: vec![(drop_key.clone(), Some(100))],
            refreshed: vec![keep_key.clone()],
            now: 500,
        };
        let committed =
            commit_eviction(&state, &plan, &|| false, rename_with_out(&state, &seq)).unwrap();
        assert_eq!(committed.renamed.len(), 1);
        assert_eq!(committed.renamed[0].0, drop_key);
        assert!(!entry_path(&state, &drop_key).exists());
        assert!(committed.renamed[0].1.is_dir());
        assert_eq!(committed.kept_changed, 0);
        let got = read(&state).unwrap();
        assert_eq!(got.units.len(), 1);
        assert_eq!(got.units[0].name_meta, "live");
        assert_eq!(got.units[0].last_live_at, 500);
        assert!(entry_path(&state, &keep_key).is_dir());
        finish(&state);
    }

    fn entry_path(state: &State, key: &StoreKey) -> PathBuf {
        key_dir(state, key)
    }

    #[test]
    fn commit_keeps_a_candidate_whose_last_live_at_rose_since_the_plan() {
        let (state, _) = scratch_state("commit-rose");
        let key = store_key(4, "fresh", "3333333333333333");
        plant_entry(&state, &key);
        write_store(
            &state,
            &StoreFile {
                units: vec![row_for(&key, 200)],
                toolchains: BTreeMap::new(),
            },
        );
        let plan = CommitPlan {
            candidates: vec![(key.clone(), Some(100))],
            refreshed: Vec::new(),
            now: 500,
        };
        let seq = AtomicUsize::new(0);
        let committed =
            commit_eviction(&state, &plan, &|| false, rename_with_out(&state, &seq)).unwrap();
        assert!(committed.renamed.is_empty());
        assert_eq!(committed.kept_changed, 1);
        assert!(entry_path(&state, &key).is_dir());
        let got = read(&state).unwrap();
        assert_eq!(got.units.len(), 1);
        assert_eq!(got.units[0].last_live_at, 200);
        finish(&state);
    }

    #[test]
    fn commit_keeps_a_rowless_candidate_that_gained_a_row() {
        let (state, _) = scratch_state("commit-gained");
        let key = store_key(5, "adopted", "4444444444444444");
        plant_entry(&state, &key);
        write_store(
            &state,
            &StoreFile {
                units: vec![row_for(&key, 50)],
                toolchains: BTreeMap::new(),
            },
        );
        let plan = CommitPlan {
            candidates: vec![(key.clone(), None)],
            refreshed: Vec::new(),
            now: 500,
        };
        let seq = AtomicUsize::new(0);
        let committed =
            commit_eviction(&state, &plan, &|| false, rename_with_out(&state, &seq)).unwrap();
        assert!(committed.renamed.is_empty());
        assert_eq!(committed.kept_changed, 1);
        assert!(entry_path(&state, &key).is_dir());
        assert_eq!(read(&state).unwrap().units.len(), 1);
        finish(&state);
    }

    #[test]
    fn commit_drops_a_value_text_only_when_no_row_and_no_dir_of_it_remain() {
        let (state, _) = scratch_state("commit-texts");
        let alone = store_key(1, "gone", "5555555555555555");
        let sibling_drop = store_key(2, "drop", "6666666666666666");
        let sibling_keep = store_key(2, "keep", "7777777777777777");
        let rowless_keep = store_key(8, "rowless", "8888888888888888");
        plant_entry(&state, &alone);
        plant_entry(&state, &sibling_drop);
        plant_entry(&state, &sibling_keep);
        plant_entry(&state, &rowless_keep);
        write_store(
            &state,
            &StoreFile {
                units: vec![
                    row_for(&alone, 10),
                    row_for(&sibling_drop, 10),
                    row_for(&sibling_keep, 10),
                ],
                toolchains: BTreeMap::from([
                    ("alone\n".into(), 1u64),
                    ("sibling\n".into(), 2u64),
                    ("rowless-value\n".into(), 8u64),
                ]),
            },
        );
        let plan = CommitPlan {
            candidates: vec![(alone.clone(), Some(10)), (sibling_drop.clone(), Some(10))],
            refreshed: Vec::new(),
            now: 500,
        };
        let seq = AtomicUsize::new(0);
        let committed =
            commit_eviction(&state, &plan, &|| false, rename_with_out(&state, &seq)).unwrap();
        assert_eq!(committed.renamed.len(), 2);
        assert_eq!(committed.dropped_values, vec![1]);
        let got = read(&state).unwrap();
        assert_eq!(got.units.len(), 1);
        assert_eq!(got.units[0].name_meta, "keep");
        assert!(!got.toolchains.contains_key("alone\n"));
        assert_eq!(got.toolchains.get("sibling\n"), Some(&2));
        assert_eq!(got.toolchains.get("rowless-value\n"), Some(&8));
        assert!(entry_path(&state, &rowless_keep).is_dir());
        finish(&state);
    }

    #[test]
    fn commit_waiting_on_a_held_state_lock_stops_when_cancelled() {
        use std::os::fd::AsRawFd;
        let (state, _) = scratch_state("commit-cancel");
        let key = store_key(6, "a", "9999999999999999");
        plant_entry(&state, &key);
        write_store(
            &state,
            &StoreFile {
                units: vec![row_for(&key, 10)],
                toolchains: BTreeMap::new(),
            },
        );
        let before = std::fs::read(state.root().join(STORE_FILE)).unwrap();
        // Another holder: flock conflicts between two open file descriptions.
        let other = std::fs::File::open(state.root().join(".lock")).unwrap();
        // SAFETY: `other` owns a valid descriptor for the whole test.
        assert_eq!(
            unsafe { libc::flock(other.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        let plan = CommitPlan {
            candidates: vec![(key.clone(), Some(10))],
            refreshed: vec![],
            now: 700,
        };
        let seq = AtomicUsize::new(0);
        let err =
            commit_eviction(&state, &plan, &|| true, rename_with_out(&state, &seq)).unwrap_err();
        assert!(matches!(err, StateError::Cancelled), "{err:?}");
        assert!(entry_path(&state, &key).is_dir());
        assert_eq!(
            std::fs::read(state.root().join(STORE_FILE)).unwrap(),
            before
        );
        drop(other);
    }

    #[test]
    fn commit_stops_renaming_when_rename_returns_none_and_still_writes() {
        let (state, _) = scratch_state("commit-stop");
        let first = store_key(6, "a", "9999999999999999");
        let second = store_key(6, "b", "aaaaaaaaaaaaaaaa");
        let held = store_key(6, "c", "bbbbbbbbbbbbbbbb");
        plant_entry(&state, &first);
        plant_entry(&state, &second);
        plant_entry(&state, &held);
        write_store(
            &state,
            &StoreFile {
                units: vec![
                    row_for(&first, 10),
                    row_for(&second, 10),
                    row_for(&held, 10),
                ],
                toolchains: BTreeMap::new(),
            },
        );
        let plan = CommitPlan {
            candidates: vec![(first.clone(), Some(10)), (second.clone(), Some(10))],
            refreshed: vec![held.clone()],
            now: 700,
        };
        let calls = AtomicUsize::new(0);
        let committed = commit_eviction(&state, &plan, &|| false, |key| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            if n >= 1 {
                return Ok(None);
            }
            let to = evict_dir(&state, std::process::id(), n);
            rename_out(&state, key, &to)?;
            Ok(Some(to))
        })
        .unwrap();
        assert_eq!(committed.renamed.len(), 1);
        assert_eq!(committed.renamed[0].0, first);
        assert!(!entry_path(&state, &first).exists());
        assert!(entry_path(&state, &second).is_dir());
        let got = read(&state).unwrap();
        assert_eq!(got.units.len(), 2);
        let held_row = got.units.iter().find(|r| r.name_meta == "c").unwrap();
        assert_eq!(held_row.last_live_at, 700);
        assert!(got.units.iter().any(|r| r.name_meta == "b"));
        finish(&state);
    }

    #[test]
    fn a_failed_rename_keeps_that_entry_and_its_row() {
        let (state, _) = scratch_state("commit-fail");
        let key = store_key(11, "stuck", "cccccccccccccccc");
        plant_entry(&state, &key);
        write_store(
            &state,
            &StoreFile {
                units: vec![row_for(&key, 10)],
                toolchains: BTreeMap::new(),
            },
        );
        let plan = CommitPlan {
            candidates: vec![(key.clone(), Some(10))],
            refreshed: Vec::new(),
            now: 500,
        };
        let committed = commit_eviction(&state, &plan, &|| false, |_| {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "nope"))
        })
        .unwrap();
        assert!(committed.renamed.is_empty());
        assert_eq!(committed.kept_changed, 0);
        assert_eq!(committed.rename_failures.len(), 1, "{committed:?}");
        assert_eq!(committed.rename_failures[0].0, key);
        assert!(
            committed.rename_failures[0].1.contains("nope"),
            "{committed:?}"
        );
        assert!(entry_path(&state, &key).is_dir());
        assert_eq!(read(&state).unwrap().units.len(), 1);
        finish(&state);
    }
}
