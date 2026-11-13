//! Platform layer: the OS gate, `Fs`, lock try and hold, bounded listing, and volume info.

pub const MACOS_ONLY_MESSAGE: &str = "cargo-hive supports macOS only";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotMacOs;

pub fn check_os(os: &str) -> Result<(), NotMacOs> {
    if os == "macos" { Ok(()) } else { Err(NotMacOs) }
}

pub fn ensure_supported() -> Result<(), NotMacOs> {
    check_os(std::env::consts::OS)
}

/// `sw_vers -productVersion`, trimmed; `None` when it cannot be run.
pub fn os_version() -> Option<String> {
    let mut spec = proc::Spec::new("sw_vers");
    spec.args.push("-productVersion".into());
    let out = proc::output(&spec).ok()?;
    let v = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !v.is_empty()).then_some(v)
}

pub fn major_version(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[cfg(target_os = "macos")]
pub mod apfs;
pub mod clock;
pub mod flock;
pub mod proc;
pub mod signals;
#[doc(hidden)]
pub mod testing;

/// Whether `pid` still names a live process (`kill(pid, 0)`).
pub fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 delivers nothing; it only asks whether `pid` exists.
    let gone = unsafe { libc::kill(pid, 0) } == -1
        && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    !gone
}

/// A directory this process created. Dropping it removes the directory and everything in it.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn adopt(path: PathBuf) -> TempDir {
        TempDir { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

/// Kind of a `FileType` from `symlink_metadata` or `DirEntry::file_type` (neither follows a link).
pub(crate) fn entry_kind(t: std::fs::FileType) -> EntryKind {
    if t.is_symlink() {
        EntryKind::Symlink
    } else if t.is_dir() {
        EntryKind::Dir
    } else if t.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    }
}

/// `lstat` of one entry: never the target of a symbolic link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryStat {
    pub dev: u64,
    pub ino: u64,
    pub nlink: u64,
    pub blocks: u64,
    pub kind: EntryKind,
    /// APFS transparent compression (`UF_COMPRESSED`).
    pub compressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    pub fs_type: String,
    /// The APFS container (`disk<N>`) the volume belongs to, or `mount:<path>` when unknown.
    pub container: String,
    pub mount: PathBuf,
    pub available_bytes: u64,
}

/// The container of a volume from `statfs.f_mntfromname`: the leading `disk<N>` after any
/// `…@` snapshot prefix. Volumes of one APFS container share free space.
pub fn container_key(mntfromname: &str) -> Option<String> {
    let device = mntfromname.rsplit_once('@').map_or(mntfromname, |(_, d)| d);
    let rest = device.strip_prefix("/dev/disk")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty()).then(|| format!("disk{digits}"))
}

/// APFS block sharing of one file (`getattrlist` common-extended attributes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sharing {
    pub private_bytes: u64,
    pub clone_id: u64,
    pub may_share: bool,
    pub shares_all: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompressOutcome {
    Compressed,
    SkippedHardLink,
    Incompressible,
    Failed(String),
}

impl std::fmt::Display for CompressOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompressOutcome::Compressed => write!(f, "compressed"),
            CompressOutcome::SkippedHardLink => write!(f, "skipped: hard link"),
            CompressOutcome::Incompressible => write!(f, "incompressible"),
            CompressOutcome::Failed(why) => write!(f, "failed: {why}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockKind {
    Flock,
    Fcntl,
    FcntlRead,
}

impl LockKind {
    pub fn as_arg(self) -> &'static str {
        match self {
            LockKind::Flock => "flock",
            LockKind::Fcntl => "fcntl",
            LockKind::FcntlRead => "fcntl-read",
        }
    }

    pub fn parse(arg: &str) -> Option<LockKind> {
        match arg {
            "flock" => Some(LockKind::Flock),
            "fcntl" => Some(LockKind::Fcntl),
            "fcntl-read" => Some(LockKind::FcntlRead),
            _ => None,
        }
    }
}

