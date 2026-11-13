//! hive's own state under `~/.hive/` (or `$HIVE_HOME`, test-only). This module is
//! the only code that writes hive's files; every write is a whole-file atomic
//! replace taken under one `flock`, so a kill never leaves a half-written file.

use cargo_hive_platform::clock::utc_date;
use cargo_hive_platform::flock::{self, FlockError, FlockGuard};
use serde::{Serialize, de::DeserializeOwned};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug)]
pub enum StateError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Malformed {
        file: PathBuf,
        detail: String,
    },
    /// A write that was not stored because the encoded file would pass its cap.
    /// The file already on disk is unchanged.
    Oversized {
        file: PathBuf,
        max: u64,
    },
    NoHome,
    /// The caller cancelled while another process held the state lock.
    Cancelled,
    DuplicatePlan {
        id: String,
    },
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            StateError::Malformed { file, detail } => {
                write!(
                    f,
                    "{} is malformed ({detail}); fix or remove it by hand",
                    file.display()
                )
            }
            StateError::Oversized { file, max } => {
                write!(
                    f,
                    "{} would be larger than {max} bytes; plan a smaller set of repositories",
                    file.display()
                )
            }
            StateError::NoHome => write!(f, "HOME is not set, so ~/.hive cannot be located"),
            StateError::Cancelled => {
                write!(
                    f,
                    "cancelled while another hive process held the state lock"
                )
            }
            StateError::DuplicatePlan { id } => {
                write!(f, "plan id {id} collided; run `cargo hive gc --plan` again")
            }
        }
    }
}

impl std::error::Error for StateError {}

fn io_err(path: &Path, source: io::Error) -> StateError {
    StateError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// A state file name is one relative component. Absolute names, `..`, and
/// separators would let `Path::join` write outside the hive root.
fn state_file(root: &Path, name: &str) -> Result<PathBuf, StateError> {
    if name.is_empty()
        || name.contains("..")
        || name.contains('/')
        || name.contains('\\')
        || Path::new(name).is_absolute()
    {
        return Err(io_err(
            &root.join(name),
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "a state file name must be one relative component",
            ),
        ));
    }
    Ok(root.join(name))
}

pub struct State {
    root: PathBuf,
}

impl State {
    pub fn open() -> Result<State, StateError> {
        let root = match std::env::var_os("HIVE_HOME") {
            Some(dir) => PathBuf::from(dir),
            None => {
                PathBuf::from(std::env::var_os("HOME").ok_or(StateError::NoHome)?).join(".hive")
            }
        };
        State::open_at(root)
    }

