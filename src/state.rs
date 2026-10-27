//! hive's own state under `~/.hive/` (or `$HIVE_HOME`, test-only). This module is
//! the only code that writes hive's files; every write is a whole-file atomic
//! replace taken under one `flock`, so a kill never leaves a half-written file.

use serde::{de::DeserializeOwned, Serialize};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub enum StateError {
    Io { path: PathBuf, source: io::Error },
    Malformed { file: PathBuf, detail: String },
    NoHome,
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
            StateError::NoHome => write!(f, "HOME is not set, so ~/.hive cannot be located"),
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
        read_json(&self.root.join(name))
    }

    /// Runs `body` while holding the exclusive state lock (`$root/.lock`).
    pub fn locked<T>(
        &self,
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
        loop {
            // SAFETY: `file` owns a valid descriptor for the whole call.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                break;
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::Interrupted {
                return Err(io_err(&path, err));
            }
        }
        let result = body(&Tx { root: &self.root });
        drop(file); // closing the descriptor releases the lock
        result
    }
}

pub const CONFIG_FILE: &str = "config.json";

#[derive(serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    secondary_roots: Option<Vec<PathBuf>>,
    cache_roots: Option<Vec<PathBuf>>,
}

/// Effective configuration: a key set in `config.json` replaces its default list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub secondary_roots: Vec<PathBuf>,
    pub cache_roots: Vec<PathBuf>,
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
        })
    }
}

pub struct Tx<'a> {
    root: &'a Path,
}

impl Tx<'_> {
    pub fn read<T: DeserializeOwned + Default>(&self, name: &str) -> Result<T, StateError> {
        read_json(&self.root.join(name))
    }

    pub fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<(), StateError> {
        let path = self.root.join(name);
        let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| StateError::Malformed {
            file: path.clone(),
            detail: e.to_string(),
        })?;
        bytes.push(b'\n');
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
        fs::File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|e| io_err(path, e))
}

/// `YYYY-MM-DD` in UTC.
pub fn utc_date(t: SystemTime) -> String {
    let (y, m, d, _, _, _) = civil(t);
    format!("{y:04}-{m:02}-{d:02}")
}

/// RFC 3339 in UTC with second precision.
pub fn utc_timestamp(t: SystemTime) -> String {
    let (y, m, d, hh, mm, ss) = civil(t);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn civil(t: SystemTime) -> (i64, u32, u32, u32, u32, u32) {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Inverse of Howard Hinnant's days_from_civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (
        y,
        m,
        d,
        (rem / 3_600) as u32,
        ((rem % 3_600) / 60) as u32,
        (rem % 60) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
