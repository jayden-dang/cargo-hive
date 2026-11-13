//! Child-process spawn: `Spec` plus bounded and unbounded runners.

use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

/// Program, args, cwd, and environment edits for a child process.
#[derive(Debug, Clone)]
pub struct Spec {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub env_clear: bool,
    pub env_set: Vec<(OsString, OsString)>,
    pub env_remove: Vec<OsString>,
    pub stdin_null: bool,
    pub own_process_group: bool,
}

impl Spec {
    pub fn new(program: impl Into<OsString>) -> Spec {
        Spec {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env_clear: false,
            env_set: Vec::new(),
            env_remove: Vec::new(),
            stdin_null: false,
            own_process_group: false,
        }
    }
}

fn command(spec: &Spec) -> Command {
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args);
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    if spec.env_clear {
        cmd.env_clear();
    }
    for (key, value) in &spec.env_set {
        cmd.env(key, value);
    }
    for key in &spec.env_remove {
        cmd.env_remove(key);
    }
    if spec.stdin_null {
        cmd.stdin(Stdio::null());
    }
    if spec.own_process_group {
        cmd.process_group(0);
    }
    cmd
}

/// Spawns from `spec`, applying its stdin / process-group flags, with the given stdio.
/// Platform-internal: lock holders need a live `Child` the public runners do not return.
pub(super) fn spawn(spec: &Spec, stdout: Stdio, stderr: Stdio) -> io::Result<Child> {
    command(spec).stdout(stdout).stderr(stderr).spawn()
}

#[derive(Debug)]
pub enum ChildOutputError {
    TimedOut,
    Cancelled,
    Failed(String),
}

