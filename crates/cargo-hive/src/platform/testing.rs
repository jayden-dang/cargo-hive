//! Test-only OS helpers: hooked `Fs`, env mutation, and raw openat/unlinkat/flock.

use std::ffi::{CStr, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::path::Path;
use std::time::SystemTime;

use super::{
    CompressOutcome, EntryKind, EntryStat, Fs, LockHolder, LockKind, LockState, Sharing, TempDir,
    TryLock, VolumeInfo,
};

type Hook<T> = Box<dyn Fn(&Path) -> T + Send + Sync>;

pub struct Hooked {
    pub inner: Box<dyn Fs>,
    pub compress: Option<Hook<CompressOutcome>>,
    pub volume: Option<Hook<io::Result<VolumeInfo>>>,
    pub sharing: Option<Hook<io::Result<Sharing>>>,
    pub entry_stat: Option<Hook<io::Result<EntryStat>>>,
}

impl Fs for Hooked {
    fn temp_dir(&self, parent: &Path, prefix: &str) -> io::Result<TempDir> {
        self.inner.temp_dir(parent, prefix)
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.inner.write_file(path, bytes)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.inner.read(path)
    }

    fn hardlink(&self, src: &Path, dst: &Path) -> io::Result<()> {
        self.inner.hardlink(src, dst)
    }

    fn set_mtime(&self, path: &Path, t: SystemTime) -> io::Result<()> {
        self.inner.set_mtime(path, t)
    }

    fn mtime(&self, path: &Path) -> io::Result<SystemTime> {
        self.inner.mtime(path)
    }

    fn inode(&self, path: &Path) -> io::Result<u64> {
        self.inner.inode(path)
    }

    fn clone_file(&self, src: &Path, dst: &Path) -> io::Result<()> {
        self.inner.clone_file(src, dst)
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        self.inner.create_dir(path)
    }

    fn sharing(&self, path: &Path) -> io::Result<Sharing> {
        match &self.sharing {
            Some(f) => f(path),
            None => self.inner.sharing(path),
        }
    }

    fn compress(&self, path: &Path) -> CompressOutcome {
        match &self.compress {
            Some(f) => f(path),
            None => self.inner.compress(path),
        }
    }

    fn hold_lock(&self, kind: LockKind, path: &Path) -> io::Result<LockHolder> {
        self.inner.hold_lock(kind, path)
    }

    fn try_lock_exclusive(&self, path: &Path) -> TryLock {
        self.inner.try_lock_exclusive(path)
    }

    fn try_lock_elsewhere(&self, kind: LockKind, path: &Path) -> io::Result<LockState> {
        self.inner.try_lock_elsewhere(kind, path)
    }

    fn entry_stat(&self, path: &Path) -> io::Result<EntryStat> {
        match &self.entry_stat {
            Some(f) => f(path),
            None => self.inner.entry_stat(path),
        }
    }

    fn read_dir_bounded(&self, dir: &Path, cap: usize) -> io::Result<Vec<(String, EntryKind)>> {
        self.inner.read_dir_bounded(dir, cap)
    }

    fn remove_tree(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_tree(path)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_file(path)
    }

    fn volume(&self, path: &Path) -> io::Result<VolumeInfo> {
        match &self.volume {
            Some(f) => f(path),
            None => self.inner.volume(path),
        }
    }
}

/// Sets a process environment variable. Caller restores the previous value.
pub fn set_env(key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) {
    // SAFETY: caller restores the previous value when the test finishes.
    unsafe { std::env::set_var(key, value) }
}

/// Removes a process environment variable. Caller restores any prior value.
pub fn remove_env(key: impl AsRef<OsStr>) {
    // SAFETY: caller restores the previous value when the test finishes.
    unsafe { std::env::remove_var(key) }
}

/// Creates `name` inside the open directory `dir` via `openat`, then closes the fd.
pub fn openat_raw(dir: &File, name: &CStr) -> io::Result<()> {
    // SAFETY: `dir` is an open directory and `name` is one NUL-terminated component.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_CREAT | libc::O_WRONLY,
            0o644,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is the descriptor just returned by `openat` and is not used again.
    unsafe { libc::close(fd) };
    Ok(())
}

/// Unlinks `name` inside the open directory `dir` via `unlinkat`.
pub fn unlinkat_raw(dir: &File, name: &CStr) {
    // SAFETY: `dir` is an open directory and `name` is a single path component.
    unsafe {
        libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0);
    }
}

/// `flock` on an already-open descriptor.
pub fn flock_raw(fd: RawFd, op: libc::c_int) -> libc::c_int {
    // SAFETY: `fd` is a live descriptor owned by the caller for the call.
    unsafe { libc::flock(fd, op) }
}