/// How long hive waits for a lock child (`__hold-lock`, `__try-lock`) to answer.
pub const CHILD_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    Free,
    Held,
}

/// A lock held on hive's behalf. Releasing or dropping it lets the lock go.
pub struct LockHolder {
    release: Option<Box<dyn FnOnce() + Send>>,
}

impl LockHolder {
    pub fn new(release: impl FnOnce() + Send + 'static) -> LockHolder {
        LockHolder {
            release: Some(Box::new(release)),
        }
    }

    pub fn release(mut self) {
        if let Some(f) = self.release.take() {
            f();
        }
    }
}

impl Drop for LockHolder {
    fn drop(&mut self) {
        if let Some(f) = self.release.take() {
            f();
        }
    }
}

/// A lock this process holds. Dropping it releases the lock.
pub struct HeldLock {
    release: Option<Box<dyn FnOnce() + Send>>,
    identity: Option<(u64, u64)>,
}

impl HeldLock {
    pub fn new(release: impl FnOnce() + Send + 'static) -> HeldLock {
        HeldLock {
            release: Some(Box::new(release)),
            identity: None,
        }
    }

    pub fn with_identity(dev: u64, ino: u64, release: impl FnOnce() + Send + 'static) -> HeldLock {
        HeldLock {
            release: Some(Box::new(release)),
            identity: Some((dev, ino)),
        }
    }

    pub fn identity(&self) -> Option<(u64, u64)> {
        self.identity
    }
}

impl Drop for HeldLock {
    fn drop(&mut self) {
        if let Some(f) = self.release.take() {
            f();
        }
    }
}

impl std::fmt::Debug for HeldLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HeldLock")
    }
}

/// The outcome of one non-blocking exclusive try on a lock file.
#[derive(Debug)]
pub enum TryLock {
    /// The lock is ours until the guard is dropped.
    Acquired(HeldLock),
    /// Another process holds a lock on the file.
    Held,
    /// The file does not exist.
    Gone,
    /// The file could not be opened, or is not a regular file.
    Unopenable(io::Error),
    /// The file opened, but `flock` failed for a reason other than another holder.
    Unlockable(io::Error),
}

/// Body of the hidden `__hold-lock` command: take the lock, print `held`, wait to be killed.
pub fn hold_lock_forever(kind: LockKind, path: &Path) -> io::Result<std::convert::Infallible> {
    #[cfg(target_os = "macos")]
    {
        apfs::hold_lock_forever(kind, path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (kind, path);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            MACOS_ONLY_MESSAGE,
        ))
    }
}

/// Body of the hidden `__try-lock` command: try `kind` on `path` once without waiting.
pub fn try_lock_once(kind: LockKind, path: &Path) -> io::Result<LockState> {
    #[cfg(target_os = "macos")]
    {
        apfs::try_lock_once(kind, path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (kind, path);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            MACOS_ONLY_MESSAGE,
        ))
    }
}