    pub fn open_at(root: PathBuf) -> Result<State, StateError> {
        let receipts = root.join("receipts");
        fs::create_dir_all(&receipts).map_err(|e| io_err(&receipts, e))?;
        Ok(State { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn read<T: DeserializeOwned + Default>(&self, name: &str) -> Result<T, StateError> {
        read_json(&state_file(&self.root, name)?)
    }

    /// Reads at most `max` bytes and takes no state lock. A writer replaces the
    /// file by rename, so this sees the previous file or the new one.
    pub fn read_bounded<T: DeserializeOwned + Default>(
        &self,
        name: &str,
        max: u64,
    ) -> Result<T, StateError> {
        read_json_limited(&state_file(&self.root, name)?, max)
    }

    /// Runs `body` while holding the exclusive state lock (`$root/.lock`).
    pub fn locked<T>(
        &self,
        body: impl FnOnce(&Tx<'_>) -> Result<T, StateError>,
    ) -> Result<T, StateError> {
        self.locked_or_cancelled(|| false, body)
    }

    /// Like `locked`, but while another process holds the lock it polls `cancel` and
    /// returns `StateError::Cancelled` once it is true. A free lock is taken even
    /// after cancellation, so a cancelled command can still log its receipt.
    pub fn locked_or_cancelled<T>(
        &self,
        cancel: impl FnMut() -> bool,
        body: impl FnOnce(&Tx<'_>) -> Result<T, StateError>,
    ) -> Result<T, StateError> {
        let path = self.root.join(".lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| io_err(&path, e))?;
        let _guard = flock::exclusive_or_cancelled(&file, cancel).map_err(|e| match e {
            FlockError::Cancelled => StateError::Cancelled,
            FlockError::Io(err) => io_err(&path, err),
        })?;
        body(&Tx { root: &self.root })
    }

    /// Takes a shared flock on `$root/store.lock`, polling `cancel` while another
    /// process holds it exclusive. Creates the file when absent. Releases on drop.
    pub fn store_shared_or_cancelled(
        &self,
        cancel: impl FnMut() -> bool,
    ) -> Result<StoreLock, StateError> {
        let path = self.root.join("store.lock");
        let file = self.open_store_lock_file(&path)?;
        let guard = flock::shared_or_cancelled(&file, cancel).map_err(|e| match e {
            FlockError::Cancelled => StateError::Cancelled,
            FlockError::Io(err) => io_err(&path, err),
        })?;
        Ok(StoreLock {
            _guard: guard,
            _file: file,
        })
    }

    /// Tries an exclusive flock on `$root/store.lock` once. Creates the file when
    /// absent. `Ok(None)` when another process holds a shared or exclusive lock.
    pub fn try_store_exclusive(&self) -> Result<Option<StoreLock>, StateError> {
        let path = self.root.join("store.lock");
        let file = self.open_store_lock_file(&path)?;
        // Bind so the flock temporary keeps the 2021 drop point under edition 2024.
        let acquired = flock::try_exclusive(&file).map_err(|e| io_err(&path, e))?;
        match acquired {
            Some(guard) => Ok(Some(StoreLock {
                _guard: guard,
                _file: file,
            })),
            None => Ok(None),
        }
    }

    fn open_store_lock_file(&self, path: &Path) -> Result<fs::File, StateError> {
        OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .map_err(|e| io_err(path, e))
    }
}

/// A held flock on `$root/store.lock`. Guard is declared first so it unlocks before the file closes.
#[derive(Debug)]
pub struct StoreLock {
    _guard: FlockGuard,
    _file: fs::File,
}

pub const CONFIG_FILE: &str = "config.json";

#[derive(serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    secondary_roots: Option<Vec<PathBuf>>,
    cache_roots: Option<Vec<PathBuf>>,
    gc_window_days: Option<u64>,
}

/// Effective configuration: a key set in `config.json` replaces its default list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub secondary_roots: Vec<PathBuf>,
    pub cache_roots: Vec<PathBuf>,
    pub gc_window_days: Option<u64>,
}

#[derive(Debug)]
pub struct WindowError(pub u64);

impl fmt::Display for WindowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "gc_window_days is {}, not a whole number of days from 1 to 365; set it within that range in config.json",
            self.0
        )
    }
}

impl std::error::Error for WindowError {}

pub fn window_days(raw: Option<u64>) -> Result<u64, WindowError> {
    let Some(days) = raw else {
        return Ok(7);
    };
    if (1..=365).contains(&days) {
        Ok(days)
    } else {
        Err(WindowError(days))
    }
}

impl State {
    pub fn config(&self) -> Result<Config, StateError> {
        let file: ConfigFile = self.read(CONFIG_FILE)?;
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let normalize = |p: PathBuf| -> PathBuf {
            let p = match (p.strip_prefix("~"), &home) {
                (Ok(rest), Some(h)) => h.join(rest),
                _ => p,
            };
            p.canonicalize().unwrap_or(p)
        };
        let dedup = |v: Vec<PathBuf>| {
            let mut out: Vec<PathBuf> = Vec::new();
            for p in v.into_iter().map(normalize) {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
            out
        };
        let secondary = file.secondary_roots.unwrap_or_else(|| {
            let mut v = vec![PathBuf::from("/private/tmp")];
            v.extend(std::env::var_os("TMPDIR").map(PathBuf::from));
            v
        });
        let cache = file
            .cache_roots
            .unwrap_or_else(|| home.iter().map(|h| h.join(".cache/cargo-target")).collect());
        Ok(Config {
            secondary_roots: dedup(secondary),
            cache_roots: dedup(cache),
            gc_window_days: file.gc_window_days,
        })
    }
}

pub struct Tx<'a> {
    root: &'a Path,
}

impl Tx<'_> {
    pub fn read<T: DeserializeOwned + Default>(&self, name: &str) -> Result<T, StateError> {
        read_json(&state_file(self.root, name)?)
    }

