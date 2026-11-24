//! Advisory flock helpers for independent opens and inherited open descriptions.

use crate::slot::effective_uid;
use std::ffi::{CString, OsStr};
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

/// Checks the owner, mode and ACL on a descriptor for the named directory.
pub fn checked_owner_directory(path: &Path) -> io::Result<Metadata> {
    open_owner_directory(path).map(|(_, metadata)| metadata)
}

/// Opens the named directory and checks the owner, mode and ACL on that descriptor.
pub fn open_owner_directory(path: &Path) -> io::Result<(File, Metadata)> {
    let (expected, file) = open_named_directory(path)?;
    let metadata = check_owned_inode(&file, &expected, Metadata::is_dir)?;
    Ok((file, metadata))
}

/// Checks that a directory above a private inode cannot be replaced or modified by another user.
pub fn checked_stable_ancestor(path: &Path) -> io::Result<()> {
    let (expected, file) = open_named_directory(path)?;
    let uid = effective_uid();
    check_named_inode(&file, &expected, |meta| {
        let mode = meta.mode();
        meta.is_dir()
            && (meta.uid() == 0 || meta.uid() == uid)
            && (mode & 0o7022 == 0 || meta.uid() == 0 && mode & 0o7000 == 0o1000)
    })?;
    #[cfg(target_os = "macos")]
    check_extended_acl(&file, true)?;
    Ok(())
}

fn open_named_directory(path: &Path) -> io::Result<(Metadata, File)> {
    let expected = symlink_metadata(path)?;
    if !expected.is_dir() {
        return Err(unsafe_owner_inode());
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    Ok((expected, file))
}

/// Opens a single named child directory without following links or opening a pipe.
pub fn open_directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
    let name = child_name(name)?;
    // SAFETY: parent owns the open directory descriptor, and name is one NUL-terminated component.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY
                | libc::O_DIRECTORY
                | libc::O_NOFOLLOW
                | libc::O_NONBLOCK
                | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn child_name(name: &OsStr) -> io::Result<CString> {
    let name = name.as_encoded_bytes();
    if name.is_empty() || name == b"." || name == b".." || name.contains(&b'/') {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    CString::new(name).map_err(|_| io::ErrorKind::InvalidInput.into())
}

/// Checks a child directory's owner and ACL on the descriptor returned by a no-follow open.
pub fn open_owner_directory_at(parent: &File, name: &OsStr) -> io::Result<(File, Metadata)> {
    let file = open_directory_at(parent, name)?;
    let metadata = check_owned_inode(&file, &file.metadata()?, Metadata::is_dir)?;
    Ok((file, metadata))
}

/// Opens an owner-controlled, singly linked regular leaf beneath an open directory.
pub fn open_owner_file_at(parent: &File, name: &OsStr) -> io::Result<File> {
    let name = child_name(name)?;
    let mut expected = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: the owned parent is open, name is one C string, and expected is writable.
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            expected.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstatat succeeded and initialized the complete record.
    let expected = unsafe { expected.assume_init() };
    if expected.st_mode & libc::S_IFMT != libc::S_IFREG
        || expected.st_nlink != 1
        || expected.st_uid != effective_uid()
        || expected.st_mode & 0o7022 != 0
    {
        return Err(unsafe_owner_inode());
    }
    // SAFETY: name is one NUL-terminated component beneath the open parent.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a new owned descriptor.
    let file = unsafe { File::from_raw_fd(fd) };
    let mut actual = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fd is owned by file and actual is writable for a stat record.
    if unsafe { libc::fstat(file.as_raw_fd(), actual.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstat succeeded and initialized the complete record.
    let actual = unsafe { actual.assume_init() };
    if actual.st_dev != expected.st_dev || actual.st_ino != expected.st_ino {
        return Err(unsafe_owner_inode());
    }
    check_owned_inode(&file, &file.metadata()?, |meta| {
        meta.is_file() && meta.nlink() == 1
    })?;
    Ok(file)
}

/// Opens an independent descriptor for the legacy state lock, keeping its existing mode.
pub fn open_owner_state_lock(path: &Path) -> io::Result<File> {
    open_owner_state_file_with(path, true)
}

/// Reads a legacy state file from a checked descriptor without changing its mode.
pub fn open_owner_state_file(path: &Path) -> io::Result<File> {
    open_owner_state_file_with(path, false)
}

fn open_owner_state_file_with(path: &Path, write: bool) -> io::Result<File> {
    let expected = symlink_metadata(path)?;
    if !expected.is_file() {
        return Err(unsafe_owner_inode());
    }
    let file = OpenOptions::new()
        .read(true)
        .write(write)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                unsafe_owner_inode()
            } else {
                err
            }
        })?;
    check_owned_inode(&file, &expected, |meta| meta.is_file() && meta.nlink() == 1)?;
    Ok(file)
}

fn check_owned_inode(
    file: &File,
    expected: &Metadata,
    valid: impl Fn(&Metadata) -> bool,
) -> io::Result<Metadata> {
    let uid = effective_uid();
    let actual = check_named_inode(file, expected, |meta| {
        valid(meta) && meta.uid() == uid && meta.mode() & 0o7022 == 0
    })?;
    #[cfg(target_os = "macos")]
    check_extended_acl(file, false)?;
    Ok(actual)
}

fn check_named_inode(
    file: &File,
    expected: &Metadata,
    valid: impl Fn(&Metadata) -> bool,
) -> io::Result<Metadata> {
    let actual = file.metadata()?;
    if !valid(expected)
        || !valid(&actual)
        || actual.dev() != expected.dev()
        || actual.ino() != expected.ino()
    {
        return Err(unsafe_owner_inode());
    }
    Ok(actual)
}

fn unsafe_owner_inode() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "unsafe owner-controlled inode",
    )
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

