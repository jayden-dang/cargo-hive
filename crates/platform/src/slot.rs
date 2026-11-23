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

/// Hashes a bounded evidence payload with the system SHA-256 implementation.
#[cfg(target_os = "macos")]
pub fn sha256(bytes: &[u8]) -> io::Result<[u8; 32]> {
    unsafe extern "C" {
        fn CC_SHA256(data: *const u8, len: u32, digest: *mut u8) -> *mut u8;
    }
    let len = u32::try_from(bytes.len()).map_err(|_| io::ErrorKind::InvalidInput)?;
    let mut digest = [0; 32];
    // SAFETY: bytes is readable for len bytes, and digest is writable for 32 bytes.
    if unsafe { CC_SHA256(bytes.as_ptr(), len, digest.as_mut_ptr()) }.is_null() {
        return Err(io::Error::other("could not hash slot proof"));
    }
    Ok(digest)
}

#[cfg(not(target_os = "macos"))]
pub fn sha256(_bytes: &[u8]) -> io::Result<[u8; 32]> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        MACOS_ONLY_MESSAGE,
    ))
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

/// Observed process image and execution inputs after the guide's child has exec'd.
pub struct ForegroundImage {
    pub program: std::path::PathBuf,
    pub cwd: std::path::PathBuf,
    pub argv: Vec<std::ffi::OsString>,
    pub environment: std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
}

#[derive(Clone, Copy)]
pub enum ForegroundExec {
    Handoff,
    Writer,
}

#[cfg(target_os = "macos")]
mod foreground {
    use super::{
        ForegroundExec, ForegroundImage, ProcessInstance, boot_session, effective_uid,
        parent_instance, random_slot_bytes,
    };
    use crate::flock::LockFileIdentity;
    use std::ffi::OsString;
    use std::fs::File;
    use std::io::{self, Seek, SeekFrom};
    use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    const PROC_UID_ONLY: u32 = 4;
    const PROC_PIDFDVNODEINFO: i32 = 1;
    const PROC_FP_CLEXEC: u32 = 2;
    const PIDS_LIMIT: usize = 8192;
    const ARGUMENT_LIMIT: usize = 256 * 1024;

    #[repr(C)]
    struct VnodeFdInfo {
        open_flags: u32,
        status: u32,
        offset: i64,
        kind: i32,
        guard_flags: u32,
        vnode: libc::vnode_info,
    }

    /// Kernel exit observer for a stopped process in the guide's dedicated process group.
    pub struct ForegroundWatch {
        parent: ProcessInstance,
        child: ProcessInstance,
        lock_fd: i32,
        lock: LockFileIdentity,
        lock_offset: i64,
        events: OwnedFd,
    }

