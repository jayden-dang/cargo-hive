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

use crate::gc::CRATE_DIR_CAP;
use crate::platform::{CompressOutcome, EntryKind, Fs};
use crate::size::MAX_DEPTH;
use crate::state::{State, StateError};

pub const STORE_FILE: &str = "store.db";
pub const MAX_STORE_BYTES: u64 = 32 * 1024 * 1024;
const TMP_KEEP: Duration = Duration::from_secs(86_400);

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
    store_dir(state)
        .join(row.toolchain_value.to_string())
        .join(format!("{}-{}", row.name_meta, row.fingerprint_hash))
}

pub fn tag_path(state: &State) -> PathBuf {
    store_dir(state).join("CACHEDIR.TAG")
}

pub fn tmp_dir(state: &State, key: &str) -> PathBuf {
    store_dir(state)
        .join(".tmp")
        .join(format!("{}-{key}", std::process::id()))
}

pub fn sweep_tmp(state: &State, now: SystemTime) {
    let dir = store_dir(state).join(".tmp");
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    for (seen, entry) in entries.enumerate() {
        if seen >= CRATE_DIR_CAP {
            return;
        }
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age <= TMP_KEEP {
            continue;
        }
        if meta.file_type().is_dir() {
            let _ = fs::remove_dir_all(&path);
        } else {
            let _ = fs::remove_file(&path);
        }
    }
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
    Stopped,
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
            PublishFailed::DiskFull(why) => {
                write!(
                    f,
                    "the disk is full ({why}); free space and run the command again"
                )
            }
            PublishFailed::Stopped => {
                write!(
                    f,
                    "stopped while publishing an entry; run the command again"
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
            return Err(PublishFailed::Stopped);
        }
        compress_one(fs, tmp, path)?;
    }
    if cancel() {
        return Err(PublishFailed::Stopped);
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

fn same_key(left: &Row, right: &Row) -> bool {
    left.toolchain_value == right.toolchain_value
        && left.name_meta == right.name_meta
        && left.fingerprint_hash == right.fingerprint_hash
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
            assert!(
                matches!(published, Err(PublishFailed::Stopped)),
                "stop_after {stop_after}"
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
        assert!(
            matches!(err, PublishFailed::DiskFull(ref why) if why.contains("libd-0123456789abcdef.rlib")),
            "{err}"
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
        sweep_tmp(&state, now);
        assert!(!old.exists());
        assert!(boundary.is_dir());
        assert!(young.is_dir());
        finish(&state);
    }

    fn set_mtime(path: &Path, t: SystemTime) {
        std::fs::File::open(path).unwrap().set_modified(t).unwrap();
    }
}