/// Checks privacy on an already open descriptor without taking a lock on it.
pub fn private_file_metadata(file: &File) -> io::Result<Metadata> {
    let metadata = file.metadata()?;
    LockFileIdentity::from_metadata(&metadata)?;
    #[cfg(target_os = "macos")]
    check_extended_acl(file, false)?;
    Ok(metadata)
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
    let metadata = private_file_metadata(file)?;
    if metadata.dev() != expected.dev || metadata.ino() != expected.ino {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private file descriptor does not match the expected inode",
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn check_extended_acl(file: &File, deny_only: bool) -> io::Result<()> {
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
    let accepted = deny_only && acl_is_deny_only(acl);
    // SAFETY: acl_get_fd returned this allocated ACL and acl_free disposes of it once.
    unsafe { acl_free(acl) };
    if accepted {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "extended ACL is not permitted on this owner-controlled inode",
        ))
    }
}

#[cfg(target_os = "macos")]
fn acl_is_deny_only(acl: *mut libc::c_void) -> bool {
    use libc::{c_int, c_void};

    const ACL_FIRST_ENTRY: c_int = 0;
    const ACL_NEXT_ENTRY: c_int = -1;
    const ACL_EXTENDED_DENY: c_int = 2;
    const ACL_MAX_ENTRIES: usize = 128;

    unsafe extern "C" {
        fn acl_valid(acl: *mut c_void) -> c_int;
        fn acl_get_entry(acl: *mut c_void, id: c_int, entry: *mut *mut c_void) -> c_int;
        fn acl_get_tag_type(entry: *mut c_void, tag: *mut c_int) -> c_int;
    }

    // SAFETY: the caller holds an ACL returned by acl_get_fd for the whole call.
    if unsafe { acl_valid(acl) } != 0 {
        return false;
    }
    let mut id = ACL_FIRST_ENTRY;
    for count in 0..=ACL_MAX_ENTRIES {
        let mut entry = std::ptr::null_mut();
        // SAFETY: the ACL is valid and entry points to a writable entry pointer.
        if unsafe { acl_get_entry(acl, id, &mut entry) } != 0 {
            // On Darwin, EINVAL after traversing a valid ACL marks the end of its entries.
            return id == ACL_NEXT_ENTRY
                && io::Error::last_os_error().raw_os_error() == Some(libc::EINVAL);
        }
        let mut tag = 0;
        // SAFETY: success above yields an entry from the still-live ACL and tag is writable.
        if count == ACL_MAX_ENTRIES
            || unsafe { acl_get_tag_type(entry, &mut tag) } != 0
            || tag != ACL_EXTENDED_DENY
        {
            return false;
        }
        id = ACL_NEXT_ENTRY;
    }
    false
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