    impl ForegroundWatch {
        pub fn start(
            parent: ProcessInstance,
            child: u32,
            lock_fd: i32,
            lock: LockFileIdentity,
            inherited: BorrowedFd<'_>,
        ) -> io::Result<Self> {
            let pid = i32::try_from(child).map_err(|_| invalid("invalid foreground child PID"))?;
            let info = process(pid)?;
            let random = random_slot_bytes()?;
            let offset = (i64::from_ne_bytes(
                random[..8]
                    .try_into()
                    .map_err(|_| invalid("could not create a lock descriptor challenge"))?,
            ) & 0x1fff_ffff_ffff_ffff)
                | (1 << 32);
            // A random file offset changes only the inherited open description, not the inode.
            File::from(inherited.try_clone_to_owned()?).seek(SeekFrom::Start(offset as u64))?;
            if !same_child(&info, &parent, child)
                || info.pbi_status != libc::SSTOP
                || parent_instance()? != parent
                || !owns_lock(pid, lock_fd, lock, Some(offset))?
            {
                return Err(invalid(
                    "could not verify stopped foreground child lock custody",
                ));
            }
            let child = ProcessInstance {
                boot_session: boot_session()?,
                pid: child,
                start_secs: info.pbi_start_tvsec,
                start_micros: info.pbi_start_tvusec,
            };
            // SAFETY: kqueue has no arguments and returns a newly owned descriptor.
            let events = unsafe { libc::kqueue() };
            if events < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: events is a newly created and valid descriptor.
            let events = unsafe { OwnedFd::from_raw_fd(events) };
            // The observer must stay alive across exec and receive status from the kernel.
            let event = libc::kevent64_s {
                ident: child.pid.into(),
                filter: libc::EVFILT_PROC,
                flags: libc::EV_ADD | libc::EV_ENABLE | libc::EV_CLEAR,
                fflags: libc::NOTE_EXEC | libc::NOTE_EXIT | libc::NOTE_EXITSTATUS,
                data: 0,
                udata: 0,
                ext: [0, 0],
            };
            // SAFETY: event describes one registration and no event output is requested.
            if unsafe {
                libc::kevent64(
                    events.as_raw_fd(),
                    &event,
                    1,
                    std::ptr::null_mut(),
                    0,
                    0,
                    std::ptr::null(),
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            let watch = Self {
                parent,
                child,
                lock_fd,
                lock,
                lock_offset: offset,
                events,
            };
            watch.current_child(libc::SSTOP)?;
            Ok(watch)
        }

        pub fn child(&self) -> &ProcessInstance {
            &self.child
        }

        pub fn stopped_image(&self) -> io::Result<ForegroundImage> {
            self.current_child(libc::SSTOP)?;
            let image = image(self.child.pid as i32, None)?;
            self.current_child(libc::SSTOP)?;
            Ok(image)
        }

        pub fn wait(
            self,
            limit: Duration,
            writer_program: &std::path::Path,
            environment_count: usize,
            mut admit_exec: impl FnMut(ForegroundImage) -> Option<ForegroundExec>,
            mut cancelled: impl FnMut() -> bool,
        ) -> io::Result<ProcessInstance> {
            self.current_child(libc::SSTOP)?;
            let pid = self.child.pid as i32;
            // SAFETY: the just-checked stopped child still has the expected guide and instance.
            if unsafe { libc::kill(pid, libc::SIGCONT) } != 0 {
                return Err(io::Error::last_os_error());
            }
            let deadline = Instant::now() + limit;
            let mut admitted = false;
            let mut handoff = false;
            loop {
                self.check_guide(&mut cancelled, deadline)?;
                let event = self.event()?;
                if let Some(event) = event {
                    if event.fflags & libc::NOTE_EXEC != 0 {
                        if event.fflags & libc::NOTE_EXIT != 0 {
                            return Err(invalid("foreground executable observation was ambiguous"));
                        }
                        self.current_child(0)?;
                        let image = image(pid, Some((writer_program, environment_count)))?;
                        self.current_child(0)?;
                        match admit_exec(image) {
                            Some(ForegroundExec::Handoff) if !handoff && !admitted => {
                                handoff = true;
                            }
                            Some(ForegroundExec::Writer) if !admitted => {
                                admitted = true;
                            }
                            Some(ForegroundExec::Writer) if !handoff => {
                                // The exec notification for env can be read after Cargo already exec'd.
                                handoff = true;
                            }
                            _ => {
                                return Err(invalid(
                                    "actual foreground exec differs from reviewed Cargo",
                                ));
                            }
                        }
                    }
                    if event.fflags & libc::NOTE_EXIT != 0 {
                        if !admitted || event.fflags & libc::NOTE_EXITSTATUS == 0 || event.data != 0
                        {
                            return Err(invalid(
                                "foreground Cargo did not have an observed successful exit",
                            ));
                        }
                        break;
                    }
                } else if let Ok(info) = process(pid)
                    && info.pbi_status != libc::SZOMB
                    && (!same_child(&info, &self.parent, self.child.pid)
                        || !matches_instance(&info, &self.child)
                        || !owns_lock(pid, self.lock_fd, self.lock, Some(self.lock_offset))?)
                {
                    return Err(invalid("foreground writer lost its inherited lock custody"));
                }
            }
            let quiescence = deadline.min(Instant::now() + Duration::from_secs(2));
            loop {
                self.check_guide(&mut cancelled, quiescence)?;
                // SAFETY: signal 0 only tests whether the observed child's process group exists.
                let result = unsafe { libc::kill(-pid, 0) };
                if result != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if other_lock_holder(self.parent.pid, self.lock_fd, self.lock)? {
                return Err(invalid(
                    "another process can still write with the inherited slot lock",
                ));
            }
            self.check_guide(&mut cancelled, deadline)?;
            Ok(self.child)
        }

        fn current_child(&self, status: u32) -> io::Result<()> {
            let info = process(self.child.pid as i32)?;
            if !same_child(&info, &self.parent, self.child.pid)
                || !matches_instance(&info, &self.child)
                || (status != 0 && status != info.pbi_status)
                || info.pbi_status == libc::SZOMB
                || !owns_lock(
                    self.child.pid as i32,
                    self.lock_fd,
                    self.lock,
                    Some(self.lock_offset),
                )?
            {
                return Err(invalid(
                    "foreground writer identity or lock custody changed",
                ));
            }
            Ok(())
        }

        fn check_guide(
            &self,
            cancelled: &mut impl FnMut() -> bool,
            deadline: Instant,
        ) -> io::Result<()> {
            if cancelled() {
                return Err(io::ErrorKind::Interrupted.into());
            }
            if Instant::now() >= deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            if parent_instance()? != self.parent {
                return Err(invalid("foreground guide is no longer the recorded runner"));
            }
            Ok(())
        }

        fn event(&self) -> io::Result<Option<libc::kevent64_s>> {
            let mut event = std::mem::MaybeUninit::zeroed();
            let timeout = libc::timespec {
                tv_sec: 0,
                tv_nsec: 50_000_000,
            };
            // SAFETY: there are no registrations to read. One event can be written to event.
            match unsafe {
                libc::kevent64(
                    self.events.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    event.as_mut_ptr(),
                    1,
                    0,
                    &timeout,
                )
            } {
                0 => Ok(None),
                1 => {
                    // SAFETY: kevent64 reported writing exactly one event.
                    let event = unsafe { event.assume_init() };
                    if event.ident != u64::from(self.child.pid) || event.flags & libc::EV_ERROR != 0
                    {
                        return Err(invalid(
                            "foreground process event did not match the watched child",
                        ));
                    }
                    Ok(Some(event))
                }
                _ => {
                    let err = io::Error::last_os_error();
                    if err.kind() == io::ErrorKind::Interrupted {
                        Ok(None)
                    } else {
                        Err(err)
                    }
                }
            }
        }
    }

    fn invalid(message: &'static str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, message)
    }

    fn process(pid: i32) -> io::Result<libc::proc_bsdinfo> {
        process_info(pid, libc::PROC_PIDTBSDINFO)
    }

    fn process_info<T>(pid: i32, flavor: i32) -> io::Result<T> {
        let mut info = std::mem::MaybeUninit::<T>::zeroed();
        // SAFETY: callers select a flavor with the matching Darwin ABI structure T.
        let read = unsafe {
            libc::proc_pidinfo(
                pid,
                flavor,
                0,
                info.as_mut_ptr().cast(),
                size_of::<T>() as i32,
            )
        };
        if read != size_of::<T>() as i32 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call populated exactly one complete structure.
        Ok(unsafe { info.assume_init() })
    }

    fn same_child(info: &libc::proc_bsdinfo, parent: &ProcessInstance, child: u32) -> bool {
        info.pbi_ppid == parent.pid
            && info.pbi_pid == child
            && info.pbi_pgid == child
            && info.pbi_uid == effective_uid()
            && info.pbi_start_tvsec != 0
            && info.pbi_start_tvusec < 1_000_000
    }

    fn matches_instance(info: &libc::proc_bsdinfo, instance: &ProcessInstance) -> bool {
        info.pbi_start_tvsec == instance.start_secs
            && info.pbi_start_tvusec == instance.start_micros
    }

    fn owns_lock(
        pid: i32,
        fd: i32,
        lock: LockFileIdentity,
        offset: Option<i64>,
    ) -> io::Result<bool> {
        let mut info = std::mem::MaybeUninit::<VnodeFdInfo>::zeroed();
        // SAFETY: info is writable for the size of the Darwin vnode_fdinfo structure.
        let read = unsafe {
            libc::proc_pidfdinfo(
                pid,
                fd,
                PROC_PIDFDVNODEINFO,
                info.as_mut_ptr().cast(),
                size_of::<VnodeFdInfo>() as i32,
            )
        };
        if read != size_of::<VnodeFdInfo>() as i32 {
            let err = io::Error::last_os_error();
            return match err.raw_os_error() {
                Some(libc::EBADF | libc::ESRCH | libc::ENOTSUP | libc::EINVAL | libc::ENOENT) => {
                    Ok(false)
                }
                _ => Err(err),
            };
        }
        // SAFETY: proc_pidfdinfo returned the full vnode_fdinfo structure.
        let info = unsafe { info.assume_init() };
        Ok(info.status & PROC_FP_CLEXEC == 0
            && offset.is_none_or(|expected| expected == info.offset)
            && u64::from(info.vnode.vi_stat.vst_dev) == lock.dev
            && info.vnode.vi_stat.vst_ino == lock.ino)
    }

    fn image(
        pid: i32,
        writer_environment: Option<(&std::path::Path, usize)>,
    ) -> io::Result<ForegroundImage> {
        let cwd: libc::proc_vnodepathinfo = process_info(pid, libc::PROC_PIDVNODEPATHINFO)?;
        let cwd = cwd
            .pvi_cdir
            .vip_path
            .into_iter()
            .flatten()
            .map(|byte| byte as u8)
            .take_while(|byte| *byte != 0)
            .collect::<Vec<_>>();
        let mut bytes = vec![0u8; ARGUMENT_LIMIT];
        let mut len = bytes.len();
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        // SAFETY: the MIB is valid, and bytes is writable for the in/out len bound.
        if unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as u32,
                bytes.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        if len <= size_of::<i32>() || len >= ARGUMENT_LIMIT {
            return Err(invalid("invalid foreground argument data"));
        }
        let argc = i32::from_ne_bytes(
            bytes[..size_of::<i32>()]
                .try_into()
                .map_err(|_| invalid("invalid foreground argument count"))?,
        );
        if !(1..=336).contains(&argc) {
            return Err(invalid("invalid foreground argument count"));
        }
        let tail = &bytes[size_of::<i32>()..len];
        let end = tail
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| invalid("missing foreground exec path"))?;
        let program = PathBuf::from(OsString::from_vec(tail[..end].to_vec()));
        let args = tail[end..]
            .iter()
            .position(|byte| *byte != 0)
            .ok_or_else(|| invalid("missing foreground argument data"))?;
        let mut parts = tail[end + args..].split(|byte| *byte == 0);
        let argv = parts
            .by_ref()
            .take(argc as usize)
            .map(|arg| OsString::from_vec(arg.to_vec()))
            .collect::<Vec<_>>();
        if argv.len() != argc as usize || !program.is_absolute() || cwd.first() != Some(&b'/') {
            return Err(invalid(
                "incomplete foreground executable or argument proof",
            ));
        }
        let environment = parse_environment(
            parts,
            writer_environment.and_then(|(writer, count)| (writer == program).then_some(count)),
        )?;
        Ok(ForegroundImage {
            program,
            cwd: PathBuf::from(OsString::from_vec(cwd)),
            argv,
            environment,
        })
    }

    fn parse_environment<'a>(
        mut parts: impl Iterator<Item = &'a [u8]>,
        environment_count: Option<usize>,
    ) -> io::Result<std::collections::BTreeMap<OsString, OsString>> {
        let mut environment = std::collections::BTreeMap::new();
        let Some(count) = environment_count else {
            return Ok(environment);
        };
        for value in parts.by_ref().take(count) {
            if value.is_empty() {
                return Err(invalid("the foreground environment is incomplete"));
            }
            let equal = value
                .iter()
                .position(|byte| *byte == b'=')
                .ok_or_else(|| invalid("invalid foreground environment entry"))?;
            if equal == 0
                || environment
                    .insert(
                        OsString::from_vec(value[..equal].to_vec()),
                        OsString::from_vec(value[equal + 1..].to_vec()),
                    )
                    .is_some()
            {
                return Err(invalid("duplicate or invalid foreground environment entry"));
            }
        }
        if environment.len() != count {
            return Err(invalid("the foreground environment is incomplete"));
        }
        validate_bootstrap_tail(parts)?;
        Ok(environment)
    }