/// The filesystem operations hive needs. Doctor's checks run through this trait so they
/// can be exercised against a fake.
pub trait Fs {
    /// Creates `<parent>/<prefix>XXXXXXXX` with `mkdtemp`.
    fn temp_dir(&self, parent: &Path, prefix: &str) -> io::Result<TempDir>;
    fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    fn hardlink(&self, src: &Path, dst: &Path) -> io::Result<()>;
    fn set_mtime(&self, path: &Path, t: SystemTime) -> io::Result<()>;
    fn mtime(&self, path: &Path) -> io::Result<SystemTime>;
    fn inode(&self, path: &Path) -> io::Result<u64>;
    fn clone_file(&self, src: &Path, dst: &Path) -> io::Result<()>;
    fn create_dir(&self, path: &Path) -> io::Result<()>;
    fn sharing(&self, path: &Path) -> io::Result<Sharing>;
    /// Applesauce LZFSE compression of one file. A file with other hard links is skipped.
    fn compress(&self, path: &Path) -> CompressOutcome;
    /// Holds `kind` on `path` in another process until the holder is released.
    fn hold_lock(&self, kind: LockKind, path: &Path) -> io::Result<LockHolder>;
    /// `flock(LOCK_EX | LOCK_NB)` on `path`, opened read-only with `O_NOFOLLOW | O_NONBLOCK`
    /// and never created; the lock lasts until the returned guard is dropped.
    fn try_lock_exclusive(&self, path: &Path) -> TryLock;
    /// Has another process try `kind` on `path` once, without waiting.
    fn try_lock_elsewhere(&self, kind: LockKind, path: &Path) -> io::Result<LockState>;
    /// `lstat`: never follows a symbolic link.
    fn entry_stat(&self, path: &Path) -> io::Result<EntryStat>;
    /// The UTF-8 names and no-follow kinds of `dir`'s entries. More than `cap` entries is an
    /// error, and so is a name that is not UTF-8.
    fn read_dir_bounded(&self, dir: &Path, cap: usize) -> io::Result<Vec<(String, EntryKind)>>;
    /// Removes a directory tree, or a symbolic link given as the root, never following a link.
    fn remove_tree(&self, path: &Path) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    /// `statfs` of the volume holding `path`.
    fn volume(&self, path: &Path) -> io::Result<VolumeInfo>;
    fn load_average_hundredths(&self) -> io::Result<u64> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Available,
    Unavailable(String),
}

pub struct Platform {
    pub fs: Box<dyn Fs>,
}

#[cfg(target_os = "macos")]
pub fn current() -> Result<Platform, NotMacOs> {
    let holder = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("cargo-hive"));
    Ok(Platform {
        fs: Box::new(apfs::Apfs::new(holder)),
    })
}

#[cfg(not(target_os = "macos"))]
pub fn current() -> Result<Platform, NotMacOs> {
    Err(NotMacOs)
}

/// Host filesystem for explain: APFS when available, otherwise a listing-only stub.
pub fn host_fs() -> Box<dyn Fs> {
    #[cfg(target_os = "macos")]
    if let Ok(platform) = current() {
        return platform.fs;
    }
    Box::new(DirListFs)
}

struct DirListFs;

fn unsupported_fs() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, "not supported")
}

impl Fs for DirListFs {
    fn temp_dir(&self, _parent: &Path, _prefix: &str) -> io::Result<TempDir> {
        Err(unsupported_fs())
    }

    fn write_file(&self, _path: &Path, _bytes: &[u8]) -> io::Result<()> {
        Err(unsupported_fs())
    }

    fn read(&self, _path: &Path) -> io::Result<Vec<u8>> {
        Err(unsupported_fs())
    }

    fn hardlink(&self, _src: &Path, _dst: &Path) -> io::Result<()> {
        Err(unsupported_fs())
    }

    fn set_mtime(&self, _path: &Path, _t: SystemTime) -> io::Result<()> {
        Err(unsupported_fs())
    }

    fn mtime(&self, _path: &Path) -> io::Result<SystemTime> {
        Err(unsupported_fs())
    }

    fn inode(&self, _path: &Path) -> io::Result<u64> {
        Err(unsupported_fs())
    }

    fn clone_file(&self, _src: &Path, _dst: &Path) -> io::Result<()> {
        Err(unsupported_fs())
    }

    fn create_dir(&self, _path: &Path) -> io::Result<()> {
        Err(unsupported_fs())
    }

    fn sharing(&self, _path: &Path) -> io::Result<Sharing> {
        Err(unsupported_fs())
    }

    fn compress(&self, _path: &Path) -> CompressOutcome {
        CompressOutcome::Failed("not supported".to_string())
    }

    fn hold_lock(&self, _kind: LockKind, _path: &Path) -> io::Result<LockHolder> {
        Err(unsupported_fs())
    }

    fn try_lock_exclusive(&self, _path: &Path) -> TryLock {
        TryLock::Unopenable(unsupported_fs())
    }

