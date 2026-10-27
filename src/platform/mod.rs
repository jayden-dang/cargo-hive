//! Platform layer. Task 2 adds the OS check and version; Tasks 7–9 add `Fs` and `Approve`.

use std::process::Command;

pub const MACOS_ONLY_MESSAGE: &str = "cargo-hive supports macOS only";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotMacOs;

pub fn check_os(os: &str) -> Result<(), NotMacOs> {
    if os == "macos" {
        Ok(())
    } else {
        Err(NotMacOs)
    }
}

pub fn ensure_supported() -> Result<(), NotMacOs> {
    check_os(std::env::consts::OS)
}

/// `sw_vers -productVersion`, trimmed; `None` when it cannot be run.
pub fn os_version() -> Option<String> {
    let out = Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
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

/// APFS block sharing of one file (`getattrlist` common-extended attributes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sharing {
    pub private_bytes: u64,
    pub clone_id: u64,
    pub may_share: bool,
    pub shares_all: bool,
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
    fn sharing(&self, path: &Path) -> io::Result<Sharing>;
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
    fn the_major_version_is_parsed() {
        assert_eq!(major_version("27.0.1"), Some(27));
        assert_eq!(major_version("26.6.2"), Some(26));
        assert_eq!(major_version("x"), None);
    }
}