    fn validate_bootstrap_tail<'a>(parts: impl Iterator<Item = &'a [u8]>) -> io::Result<()> {
        // During exec, applev may immediately follow envp. Dyld later clears some
        // bootstrap strings, leaving zero-filled gaps among the remaining entries.
        let mut keys = std::collections::BTreeSet::new();
        let mut parts = parts.peekable();
        let delimited = parts.peek().is_some_and(|field| field.is_empty());
        let mut terminated = false;
        for field in parts {
            terminated = field.is_empty();
            if terminated {
                continue;
            }
            let equal = field
                .iter()
                .position(|byte| *byte == b'=')
                .ok_or_else(|| invalid("unexplained foreground process argument tail"))?;
            let key = &field[..equal];
            if !matches!(
                key,
                b"executable_path"
                    | b"executable_file"
                    | b"executable_boothash"
                    | b"executable_cdhash"
                    | b"dyld_file"
                    | b"main_stack"
                    | b"stack_guard"
                    | b"malloc_entropy"
                    | b"security_config"
                    | b"ptr_munge"
                    | b"th_port"
                    | b"pfz"
            ) || !keys.insert(key)
            {
                return Err(invalid("unexplained foreground process argument tail"));
            }
        }
        if !terminated
            || (!delimited
                && ![
                    b"executable_file".as_slice(),
                    b"dyld_file",
                    b"main_stack",
                    b"stack_guard",
                    b"malloc_entropy",
                ]
                .into_iter()
                .all(|key| keys.contains(key)))
        {
            return Err(invalid("incomplete Darwin bootstrap argument tail"));
        }
        Ok(())
    }

