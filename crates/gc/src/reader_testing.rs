//! Read-only byte segments and timestamp metadata for compiler-reader tests.

use cargo_hive_platform::{
    CompressOutcome, EntryKind, EntryStat, Fs, LockHolder, LockKind, LockState, Sharing, TempDir,
    TryLock, VolumeInfo,
};
use std::{
    cell::Cell,
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

pub(crate) struct ReaderFs {
    pub segments: Vec<(u64, Vec<u8>)>,
    pub times: BTreeMap<PathBuf, Result<SystemTime, io::ErrorKind>>,
    pub read_error: Option<io::ErrorKind>,
    pub chunk: usize,
    pub reads: Cell<usize>,
}

impl ReaderFs {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            segments: vec![(0, bytes)],
            times: BTreeMap::new(),
            read_error: None,
            chunk: usize::MAX,
            reads: Cell::new(0),
        }
    }
}

fn unused() -> io::Error {
    io::ErrorKind::Unsupported.into()
}

impl Fs for ReaderFs {
    fn read(&self, _: &Path) -> io::Result<Vec<u8>> {
        panic!("reader attempted an unbounded read")
    }
    fn read_at(&self, _: &Path, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        self.reads.set(self.reads.get() + 1);
        assert!(
            self.reads.get() <= 64,
            "reader exceeded bounded request budget"
        );
        if let Some(kind) = self.read_error {
            return Err(kind.into());
        }
        for (base, bytes) in &self.segments {
            let Some(start) = offset
                .checked_sub(*base)
                .and_then(|n| usize::try_from(n).ok())
            else {
                continue;
            };
            if start >= bytes.len() {
                continue;
            }
            let n = len.min(self.chunk).min(bytes.len() - start);
            return Ok(bytes[start..start + n].to_vec());
        }
        Ok(Vec::new())
    }
    fn mtime(&self, path: &Path) -> io::Result<SystemTime> {
        self.times
            .get(path)
            .copied()
            .unwrap_or(Err(io::ErrorKind::NotFound))
            .map_err(io::Error::from)
    }
    fn temp_dir(&self, _: &Path, _: &str) -> io::Result<TempDir> {
        panic!("reader attempted mutation: temp_dir")
    }
    fn write_file(&self, _: &Path, _: &[u8]) -> io::Result<()> {
        panic!("reader attempted mutation: write_file")
    }
    fn hardlink(&self, _: &Path, _: &Path) -> io::Result<()> {
        panic!("reader attempted mutation: hardlink")
    }
    fn set_mtime(&self, _: &Path, _: SystemTime) -> io::Result<()> {
        panic!("reader attempted mutation: set_mtime")
    }
    fn clone_file(&self, _: &Path, _: &Path) -> io::Result<()> {
        panic!("reader attempted mutation: clone_file")
    }
    fn create_dir(&self, _: &Path) -> io::Result<()> {
        panic!("reader attempted mutation: create_dir")
    }
    fn remove_tree(&self, _: &Path) -> io::Result<()> {
        panic!("reader attempted mutation: remove_tree")
    }
    fn remove_file(&self, _: &Path) -> io::Result<()> {
        panic!("reader attempted mutation: remove_file")
    }
    fn inode(&self, _: &Path) -> io::Result<u64> {
        Err(unused())
    }
    fn sharing(&self, _: &Path) -> io::Result<Sharing> {
        Err(unused())
    }
    fn hold_lock(&self, _: LockKind, _: &Path) -> io::Result<LockHolder> {
        Err(unused())
    }
    fn try_lock_elsewhere(&self, _: LockKind, _: &Path) -> io::Result<LockState> {
        Err(unused())
    }
    fn entry_stat(&self, _: &Path) -> io::Result<EntryStat> {
        Err(unused())
    }
    fn read_dir_bounded(&self, _: &Path, _: usize) -> io::Result<Vec<(String, EntryKind)>> {
        Err(unused())
    }
    fn volume(&self, _: &Path) -> io::Result<VolumeInfo> {
        Err(unused())
    }
    fn compress(&self, _: &Path) -> CompressOutcome {
        panic!("reader attempted compression")
    }
    fn try_lock_exclusive(&self, _: &Path) -> TryLock {
        TryLock::Unopenable(unused())
    }
}
