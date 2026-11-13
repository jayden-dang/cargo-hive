//! Process cancel flag set by SIGINT/SIGTERM handlers.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set by SIGINT/SIGTERM; callers poll it between steps and while waiting.
static CANCEL: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_signal: libc::c_int) {
    CANCEL.store(true, Ordering::SeqCst);
}

pub fn install() -> io::Result<()> {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: an all-zero sigaction is valid; the handler only stores to an atomic,
        // which is async-signal-safe.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

pub fn cancelled() -> bool {
    CANCEL.load(Ordering::SeqCst)
}