    fn other_lock_holder(parent: u32, fd: i32, lock: LockFileIdentity) -> io::Result<bool> {
        let mut pids = vec![0i32; PIDS_LIMIT];
        // SAFETY: pids is writable for the supplied array size, in bytes.
        let count = unsafe {
            libc::proc_listpids(
                PROC_UID_ONLY,
                effective_uid(),
                pids.as_mut_ptr().cast(),
                (pids.len() * size_of::<i32>()) as i32,
            )
        };
        if count <= 0 || count as usize >= pids.len() * size_of::<i32>() {
            return Err(invalid("could not verify complete slot custody"));
        }
        for &pid in &pids[..count as usize / size_of::<i32>()] {
            if pid > 0
                && pid as u32 != parent
                && pid as u32 != std::process::id()
                && owns_lock(pid, fd, lock, None)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    #[cfg(test)]
    mod tests {
        use super::parse_environment;

        #[test]
        fn exact_environment_does_not_hide_a_trailing_variable() {
            let parts = [
                b"HOME=/private/tmp".as_slice(),
                b"UNREVIEWED=unexpected",
                b"",
            ];
            assert!(parse_environment(parts.into_iter(), Some(1)).is_err());
            let separated = [
                b"HOME=/private/tmp".as_slice(),
                b"",
                b"UNREVIEWED=unexpected",
                b"",
            ];
            assert!(parse_environment(separated.into_iter(), Some(1)).is_err());
        }

        #[test]
        fn darwin_bootstrap_record_is_outside_the_cargo_environment() {
            let parts = [
                b"HOME=/private/tmp".as_slice(),
                b"executable_file=test",
                b"dyld_file=test",
                b"main_stack=test",
                b"stack_guard=test",
                b"malloc_entropy=test",
                b"",
            ];
            let values = parse_environment(parts.into_iter(), Some(1)).unwrap();
            assert_eq!(values.len(), 1);
            assert!(values.contains_key(std::ffi::OsStr::new("HOME")));
            let residual = [b"HOME=/private/tmp".as_slice(), b"", b"", b"pfz=test", b""];
            assert_eq!(
                parse_environment(residual.into_iter(), Some(1)).unwrap(),
                values
            );
        }
    }
}

#[cfg(target_os = "macos")]
pub use foreground::ForegroundWatch;

#[cfg(not(target_os = "macos"))]
pub struct ForegroundWatch {
    child: ProcessInstance,
}

#[cfg(not(target_os = "macos"))]
impl ForegroundWatch {
    pub fn start(
        _parent: ProcessInstance,
        _child: u32,
        _lock_fd: i32,
        _lock: crate::flock::LockFileIdentity,
        _inherited: std::os::fd::BorrowedFd<'_>,
    ) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            MACOS_ONLY_MESSAGE,
        ))
    }

    pub fn child(&self) -> &ProcessInstance {
        &self.child
    }

    pub fn stopped_image(&self) -> io::Result<ForegroundImage> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            MACOS_ONLY_MESSAGE,
        ))
    }

    pub fn wait(
        self,
        _limit: std::time::Duration,
        _writer_program: &std::path::Path,
        _environment_count: usize,
        _admit_exec: impl FnMut(ForegroundImage) -> Option<ForegroundExec>,
        _cancelled: impl FnMut() -> bool,
    ) -> io::Result<ProcessInstance> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            MACOS_ONLY_MESSAGE,
        ))
    }
}