impl fmt::Display for ChildOutputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChildOutputError::TimedOut => write!(f, "child process exceeded its deadline"),
            ChildOutputError::Cancelled => write!(f, "child process was cancelled"),
            ChildOutputError::Failed(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ChildOutputError {}

fn kill_pgid(pid: u32) {
    // SAFETY: the child leads its own process group; this signals only that group.
    unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
}

fn kill_group(child: &Child) {
    kill_pgid(child.id());
}

/// How long the pipe readers may take, once the child has exited or been killed, to
/// reach end of file. A process outside the child's group that holds a pipe's write end
/// (a lingering grandchild, or a child another thread was spawning while the pipe still
/// lacked close-on-exec, since macOS has no `pipe2`) would otherwise hold the caller
/// until it exits.
const PIPE_DRAIN_GRACE: Duration = Duration::from_secs(1);

/// One pipe read into a shared buffer, so the bytes read so far survive a bounded drain.
struct PipeReader {
    buf: Arc<Mutex<Vec<u8>>>,
    done: mpsc::Receiver<()>,
}

impl PipeReader {
    fn spawn(pipe: Option<impl Read + Send + 'static>) -> PipeReader {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let (tx, done) = mpsc::channel();
        if let Some(mut pipe) = pipe {
            let shared = Arc::clone(&buf);
            thread::spawn(move || {
                let mut chunk = [0u8; 8192];
                loop {
                    match pipe.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => shared
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .extend_from_slice(&chunk[..n]),
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
                let _ = tx.send(());
            });
        } else {
            let _ = tx.send(());
        }
        PipeReader { buf, done }
    }

    /// Waits for end of file until `until`, then takes what was read.
    fn take(self, until: Instant) -> Vec<u8> {
        let _ = self
            .done
            .recv_timeout(until.saturating_duration_since(Instant::now()));
        std::mem::take(&mut *self.buf.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

struct PipeReaders {
    stdout: PipeReader,
    stderr: PipeReader,
}

impl PipeReaders {
    /// Both pipes, each given until one grace period after now to reach end of file.
    fn finish(self) -> (Vec<u8>, Vec<u8>) {
        let until = Instant::now() + PIPE_DRAIN_GRACE;
        (self.stdout.take(until), self.stderr.take(until))
    }
}

/// Runs `spec` with piped stdout and stderr. A deadline or a cancel kills the
/// process group. After a normal exit the pipe readers get `PIPE_DRAIN_GRACE` to reach
/// end of file; on expiry, cancel or error the output is dropped at once. Either way a
/// reader still blocked is left behind and ends at end of file.
///
/// Always nulls stdin and puts the child in its own process group, as 0.1.0 did.
pub fn output_with_deadline(
    spec: &Spec,
    deadline: Duration,
    mut cancel: impl FnMut() -> bool,
) -> Result<Output, ChildOutputError> {
    let mut child = command(spec)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| ChildOutputError::Failed(format!("could not spawn: {e}")))?;
    let readers = PipeReaders {
        stdout: PipeReader::spawn(child.stdout.take()),
        stderr: PipeReader::spawn(child.stderr.take()),
    };
    let started = Instant::now();
    let status = loop {
        if cancel() {
            kill_group(&child);
            let _ = child.wait();
            return Err(ChildOutputError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(retry) if retry.kind() == io::ErrorKind::Interrupted => {
                    if cancel() {
                        kill_group(&child);
                        let _ = child.wait();
                        return Err(ChildOutputError::Cancelled);
                    }
                }
                Err(retry) => {
                    kill_group(&child);
                    let _ = child.wait();
                    return Err(ChildOutputError::Failed(retry.to_string()));
                }
            },
            Err(e) => {
                kill_group(&child);
                let _ = child.wait();
                return Err(ChildOutputError::Failed(e.to_string()));
            }
        }
        if started.elapsed() >= deadline {
            kill_group(&child);
            let _ = child.wait();
            return Err(ChildOutputError::TimedOut);
        }
        thread::sleep(Duration::from_millis(20));
    };
    let (stdout, stderr) = readers.finish();
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

pub struct Capture {
    pub stdout_cap: usize,
    pub stderr_cap: usize,
}

#[derive(Debug)]
pub struct Ran {
    pub status: ExitStatus,
    pub exited_after: Duration,
    pub stdout: Vec<u8>,
    pub stderr_tail: Vec<u8>,
}

/// Runs `spec` to a deadline with capped stdout (prefix) and stderr (tail). A
/// cancel or the deadline kills the process group. `exited_after` is stamped
/// when `wait` returns, before reader joins.
///
/// Always nulls stdin and puts the child in its own process group, as 0.1.0 did.
pub fn run_bounded(
    spec: &Spec,
    deadline: Duration,
    capture: Capture,
    mut cancel: impl FnMut() -> bool,
) -> Result<Ran, ChildOutputError> {
    let stdout_stdio = if capture.stdout_cap == 0 {
        Stdio::null()
    } else {
        Stdio::piped()
    };
    let stderr_stdio = if capture.stderr_cap == 0 {
        Stdio::null()
    } else {
        Stdio::piped()
    };
    let start = Instant::now();
    let mut child = command(spec)
        .stdin(Stdio::null())
        .stdout(stdout_stdio)
        .stderr(stderr_stdio)
        .process_group(0)
        .spawn()
        .map_err(|e| ChildOutputError::Failed(format!("could not spawn: {e}")))?;
    let pid = child.id();
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_rx = match stdout_pipe {
        Some(mut pipe) => {
            let cap = capture.stdout_cap;
            let (tx, rx) = mpsc::channel();
            thread::spawn(move || {
                let _ = tx.send(read_stdout_prefix(&mut pipe, cap));
            });
            Some(rx)
        }
        None => None,
    };
    let stderr_rx = match stderr_pipe {
        Some(mut pipe) => {
            let cap = capture.stderr_cap;
            let (tx, rx) = mpsc::channel();
            thread::spawn(move || {
                let _ = tx.send(read_stderr_tail(&mut pipe, cap));
            });
            Some(rx)
        }
        None => None,
    };

    let (wait_tx, wait_rx) = mpsc::channel();
    thread::spawn(move || {
        let status = child.wait();
        let exited_after = start.elapsed();
        let _ = wait_tx.send((status, exited_after));
    });

    let slice = Duration::from_millis(20);
    let (status, exited_after) = loop {
        match wait_rx.recv_timeout(slice) {
            Ok((Ok(status), exited_after)) => break (status, exited_after),
            Ok((Err(e), _)) => {
                finish_readers(stdout_rx, stderr_rx);
                return Err(ChildOutputError::Failed(e.to_string()));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if cancel() {
                    kill_pgid(pid);
                    let _ = wait_rx.recv();
                    finish_readers(stdout_rx, stderr_rx);
                    return Err(ChildOutputError::Cancelled);
                }
                if start.elapsed() >= deadline {
                    kill_pgid(pid);
                    let _ = wait_rx.recv();
                    finish_readers(stdout_rx, stderr_rx);
                    return Err(ChildOutputError::TimedOut);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                finish_readers(stdout_rx, stderr_rx);
                return Err(ChildOutputError::Failed(
                    "waiter thread disconnected before exit".into(),
                ));
            }
        }
    };

    let (stdout, stderr_tail) = finish_readers(stdout_rx, stderr_rx);
    Ok(Ran {
        status,
        exited_after,
        stdout,
        stderr_tail,
    })
}

/// Runs `spec` with no deadline, honouring its stdin and process-group flags.
pub fn output(spec: &Spec) -> io::Result<Output> {
    command(spec).output()
}

fn read_stdout_prefix(pipe: &mut impl Read, cap: usize) -> Vec<u8> {
    let mut kept = Vec::with_capacity(cap.min(8192));
    let mut buf = [0u8; 8192];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if kept.len() < cap {
                    let take = (cap - kept.len()).min(n);
                    kept.extend_from_slice(&buf[..take]);
                }
            }
            Err(_) => break,
        }
    }
    kept
}

fn read_stderr_tail(pipe: &mut impl Read, cap: usize) -> Vec<u8> {
    let mut ring = Vec::with_capacity(cap.min(8192));
    let mut buf = [0u8; 8192];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => push_tail(&mut ring, cap, &buf[..n]),
            Err(_) => break,
        }
    }
    ring
}

fn push_tail(ring: &mut Vec<u8>, cap: usize, data: &[u8]) {
    if cap == 0 || data.is_empty() {
        return;
    }
    if data.len() >= cap {
        ring.clear();
        ring.extend_from_slice(&data[data.len() - cap..]);
        return;
    }
    let need = ring.len() + data.len();
    if need > cap {
        ring.drain(..need - cap);
    }
    ring.extend_from_slice(data);
}

fn finish_readers(
    stdout_rx: Option<mpsc::Receiver<Vec<u8>>>,
    stderr_rx: Option<mpsc::Receiver<Vec<u8>>>,
) -> (Vec<u8>, Vec<u8>) {
    let grace = Duration::from_secs(1);
    let stdout = match stdout_rx {
        Some(rx) => rx.recv_timeout(grace).unwrap_or_default(),
        None => Vec::new(),
    };
    let stderr = match stderr_rx {
        Some(rx) => rx.recv_timeout(grace).unwrap_or_default(),
        None => Vec::new(),
    };
    (stdout, stderr)
}

#[cfg(test)]
mod tests {
    use super::{Capture, ChildOutputError, Spec, output, output_with_deadline, run_bounded};
    use std::ffi::OsString;
    use std::time::{Duration, Instant};

    #[test]
    fn a_short_deadline_times_out_a_sleeping_child() {
        let mut spec = Spec::new("sleep");
        spec.args.push(OsString::from("5"));
        let err = run_bounded(
            &spec,
            Duration::from_millis(100),
            Capture {
                stdout_cap: 0,
                stderr_cap: 0,
            },
            || false,
        )
        .expect_err("sleep 5 must not finish inside 100 ms");
        assert!(matches!(err, ChildOutputError::TimedOut), "{err:?}");
    }

    #[test]
    fn output_honours_env_clear_and_env_set() {
        let mut spec = Spec::new("sh");
        spec.args = vec![OsString::from("-c"), OsString::from("echo $X")];
        spec.env_clear = true;
        spec.env_set
            .push((OsString::from("X"), OsString::from("1")));
        let out = output(&spec).expect("sh -c echo");
        assert!(out.status.success(), "{out:?}");
        assert_eq!(out.stdout, b"1\n");
    }

    #[test]
    fn env_remove_leaves_the_variable_unset_in_the_child() {
        let mut spec = Spec::new("sh");
        spec.args = vec![OsString::from("-c"), OsString::from("echo ${HOME-unset}")];
        spec.env_remove.push(OsString::from("HOME"));
        let out = output(&spec).expect("sh -c HOME probe");
        assert!(out.status.success(), "{out:?}");
        assert_eq!(out.stdout, b"unset\n");
    }

    #[test]
    fn a_short_deadline_kills_a_sleeping_child() {
        let mut spec = Spec::new("sleep");
        spec.args.push(OsString::from("30"));
        let started = Instant::now();
        let err = output_with_deadline(&spec, Duration::from_millis(200), || false)
            .expect_err("a 30s sleep must not finish inside the deadline");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "sleep was still running after {:?}",
            started.elapsed()
        );
        assert!(matches!(err, ChildOutputError::TimedOut), "{err:?}");
    }

    #[test]
    fn a_cancel_kills_the_child_instead_of_waiting_out_the_deadline() {
        let mut spec = Spec::new("sleep");
        spec.args.push(OsString::from("30"));
        let started = Instant::now();
        let err = output_with_deadline(&spec, Duration::from_secs(60), || true)
            .expect_err("cancel stops the child");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(matches!(err, ChildOutputError::Cancelled), "{err:?}");
    }

    /// A 20 s process in its own process group, so a group kill does not reach it.
    const LEAVE_GROUP: &str = "perl -e 'setpgrp(0, 0); exec qw(sleep 20)'";

    fn sh(script: &str) -> Spec {
        let mut spec = Spec::new("/bin/sh");
        spec.args = vec![OsString::from("-c"), OsString::from(script)];
        spec
    }

    #[test]
    fn a_child_that_exits_reports_its_status_and_when_it_exited() {
        let ran = run_bounded(
            &sh("sleep 0.2; exit 3"),
            Duration::from_secs(10),
            Capture {
                stdout_cap: 0,
                stderr_cap: 1024,
            },
            || false,
        )
        .unwrap();
        assert_eq!(ran.status.code(), Some(3));
        assert!(
            ran.exited_after >= Duration::from_millis(200),
            "{:?}",
            ran.exited_after
        );
        assert!(ran.exited_after < Duration::from_secs(5));
    }

    #[test]
    fn output_past_its_cap_keeps_only_the_cap() {
        let ran = run_bounded(
            &sh("head -c 100000 /dev/zero; head -c 100000 /dev/zero >&2"),
            Duration::from_secs(10),
            Capture {
                stdout_cap: 4096,
                stderr_cap: 4096,
            },
            || false,
        )
        .unwrap();
        assert_eq!(ran.stdout.len(), 4096);
        assert_eq!(ran.stderr_tail.len(), 4096);
    }

    #[test]
    fn output_at_its_cap_is_kept_whole() {
        let ran = run_bounded(
            &sh("head -c 4096 /dev/zero >&2"),
            Duration::from_secs(10),
            Capture {
                stdout_cap: 0,
                stderr_cap: 4096,
            },
            || false,
        )
        .unwrap();
        assert_eq!(ran.stderr_tail.len(), 4096);
    }

    #[test]
    fn a_child_past_its_deadline_is_killed_with_its_group() {
        let start = Instant::now();
        let err = run_bounded(
            &sh("sleep 30 & sleep 30"),
            Duration::from_millis(300),
            Capture {
                stdout_cap: 0,
                stderr_cap: 0,
            },
            || false,
        )
        .unwrap_err();
        assert!(matches!(err, ChildOutputError::TimedOut));
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_cancel_kills_the_child() {
        let start = Instant::now();
        let err = run_bounded(
            &sh("sleep 30"),
            Duration::from_secs(60),
            Capture {
                stdout_cap: 0,
                stderr_cap: 0,
            },
            || start.elapsed() > Duration::from_millis(200),
        )
        .unwrap_err();
        assert!(matches!(err, ChildOutputError::Cancelled));
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_process_holding_the_pipes_open_does_not_delay_output_with_deadline() {
        // The grandchild leaves the child's process group, as a process that inherited
        // the pipe from a concurrent spawn does, so only a bounded drain returns early.
        let spec = sh(&format!(
            "printf out; printf err >&2; {LEAVE_GROUP} & exit 0"
        ));
        let start = Instant::now();
        let out = output_with_deadline(&spec, Duration::from_secs(60), || false).unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(out.stdout, b"out", "the bytes read before exit are kept");
        assert_eq!(out.stderr, b"err");
    }

    #[test]
    fn an_expired_deadline_does_not_wait_for_a_process_holding_the_pipes() {
        let spec = sh(&format!("{LEAVE_GROUP} & sleep 30"));
        let start = Instant::now();
        let err = output_with_deadline(&spec, Duration::from_millis(200), || false).unwrap_err();
        assert!(matches!(err, ChildOutputError::TimedOut), "{err:?}");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn a_grandchild_holding_stderr_open_does_not_delay_the_result() {
        let start = Instant::now();
        let ran = run_bounded(
            &sh("(sleep 20 >&2 &) ; exit 0"),
            Duration::from_secs(60),
            Capture {
                stdout_cap: 0,
                stderr_cap: 1024,
            },
            || false,
        )
        .unwrap();
        assert_eq!(ran.status.code(), Some(0));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    fn assert_error<E: std::error::Error>(_: &E) {}

    #[test]
    fn child_output_errors_display_what_went_wrong() {
        let cancelled = ChildOutputError::Cancelled;
        let timed_out = ChildOutputError::TimedOut;
        let failed = ChildOutputError::Failed("could not spawn: boom".into());
        assert_error(&cancelled);
        assert_error(&timed_out);
        assert_error(&failed);
        assert_eq!(cancelled.to_string(), "child process was cancelled");
        assert_eq!(timed_out.to_string(), "child process exceeded its deadline");
        assert_eq!(failed.to_string(), "could not spawn: boom");
        assert!(std::error::Error::source(&cancelled).is_none());
    }
}