    /// Reads at most `max` bytes. One byte over that is an error and the rest is not read.
    pub fn read_bounded<T: DeserializeOwned + Default>(
        &self,
        name: &str,
        max: u64,
    ) -> Result<T, StateError> {
        read_json_limited(&state_file(self.root, name)?, max)
    }

    pub fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<(), StateError> {
        let path = state_file(self.root, name)?;
        let bytes = encode(&path, value)?;
        atomic_write(&path, &bytes)
    }

    /// Writes `value` only when the same bytes `write` would store are at most `max`.
    pub fn write_bounded<T: Serialize>(
        &self,
        name: &str,
        value: &T,
        max: u64,
    ) -> Result<(), StateError> {
        let path = state_file(self.root, name)?;
        let bytes = encode(&path, value)?;
        if bytes.len() as u64 > max {
            return Err(StateError::Oversized { file: path, max });
        }
        atomic_write(&path, &bytes)
    }

    /// Appends one line to today's receipts file by rewriting it whole.
    pub fn append_receipt_line(&self, line: &str) -> Result<PathBuf, StateError> {
        let path = self
            .root
            .join("receipts")
            .join(format!("{}.jsonl", utc_date(SystemTime::now())));
        let mut bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(io_err(&path, e)),
        };
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        atomic_write(&path, &bytes)?;
        Ok(path)
    }
}

fn encode<T: Serialize>(path: &Path, value: &T) -> Result<Vec<u8>, StateError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| StateError::Malformed {
        file: path.to_path_buf(),
        detail: e.to_string(),
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn read_json<T: DeserializeOwned + Default>(path: &Path) -> Result<T, StateError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| StateError::Malformed {
            file: path.to_path_buf(),
            detail: e.to_string(),
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(io_err(path, e)),
    }
}