    fn try_lock_elsewhere(&self, _kind: LockKind, _path: &Path) -> io::Result<LockState> {
        Err(unsupported_fs())
    }

    fn entry_stat(&self, _path: &Path) -> io::Result<EntryStat> {
        Err(unsupported_fs())
    }

    fn read_dir_bounded(&self, dir: &Path, cap: usize) -> io::Result<Vec<(String, EntryKind)>> {
        read_dir_capped(dir, cap)
    }

    fn remove_tree(&self, _path: &Path) -> io::Result<()> {
        Err(unsupported_fs())
    }

    fn remove_file(&self, _path: &Path) -> io::Result<()> {
        Err(unsupported_fs())
    }

    fn volume(&self, _path: &Path) -> io::Result<VolumeInfo> {
        Err(unsupported_fs())
    }
}

fn read_dir_capped(dir: &Path, cap: usize) -> io::Result<Vec<(String, EntryKind)>> {
    let mut out = Vec::new();
    for (seen, entry) in std::fs::read_dir(dir)?.enumerate() {
        if seen >= cap {
            return Err(io::Error::other(format!(
                "{} has more than {cap} entries",
                dir.display()
            )));
        }
        let entry = entry?;
        let kind = entry_kind(entry.file_type()?);
        let name = entry.file_name().into_string().map_err(|_| {
            io::Error::other(format!(
                "{} has a name that is not valid UTF-8",
                dir.display()
            ))
        })?;
        out.push((name, kind));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_macos_is_supported() {
        assert_eq!(check_os("macos"), Ok(()));
        assert_eq!(check_os("linux"), Err(NotMacOs));
        assert_eq!(check_os("windows"), Err(NotMacOs));
        assert_eq!(MACOS_ONLY_MESSAGE, "cargo-hive supports macOS only");
    }

    #[test]
    fn seed_errno_values_map_to_the_error_kinds_seed_matches() {
        let rows = [
            (libc::ENOSPC, io::ErrorKind::StorageFull),
            (libc::EXDEV, io::ErrorKind::CrossesDevices),
            (libc::EEXIST, io::ErrorKind::AlreadyExists),
            (libc::ENOTEMPTY, io::ErrorKind::DirectoryNotEmpty),
            (libc::ENOTDIR, io::ErrorKind::NotADirectory),
        ];
        for (errno, kind) in rows {
            assert_eq!(
                io::Error::from_raw_os_error(errno).kind(),
                kind,
                "errno {errno}"
            );
        }
        assert_eq!(libc::ENOSPC, 28);
        assert_eq!(libc::EXDEV, 18);
        assert_eq!(libc::EEXIST, 17);
        assert_eq!(libc::ENOTEMPTY, 66);
        assert_eq!(libc::ENOTDIR, 20);
    }

    #[test]
    fn the_major_version_is_parsed() {
        assert_eq!(major_version("27.0.1"), Some(27));
        assert_eq!(major_version("26.6.2"), Some(26));
        assert_eq!(major_version("x"), None);
    }

    #[test]
    fn lock_children_get_five_seconds_to_answer() {
        assert_eq!(CHILD_DEADLINE.as_secs(), 5);
    }

    struct Refuse;

    fn refused() -> io::Error {
        io::Error::other("refused")
    }

    impl Fs for Refuse {
        fn temp_dir(&self, _: &Path, _: &str) -> io::Result<TempDir> {
            Err(refused())
        }
        fn write_file(&self, _: &Path, _: &[u8]) -> io::Result<()> {
            Err(refused())
        }
        fn read(&self, _: &Path) -> io::Result<Vec<u8>> {
            Err(refused())
        }
        fn hardlink(&self, _: &Path, _: &Path) -> io::Result<()> {
            Err(refused())
        }
        fn set_mtime(&self, _: &Path, _: SystemTime) -> io::Result<()> {
            Err(refused())
        }
        fn mtime(&self, _: &Path) -> io::Result<SystemTime> {
            Err(refused())
        }
        fn inode(&self, _: &Path) -> io::Result<u64> {
            Err(refused())
        }
        fn clone_file(&self, _: &Path, _: &Path) -> io::Result<()> {
            Err(refused())
        }
        fn create_dir(&self, path: &Path) -> io::Result<()> {
            std::fs::create_dir(path)
        }
        fn sharing(&self, _: &Path) -> io::Result<Sharing> {
            Err(refused())
        }
        fn compress(&self, _: &Path) -> CompressOutcome {
            CompressOutcome::Failed("refused".into())
        }
        fn hold_lock(&self, _: LockKind, _: &Path) -> io::Result<LockHolder> {
            Err(refused())
        }
        fn try_lock_exclusive(&self, _: &Path) -> TryLock {
            TryLock::Unopenable(refused())
        }
        fn try_lock_elsewhere(&self, _: LockKind, _: &Path) -> io::Result<LockState> {
            Err(refused())
        }
        fn entry_stat(&self, _: &Path) -> io::Result<EntryStat> {
            Err(refused())
        }
        fn read_dir_bounded(&self, _: &Path, _: usize) -> io::Result<Vec<(String, EntryKind)>> {
            Err(refused())
        }
        fn remove_tree(&self, _: &Path) -> io::Result<()> {
            Err(refused())
        }
        fn remove_file(&self, _: &Path) -> io::Result<()> {
            Err(refused())
        }
        fn volume(&self, _: &Path) -> io::Result<VolumeInfo> {
            Err(refused())
        }
    }

    #[test]
    fn a_hooked_filesystem_uses_the_closure_when_one_is_set() {
        let root = PathBuf::from(format!(
            "/private/tmp/hive-it-hooked-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let fs = testing::Hooked {
            inner: Box::new(Refuse),
            compress: Some(Box::new(|_| CompressOutcome::Incompressible)),
            volume: Some(Box::new(|_| {
                Ok(VolumeInfo {
                    fs_type: "apfs".into(),
                    container: "disk9".into(),
                    mount: PathBuf::from("/"),
                    available_bytes: 3,
                })
            })),
            sharing: Some(Box::new(|_| {
                Ok(Sharing {
                    private_bytes: 1,
                    clone_id: 9,
                    may_share: false,
                    shares_all: false,
                })
            })),
            entry_stat: Some(Box::new(|_| {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "injected"))
            })),
        };
        let path = root.join("missing");
        assert_eq!(fs.compress(&path), CompressOutcome::Incompressible);
        assert_eq!(fs.volume(&path).unwrap().available_bytes, 3);
        assert_eq!(fs.sharing(&path).unwrap().clone_id, 9);
        assert_eq!(
            fs.entry_stat(&path).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        let made = root.join("made");
        fs.create_dir(&made).unwrap();
        assert!(made.is_dir());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn lock_kinds_round_trip_their_argument() {
        for kind in [LockKind::Flock, LockKind::Fcntl, LockKind::FcntlRead] {
            assert_eq!(LockKind::parse(kind.as_arg()), Some(kind));
        }
        assert_eq!(LockKind::FcntlRead.as_arg(), "fcntl-read");
        assert_eq!(LockKind::parse("nope"), None);
    }

    #[test]
    fn the_container_is_the_leading_bsd_disk() {
        let cases = [
            ("/dev/disk3s1s1", Some("disk3")),
            ("/dev/disk3s5", Some("disk3")),
            ("/dev/disk12s2", Some("disk12")),
            (
                "com.apple.TimeMachine.2026-10-28-120000.backup@/dev/disk3s5",
                Some("disk3"),
            ),
            ("map auto_home", None),
            ("//user@host/share", None),
            ("/dev/diskX", None),
        ];
        for (from, want) in cases {
            assert_eq!(container_key(from).as_deref(), want, "{from}");
        }
    }
}
