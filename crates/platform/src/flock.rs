//! Advisory flock helpers: exclusive/shared with cancel poll, and a one-shot try.

use std::fs::File;
use std::io;
use std::os::unix::io::{AsRawFd, RawFd};
use std::time::Duration;

const LOCK_POLL: Duration = Duration::from_millis(10);

#[derive(Debug)]
pub enum FlockError {
    Cancelled,
    Io(io::Error),
}

/// A held flock. Unlocking happens on drop; the caller's `File` must outlive this guard.
#[derive(Debug)]
pub struct FlockGuard {
    fd: RawFd,
}

impl Drop for FlockGuard {
    fn drop(&mut self) {
        // SAFETY: `fd` was locked by a function in this module; the caller's `File`
        // keeps the descriptor open for this guard's lifetime.
        let _ = unsafe { libc::flock(self.fd, libc::LOCK_UN) };
    }
}

pub fn exclusive_or_cancelled(
    file: &File,
    mut cancel: impl FnMut() -> bool,
) -> Result<FlockGuard, FlockError> {
    loop {
        // SAFETY: `file` owns a valid descriptor for the whole call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(FlockGuard {
                fd: file.as_raw_fd(),
            });
        }
        let err = io::Error::last_os_error();
        match err.kind() {
            io::ErrorKind::Interrupted => continue,
            io::ErrorKind::WouldBlock if cancel() => return Err(FlockError::Cancelled),
            io::ErrorKind::WouldBlock => std::thread::sleep(LOCK_POLL),
            _ => return Err(FlockError::Io(err)),
        }
    }
}

pub fn shared_or_cancelled(
    file: &File,
    mut cancel: impl FnMut() -> bool,
) -> Result<FlockGuard, FlockError> {
    loop {
        // SAFETY: `file` owns a valid descriptor for the whole call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0 {
            return Ok(FlockGuard {
                fd: file.as_raw_fd(),
            });
        }
        let err = io::Error::last_os_error();
        match err.kind() {
            io::ErrorKind::Interrupted => continue,
            io::ErrorKind::WouldBlock if cancel() => return Err(FlockError::Cancelled),
            io::ErrorKind::WouldBlock => std::thread::sleep(LOCK_POLL),
            _ => return Err(FlockError::Io(err)),
        }
    }
}

pub fn try_exclusive(file: &File) -> io::Result<Option<FlockGuard>> {
    loop {
        // SAFETY: `file` owns a valid descriptor for the whole call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Some(FlockGuard {
                fd: file.as_raw_fd(),
            }));
        }
        let err = io::Error::last_os_error();
        match err.kind() {
            io::ErrorKind::Interrupted => continue,
            io::ErrorKind::WouldBlock => return Ok(None),
            _ => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FlockError, exclusive_or_cancelled, try_exclusive};
    use std::fs::{File, OpenOptions};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch_file(label: &str) -> (PathBuf, File) {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("hive-flock-{label}-{nanos}"));
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .unwrap();
        (path, file)
    }

    fn open_again(path: &std::path::Path) -> File {
        OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .unwrap()
    }

    #[test]
    fn try_exclusive_returns_none_while_another_guard_lives_and_some_after_it_drops() {
        let (path, first_file) = scratch_file("try");
        let first = try_exclusive(&first_file)
            .unwrap()
            .expect("first exclusive");
        let second_file = open_again(&path);
        assert!(
            matches!(try_exclusive(&second_file), Ok(None)),
            "second try should see the held lock"
        );
        drop(first);
        assert!(
            matches!(try_exclusive(&second_file), Ok(Some(_))),
            "after drop the lock is free"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn exclusive_or_cancelled_returns_cancelled_when_cancel_is_true_and_lock_is_held() {
        let (path, holder_file) = scratch_file("cancel");
        let _holder = try_exclusive(&holder_file).unwrap().expect("holder");
        let waiter = open_again(&path);
        let err = exclusive_or_cancelled(&waiter, || true).unwrap_err();
        assert!(matches!(err, FlockError::Cancelled));
        let _ = std::fs::remove_file(&path);
    }
}