fn read_json_limited<T: DeserializeOwned + Default>(
    path: &Path,
    max: u64,
) -> Result<T, StateError> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(T::default()),
        Err(e) => return Err(io_err(path, e)),
    };
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_err(path, e))?;
    if bytes.len() as u64 > max {
        return Err(StateError::Malformed {
            file: path.to_path_buf(),
            detail: format!("larger than {max} bytes"),
        });
    }
    serde_json::from_slice(&bytes).map_err(|e| StateError::Malformed {
        file: path.to_path_buf(),
        detail: e.to_string(),
    })
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), StateError> {
    let dir = path
        .parent()
        .expect("state files live inside the state root");
    let name = path
        .file_name()
        .expect("state files have a name")
        .to_string_lossy();
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        // Bind the dir fd so its drop stays after locals under edition 2024.
        let dir_file = fs::File::open(dir)?;
        dir_file.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|e| io_err(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cargo_hive_platform::clock::{utc_date, utc_timestamp};
    use serde::{Deserialize, Serialize};
    use std::time::{Duration, UNIX_EPOCH};

    #[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
    struct Counter {
        n: u64,
    }

    fn scratch(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("hive-state-{label}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn the_window_defaults_to_seven_days_and_accepts_one_to_365() {
        assert_eq!(window_days(None).unwrap(), 7);
        assert_eq!(window_days(Some(1)).unwrap(), 1);
        assert_eq!(window_days(Some(365)).unwrap(), 365);
        for bad in [0u64, 366, 10_000] {
            assert!(window_days(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn config_reads_the_window_key_and_still_refuses_unknown_keys() {
        let root = scratch("window-key");
        let state = State::open_at(root.clone()).unwrap();
        std::fs::write(root.join("config.json"), r#"{"gc_window_days": 30}"#).unwrap();
        assert_eq!(state.config().unwrap().gc_window_days, Some(30));
        std::fs::write(root.join("config.json"), r#"{"gc_window": 30}"#).unwrap();
        assert!(state.config().is_err());
    }

    #[test]
    fn an_out_of_range_window_does_not_fail_config_for_commands_that_ignore_it() {
        let root = scratch("window-ignored");
        let state = State::open_at(root.clone()).unwrap();
        std::fs::write(root.join("config.json"), r#"{"gc_window_days": 0}"#).unwrap();
        assert!(state.config().is_ok());
    }

    #[test]
    fn opening_creates_the_root_and_missing_files_read_as_defaults() {
        let root = scratch("open");
        let state = State::open_at(root.clone()).unwrap();
        assert!(root.join("receipts").is_dir());
        assert_eq!(
            state.read::<Counter>("counter.json").unwrap(),
            Counter::default()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_malformed_file_is_refused_and_left_untouched() {
        let root = scratch("malformed");
        let state = State::open_at(root.clone()).unwrap();
        fs::write(root.join("counter.json"), b"not json").unwrap();
        let err = state
            .locked(|tx| tx.read::<Counter>("counter.json"))
            .unwrap_err();
        assert!(matches!(err, StateError::Malformed { .. }), "{err}");
        assert_eq!(fs::read(root.join("counter.json")).unwrap(), b"not json");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_read_modify_write_cycles_lose_no_update() {
        let root = scratch("concurrent");
        State::open_at(root.clone()).unwrap();
        std::thread::scope(|s| {
            for _ in 0..8 {
                let root = root.clone();
                s.spawn(move || {
                    let state = State::open_at(root).unwrap();
                    for _ in 0..25 {
                        state
                            .locked(|tx| {
                                let mut c: Counter = tx.read("counter.json")?;
                                c.n += 1;
                                tx.write("counter.json", &c)
                            })
                            .unwrap();
                    }
                });
            }
        });
        let state = State::open_at(root.clone()).unwrap();
        assert_eq!(state.read::<Counter>("counter.json").unwrap().n, 200);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn each_receipt_line_is_appended_to_todays_file() {
        let root = scratch("receipts");
        let state = State::open_at(root.clone()).unwrap();
        let mut path = PathBuf::new();
        for i in 0..3 {
            path = state
                .locked(|tx| tx.append_receipt_line(&format!("{{\"i\":{i}}}")))
                .unwrap();
        }
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text, "{\"i\":0}\n{\"i\":1}\n{\"i\":2}\n");
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            format!("{}.jsonl", utc_date(SystemTime::now()))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn without_a_config_file_the_documented_defaults_apply_per_key() {
        let root = scratch("defaults");
        let state = State::open_at(root.clone()).unwrap();
        let canon = |p: PathBuf| p.canonicalize().unwrap_or(p);
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let cfg = state.config().unwrap();
        assert_eq!(
            cfg.cache_roots,
            vec![canon(home.join(".cache/cargo-target"))]
        );
        assert_eq!(cfg.secondary_roots[0], canon(PathBuf::from("/private/tmp")));
        if let Some(t) = std::env::var_os("TMPDIR") {
            assert!(cfg.secondary_roots.contains(&canon(PathBuf::from(t))));
        }
        fs::write(
            root.join(CONFIG_FILE),
            br#"{"cache_roots": ["/private/tmp"]}"#,
        )
        .unwrap();
        let cfg = state.config().unwrap();
        assert_eq!(cfg.cache_roots, vec![PathBuf::from("/private/tmp")]);
        assert_eq!(
            cfg.secondary_roots[0],
            PathBuf::from("/private/tmp"),
            "an unset key keeps its default"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_state_file_name_cannot_escape_the_root() {
        let root = scratch("escape");
        let state = State::open_at(root.clone()).unwrap();
        let outside = std::env::temp_dir().join(format!(
            "hive-escaped-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let abs = outside.to_str().unwrap().to_string();
        let wrote = state.locked(|tx| tx.write(&abs, &Counter { n: 1 }));
        let leaked = outside.exists();
        let _ = fs::remove_file(&outside);
        assert!(wrote.is_err(), "an absolute name is rejected");
        assert!(!leaked, "an absolute name wrote outside the root");

        let parent_file = root.parent().unwrap().join("hive-escaped.json");
        let wrote = state.locked(|tx| tx.write("../hive-escaped.json", &Counter { n: 7 }));
        let leaked = parent_file.exists();
        let _ = fs::remove_file(&parent_file);
        assert!(wrote.is_err(), "a .. name is rejected");
        assert!(!leaked, "a .. name wrote outside the root");

        assert!(state.locked(|tx| tx.write("", &Counter { n: 1 })).is_err());
        assert!(state.read::<Counter>(&abs).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn read_bounded_refuses_a_file_over_its_bound_and_takes_no_lock() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Instant;

        let root = scratch("read-bounded");
        let state = State::open_at(root.clone()).unwrap();
        let name = "counter.json";
        let path = root.join(name);
        let max = 16u64;
        let mut exact = br#"{"n":1}"#.to_vec();
        exact.resize(max as usize, b' ');
        fs::write(&path, &exact).unwrap();

        let held = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let holder = State::open_at(root.clone()).unwrap();
        let held_flag = Arc::clone(&held);
        let release_flag = Arc::clone(&release);
        let thread = std::thread::spawn(move || {
            holder
                .locked(|_| {
                    held_flag.store(true, Ordering::SeqCst);
                    let start = Instant::now();
                    while !release_flag.load(Ordering::SeqCst)
                        && start.elapsed() < Duration::from_secs(3)
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Ok(())
                })
                .unwrap();
        });
        while !held.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let started = Instant::now();
        let got: Counter = state.read_bounded(name, max).unwrap();
        assert_eq!(got, Counter { n: 1 });
        let mut over = exact;
        over.push(b' ');
        fs::write(&path, &over).unwrap();
        let err = state.read_bounded::<Counter>(name, max).unwrap_err();
        assert!(matches!(err, StateError::Malformed { .. }), "{err}");
        assert!(err.to_string().contains("larger than 16 bytes"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "read waited on the state lock"
        );
        release.store(true, Ordering::SeqCst);
        thread.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn utc_formatting_matches_known_instants() {
        assert_eq!(utc_timestamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            utc_timestamp(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            "2023-11-14T22:13:20Z"
        );
        assert_eq!(
            utc_date(UNIX_EPOCH + Duration::from_secs(951_782_400)),
            "2000-02-29"
        );
    }

    #[test]
    fn two_shared_store_locks_coexist() {
        let root = scratch("store-sh-coexist");
        let a = State::open_at(root.clone()).unwrap();
        let b = State::open_at(root.clone()).unwrap();
        let first = a.store_shared_or_cancelled(|| false).unwrap();
        let second = b.store_shared_or_cancelled(|| false).unwrap();
        drop(first);
        drop(second);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_exclusive_try_returns_none_while_a_shared_lock_is_held() {
        let root = scratch("store-ex-while-sh");
        let holder = State::open_at(root.clone()).unwrap();
        let probe = State::open_at(root.clone()).unwrap();
        let shared = holder.store_shared_or_cancelled(|| false).unwrap();
        let exclusive = probe.try_store_exclusive().unwrap();
        assert!(exclusive.is_none());
        drop(shared);
        let exclusive = probe.try_store_exclusive().unwrap();
        assert!(exclusive.is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_shared_lock_waits_for_an_exclusive_one_and_honors_cancel() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let root = scratch("store-sh-cancel");
        let holder = State::open_at(root.clone()).unwrap();
        let waiter = State::open_at(root.clone()).unwrap();
        let exclusive = holder.try_store_exclusive().unwrap().expect("exclusive");
        let held = Arc::new(AtomicBool::new(true));
        let release = Arc::clone(&held);
        let thread = std::thread::spawn(move || {
            while release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
            drop(exclusive);
        });
        let mut calls = 0u8;
        let err = waiter
            .store_shared_or_cancelled(|| {
                calls += 1;
                calls >= 3
            })
            .unwrap_err();
        assert!(matches!(err, StateError::Cancelled), "{err}");
        assert_eq!(calls, 3);
        held.store(false, Ordering::SeqCst);
        thread.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_lock_file_holds_no_bytes() {
        let root = scratch("store-lock-empty");
        let state = State::open_at(root.clone()).unwrap();
        let lock = state.try_store_exclusive().unwrap().expect("exclusive");
        let meta = fs::metadata(root.join("store.lock")).unwrap();
        assert_eq!(meta.len(), 0);
        drop(lock);
        let meta = fs::metadata(root.join("store.lock")).unwrap();
        assert_eq!(meta.len(), 0);
        fs::remove_dir_all(root).unwrap();
    }
}
