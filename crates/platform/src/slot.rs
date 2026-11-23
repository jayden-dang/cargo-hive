//! Local process identity and OS entropy for the slot writer fence.

use std::io;

#[cfg(not(target_os = "macos"))]
use crate::MACOS_ONLY_MESSAGE;

/// A process instance within the local boot session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInstance {
    pub boot_session: String,
    pub pid: u32,
    pub start_secs: u64,
    pub start_micros: u64,
}

/// Effective user ID for checking private filesystem ownership.
pub fn effective_uid() -> u32 {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

/// Local boot session UUID. Failure leaves the boot identity unknown.
#[cfg(target_os = "macos")]
pub fn boot_session() -> io::Result<String> {
    const UUID_LEN: usize = 36;
    let mut uuid = [0u8; UUID_LEN + 1];
    let mut len = uuid.len();
    // SAFETY: uuid is writable for len bytes and the sysctl name is NUL-terminated.
    let read = unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            uuid.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return Err(io::Error::last_os_error());
    }
    let digits = &uuid[..UUID_LEN];
    let expected_shape = digits.iter().enumerate().all(|(index, byte)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            *byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        }
    });
    if len != uuid.len() || uuid[UUID_LEN] != 0 || !expected_shape {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid boot session UUID",
        ));
    }
    String::from_utf8(digits.to_vec())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

#[cfg(not(target_os = "macos"))]
pub fn boot_session() -> io::Result<String> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        MACOS_ONLY_MESSAGE,
    ))
}

/// Identifies the current parent only while the same runner remains the parent.
#[cfg(target_os = "macos")]
pub fn parent_instance() -> io::Result<ProcessInstance> {
    // SAFETY: getppid takes no arguments.
    let pid = unsafe { libc::getppid() };
    if pid <= 1 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "parent runner absent",
        ));
    }
    let boot_session = boot_session()?;
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: info is writable for size bytes and the flavor writes proc_bsdinfo.
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if read != size {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "parent runner identity unavailable",
        ));
    }
    // SAFETY: proc_pidinfo populated one complete proc_bsdinfo.
    let info = unsafe { info.assume_init() };
    // SAFETY: getppid takes no arguments.
    let parent_after_lookup = unsafe { libc::getppid() };
    if pid != parent_after_lookup
        || info.pbi_pid != pid as u32
        || info.pbi_uid != effective_uid()
        || info.pbi_start_tvsec == 0
        || info.pbi_start_tvusec >= 1_000_000
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "parent runner instance could not be verified",
        ));
    }
    Ok(ProcessInstance {
        boot_session,
        pid: info.pbi_pid,
        start_secs: info.pbi_start_tvsec,
        start_micros: info.pbi_start_tvusec,
    })
}

#[cfg(not(target_os = "macos"))]
pub fn parent_instance() -> io::Result<ProcessInstance> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        MACOS_ONLY_MESSAGE,
    ))
}

/// Generates a fixed-size secret or identifier using macOS kernel entropy.
#[cfg(target_os = "macos")]
pub fn random_slot_bytes() -> io::Result<[u8; 32]> {
    let mut bytes = [0u8; 32];
    // SAFETY: bytes is writable for bytes.len() bytes, below getentropy's 256-byte limit.
    if unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(bytes)
}

#[cfg(not(target_os = "macos"))]
pub fn random_slot_bytes() -> io::Result<[u8; 32]> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        MACOS_ONLY_MESSAGE,
    ))
}
