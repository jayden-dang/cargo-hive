//! Advisory flock helpers for independent opens and inherited open descriptions.

use crate::slot::effective_uid;
use std::fs::{File, Metadata, OpenOptions, symlink_metadata};
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
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

/// Device and inode of a private regular lock file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockFileIdentity {
    pub dev: u64,
    pub ino: u64,
}

impl LockFileIdentity {
    /// Reads the identity of the named lock file without following the final path component.
    pub fn from_path(path: &Path) -> io::Result<Self> {
        let expected = Self::from_metadata(&symlink_metadata(path)?)?;
        #[cfg(target_os = "macos")]
        {
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_EVTONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(path)?;
            check_identity(&file, expected)?;
        }
        Ok(expected)
    }

    fn from_metadata(metadata: &Metadata) -> io::Result<Self> {
        if !metadata.is_file()
            || metadata.uid() != effective_uid()
            || metadata.mode() & 0o7077 != 0
            || metadata.nlink() != 1
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "private file must be a singly linked regular file owned by the effective user",
            ));
        }
        Ok(Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }
}

/// Opens a private regular file for reading and verifies that the named inode survived the open.
pub fn open_private_file(path: &Path) -> io::Result<File> {
    let expected = LockFileIdentity::from_metadata(&symlink_metadata(path)?)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    check_identity(&file, expected)?;
    Ok(file)
}

/// Copies an inherited descriptor into an owned, close-on-exec reference.
pub fn duplicate_inherited_fd(fd: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: fcntl validates the raw descriptor and attests ownership of any returned duplicate.
    let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if copy == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fcntl returned a new owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(copy) })
}

/// Outcome of a checked nonblocking flock.
#[derive(Debug)]
pub enum InheritedLock {
    Acquired(InheritedLockGuard),
    Held,
}

/// A reference to the locked open description. Drop closes only this reference.
#[derive(Debug)]
pub struct InheritedLockGuard {
    _file: File,
}

/// Tries an exclusive flock on the provided open description after checking its identity.
///
/// The caller must establish descriptor custody independently. A successful flock
/// can come from a fresh open if no other description holds the lock.
pub fn try_lock_inherited(
    fd: BorrowedFd<'_>,
    expected: LockFileIdentity,
) -> io::Result<InheritedLock> {
    let file = File::from(fd.try_clone_to_owned()?);
    check_identity(&file, expected)?;
    // SAFETY: file keeps this descriptor alive through the flock call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(InheritedLock::Acquired(InheritedLockGuard { _file: file }));
    }
    let err = io::Error::last_os_error();
    if err.kind() == io::ErrorKind::WouldBlock {
        return Ok(InheritedLock::Held);
    }
    Err(err)
}

fn check_identity(file: &File, expected: LockFileIdentity) -> io::Result<()> {
    if LockFileIdentity::from_metadata(&file.metadata()?)? != expected {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private file descriptor does not match the expected inode",
        ));
    }
    #[cfg(target_os = "macos")]
    refuse_extended_acl(file)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn refuse_extended_acl(file: &File) -> io::Result<()> {
    use libc::{c_int, c_void};

    unsafe extern "C" {
        fn acl_get_fd(fd: c_int) -> *mut c_void;
        fn acl_free(acl: *mut c_void) -> c_int;
    }

    // SAFETY: file owns a live descriptor through the ACL query.
    let acl = unsafe { acl_get_fd(file.as_raw_fd()) };
    if acl.is_null() {
        let err = io::Error::last_os_error();
        // Darwin returns ENOENT for an absent ACL on the open descriptor.
        return if err.raw_os_error() == Some(libc::ENOENT) {
            Ok(())
        } else {
            Err(err)
        };
    }
    // SAFETY: acl_get_fd returned this allocated ACL and acl_free disposes of it once.
    unsafe { acl_free(acl) };
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "private file must not have an extended ACL",
    ))
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
