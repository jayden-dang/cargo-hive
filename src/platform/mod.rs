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
