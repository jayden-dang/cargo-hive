# Tasks: Foundation (registry, discovery, classification, target identification, fingerprint reader, receipts, doctor)

> **For agentic workers:** after plan approval, pick one execute skill —
> `build-in-waves` (continuous dependency-aware scheduler), `build-by-story` (human-gated story review
> units), or `build-inline` (controller implements, no implementer subagents).
> The chosen skill writes `Execution-mode:`. Steps use checkbox (`- [ ]`) syntax
> for tracking.

Feature code: CORE
Status: Approved
Date: 2026-10-25
Execution-mode: continuous
Max-concurrency: auto
Requirements: ./requirements.md
Design: ./design.md

**Goal:** Ship `cargo hive doctor | repo add | repo list | status`. Together they tell a developer or an agent which checkouts and Cargo target dirs exist on this Mac, how hive may treat each one, and what hive can trust here. Nothing outside hive's own state is changed.

**Architecture:** There is one crate, `cargo-hive`: a library holding all logic, plus a thin `clap` binary that Cargo runs as `cargo hive`. `state` is the only writer of hive's files. It uses one `flock` and atomic whole-file rewrites. Every command builds a `receipt` of deterministic counters. Discovery is a set of bounded walks plus `git` and `cargo metadata --no-deps --locked --offline` children with scrubbed environments. Fingerprints are read through an exact format signature (ADR-0001).

**Tech Stack:** Rust (edition 2021; proof toolchain rustc 1.97.1, aarch64-apple-darwin, macOS 27). Dependencies: `serde` 1.0.229, `serde_json` 1.0.154, `libc` 0.2.191, `clap` 4.6.7 (derive). On macOS only: `applesauce` 0.8.10 and `objc2-local-authentication` 0.3.2. The approved `objc2` 0.6.5 and `objc2-foundation` 0.3.2 arrive transitively through `objc2-local-authentication`, and nothing in this feature names them directly, so they are not listed (the plan review built the crate without them). Tests use `cargo test`: unit tests in-module, integration tests in `tests/*.rs` that run the binary through `env!("CARGO_BIN_EXE_cargo-hive")` with `HIVE_HOME` set to a scratch dir. `block2` is approved but not needed until ROAD-7 (`evaluatePolicy`), so it is not added here.

## Global Constraints

There is no `docs/standards/`, `docs/product/guidelines.md` or `docs/agents/project.md` in this repo. The canonical constraint sources are therefore the approved decision documents. Read them and do not restate them. The hashes were taken on 2026-10-27; a mismatch at execute time means the source changed after planning:

| Source | sha256 (first 16) |
|---|---|
| `.skills/CORE/close-package.md` (locks A1–A6, D1–D8, S1–S2, C1, I1–I4, F9 amendment, 2026-10-28 amendment) | `5d2077befae8a6c6` |
| `docs/specs/2026-10-25-foundation/design.md` (Decisions 1–15) | `11e491d01940fc2f` |
| `docs/adr/0001-read-cargo-internal-fingerprints.md` | `49753555c2cb1f39` |
| `CONTEXT.md` (glossary; use its terms) | `2324238c330dc529` |

Re-pinned 2026-10-28 after CORE was Implemented: the close package gained its define-project amendment (Desired narrowed to `docs/product/vision.md`); `design.md` and `CONTEXT.md` had changed during the close-fix rounds and gained the 2026-10-28 `Satisfies:` lines and the **hive** entry. The 2026-10-27 values were `efb97ca2fbc6607e`, `b06c89ff85ddb3e1` and `6d39ad7c7a2c334e`.

Commands every task must leave green:
- `cargo fmt --check`, after running `cargo fmt`. The code in this plan is the content to write; rustfmt owns the layout.
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`

## Repository and setup (once, before Task 1)

- The crate lives at the root of `/Users/jayden/Developer/experiments/cargo-zip`, beside `docs/`, `.skills/` and `CONTEXT.md`. Every path in this plan is relative to that root, and the README links `docs/adr/0001`.
- That folder is not a git repository yet. With the user's approval the controller runs `git init -b main` there and writes `.gitignore` with the line `/.remember/` (a personal notes buffer). It then commits the existing research and spec files as `chore: research, specs and plan for cargo-hive` before Task 1 starts.
- The empty repository at `cargo-zip/cargo-hive/` is not used by this plan. Whether to remove it is the user's call.
- `Cargo.lock` is committed, since this is a binary crate.
- Every commit step stages with `git add -A`.

Shared files: `src/lib.rs` (one `pub mod` line per module) and `src/cli.rs` (one variant and one dispatch arm per command) are edited by many tasks. Their Depends-on edges serialize most of those edits. If parallel lanes still touch them together, the merge is mechanical: keep both lines.

"Active toolchain" means the toolchain `rustup` resolves outside any checkout, with hive's state dir as the working directory. Doctor (check f and the recorded release) and the `status` gate use the same resolution, so they agree. A checkout pinned to another release is still protected, because every profile dir's fingerprints are checked against the format signature when read (ADR-0001). This deviates from design Decision 7, which resolved in doctor's temp dir. Both dirs are neutral; the state dir is used so that the gate and doctor resolve identically.

House rules for this plan:
1. Hive's code never writes outside `$HIVE_HOME` (or `~/.hive`) and doctor's temp dir.
2. Child `git`/`cargo` processes always get the scrubbed environment from Decisions 4, 5 and 7.
3. Requirement IDs never appear in source code (identifiers or comments) or test names; they live in requirements, design and these task footers.
4. Tests never write under the real `~/.cargo`, `~/.rustup`, `~/.cache` or any repo outside their scratch dir.
5. Integration fixtures live in `/private/tmp/hive-it-*` and are removed on drop.

## File Structure

| Path | Responsibility |
|---|---|
| `Cargo.toml` | Crate manifest: lib `cargo_hive`, bin `cargo-hive`, dependencies, and target-gated macOS dependencies |
| `.gitignore` | Ignore `target/` and the personal `.remember/` buffer |
| `README.md` | Install from scratch (Rust toolchain plus Xcode Command Line Tools for the C parts of applesauce), commands, `HIVE_HOME` documented as test-only, and the statement that hive installs no config, environment, wrapper or hook |
| `src/lib.rs` | Module declarations |
| `src/main.rs` | Binary entry, forwards to `cli::main_with` |
| `src/cli.rs` | clap commands (`doctor`, `repo add/list`, `status`, hidden `__hold-lock`), dispatch, receipt emission, printing |
| `src/state.rs` | State root, lock, atomic writes, fail-closed reads, daily receipt log, `Config` with defaults, UTC time formatting |
| `src/receipt.rs` | Receipt schema v1 and its builder |
| `src/git.rs` | `git` child calls with a scrubbed environment (infrastructure) |
| `src/registry.rs` | `Registry`/`Repo`, `register` with its six refusals, `list` |
| `src/classify.rs` | Pure C1 classification |
| `src/discover.rs` | Checkouts per repo, bounded walks, target collection, metadata cache, `Snapshot` |
| `src/cargo_target.rs` | Cargo target identification, profile dirs, ownership via `cargo metadata` |
| `src/fingerprint.rs` | Format signature, units, edge resolution |
| `src/size.rs` | Inode-deduplicated allocated bytes |
| `src/platform/mod.rs` | `Fs`/`Approve` traits and value types, the OS check, `sw_vers` version |
| `src/platform/apfs.rs` | macOS `Fs`: temp dirs, clonefile, getattrlist sharing, applesauce compression, flock/fcntl lock holder and probe |
| `src/platform/local_auth.rs` | macOS `Approve`: device-owner authentication availability only |
| `src/doctor.rs` | Checks a–f, report, capability map, `require` gate, toolchain resolution, cancellation, stale temp-dir sweep |
| `tests/common/mod.rs` | Integration helpers: `Scratch`, `Hive` (`ready`, `config`, `build_crate`, `write_ready_capabilities`), `git`, `init_repo` |
| `tests/receipts.rs` | Story 5, plus CORE-2.6 and CORE-2.5, through the binary |
| `tests/repo.rs` | Story 2 through the binary |
| `tests/status_checkouts.rs` | Story 3 through the binary |
| `tests/platform_locks.rs` | Lock holder and probe through the binary plus the library |
| `tests/doctor.rs` | Story 1 through the binary (machine-gated on macOS) |
| `tests/status_targets.rs` | Story 4 through the binary |
| `tests/capability_gate.rs` | CORE-1.4 and CORE-1.10 through `status` |
| `tests/guards.rs` | CORE-6.3, 6.5, 7.1 and 7.2 guards, and the CORE-6.1 performance check (ignored by default) |

Retrieval: `load-subgraph` `blast_radius` on these paths and `cluster(CORE)` found nothing. The catalog holds only `CORE` and the repo has no source yet, so the coverage statement is: 0 neighbours, 0 existing readers of any mapped path.

---

### Task 1: Crate skeleton and crash-safe state

**Files:**
- Create: `Cargo.toml`
- Modify: `.gitignore` (add `/target`; setup created the file)
- Create: `src/lib.rs`
- Create: `src/state.rs`
- Test: `src/state.rs` (`#[cfg(test)] mod tests`)

**Reuse:** 3 — `std::fs` and `std::os::unix` for files, `fsync` and `rename`; 5 — `libc::flock` for the lock, `serde_json` for the shapes

**Interfaces:**
- Consumes: none
- Produces: `cargo_hive::state::{State, Tx, StateError, utc_date, utc_timestamp}`
  - `State::open() -> Result<State, StateError>` and `State::open_at(PathBuf) -> Result<State, StateError>`
  - `State::root(&self) -> &Path`
  - `State::read::<T: DeserializeOwned + Default>(&self, name: &str) -> Result<T, StateError>`
  - `State::locked<T>(&self, body: impl FnOnce(&Tx<'_>) -> Result<T, StateError>) -> Result<T, StateError>`
  - `Tx::read`
  - `Tx::write<T: Serialize>(&self, name: &str, value: &T) -> Result<(), StateError>`
  - `Tx::append_receipt_line(&self, line: &str) -> Result<PathBuf, StateError>`
  - `StateError::{Io{path, source}, Malformed{file, detail}, NoHome}`

**Depends-on:** none

- [x] **Step 1: Write the manifest, the module list and the failing tests**

`Cargo.toml`:

```toml
[package]
name = "cargo-hive"
version = "0.1.0"
edition = "2021"
description = "Cargo build artifacts across agent worktrees on macOS, with measured receipts"
license = "MIT OR Apache-2.0"

[lib]
name = "cargo_hive"
path = "src/lib.rs"

[[bin]]
name = "cargo-hive"
path = "src/main.rs"

[dependencies]
clap = { version = "4.6.7", features = ["derive"] }
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.154"
libc = "0.2.191"

[target.'cfg(target_os = "macos")'.dependencies]
applesauce = "0.8.10"
objc2-local-authentication = "0.3.2"

[profile.release]
panic = "unwind"
```

`.gitignore` (setup wrote `/.remember/`; append the second line):

```
/.remember/
/target
```

`src/lib.rs`:

```rust
pub mod state;
```

`src/main.rs` (temporary until Task 2 replaces it):

```rust
fn main() {}
```

At the bottom of `src/state.rs` (the file starts with only this test module):

```rust
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
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("hive-state-{label}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn opening_creates_the_root_and_missing_files_read_as_defaults() {
        let root = scratch("open");
        let state = State::open_at(root.clone()).unwrap();
        assert!(root.join("receipts").is_dir());
        assert_eq!(state.read::<Counter>("counter.json").unwrap(), Counter::default());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_malformed_file_is_refused_and_left_untouched() {
        let root = scratch("malformed");
        let state = State::open_at(root.clone()).unwrap();
        fs::write(root.join("counter.json"), b"not json").unwrap();
        let err = state.locked(|tx| tx.read::<Counter>("counter.json")).unwrap_err();
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
            path = state.locked(|tx| tx.append_receipt_line(&format!("{{\"i\":{i}}}"))).unwrap();
        }
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text, "{\"i\":0}\n{\"i\":1}\n{\"i\":2}\n");
        assert_eq!(path.file_name().unwrap().to_string_lossy(), format!("{}.jsonl", utc_date(SystemTime::now())));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn utc_formatting_matches_known_instants() {
        assert_eq!(utc_timestamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(utc_timestamp(UNIX_EPOCH + Duration::from_secs(1_700_000_000)), "2023-11-14T22:13:20Z");
        assert_eq!(utc_date(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29");
    }
}
```

Run: `cargo test --lib state` — expect: compile errors, because `State`, `Tx`, `StateError`, `utc_date` and `utc_timestamp` are not defined.

- [x] **Step 2: Implement `src/state.rs` above the test module**

```rust
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
                write!(f, "{} is malformed ({detail}); fix or remove it by hand", file.display())
            }
            StateError::NoHome => write!(f, "HOME is not set, so ~/.hive cannot be located"),
        }
    }
}

impl std::error::Error for StateError {}

fn io_err(path: &Path, source: io::Error) -> StateError {
    StateError::Io { path: path.to_path_buf(), source }
}

pub struct State {
    root: PathBuf,
}

impl State {
    pub fn open() -> Result<State, StateError> {
        let root = match std::env::var_os("HIVE_HOME") {
            Some(dir) => PathBuf::from(dir),
            None => PathBuf::from(std::env::var_os("HOME").ok_or(StateError::NoHome)?).join(".hive"),
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
    pub fn locked<T>(&self, body: impl FnOnce(&Tx<'_>) -> Result<T, StateError>) -> Result<T, StateError> {
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

pub struct Tx<'a> {
    root: &'a Path,
}

impl Tx<'_> {
    pub fn read<T: DeserializeOwned + Default>(&self, name: &str) -> Result<T, StateError> {
        read_json(&self.root.join(name))
    }

    pub fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<(), StateError> {
        let path = self.root.join(name);
        let mut bytes = serde_json::to_vec_pretty(value)
            .map_err(|e| StateError::Malformed { file: path.clone(), detail: e.to_string() })?;
        bytes.push(b'\n');
        atomic_write(&path, &bytes)
    }

    /// Appends one line to today's receipts file by rewriting it whole.
    pub fn append_receipt_line(&self, line: &str) -> Result<PathBuf, StateError> {
        let path = self.root.join("receipts").join(format!("{}.jsonl", utc_date(SystemTime::now())));
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
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| StateError::Malformed { file: path.to_path_buf(), detail: e.to_string() }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(io_err(path, e)),
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), StateError> {
    let dir = path.parent().expect("state files live inside the state root");
    let name = path.file_name().expect("state files have a name").to_string_lossy();
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new().create(true).write(true).truncate(true).open(&tmp)?;
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
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
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
    (y, m, d, (rem / 3_600) as u32, ((rem % 3_600) / 60) as u32, (rem % 60) as u32)
}
```

Run: `cargo test --lib state` — expect: 5 passed. Then run `cargo clippy --all-targets -- -D warnings`.

- [x] **Step 3: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(state): crash-safe state root with one flock and atomic rewrites"`

_Requirements: CORE-2.6, CORE-6.5_

---

### Task 2: Receipts, the CLI skeleton, and `repo list`

**Files:**
- Create: `src/receipt.rs`
- Create: `src/platform/mod.rs`
- Create: `src/registry.rs`
- Create: `src/cli.rs`
- Modify: `src/main.rs` (replace the placeholder body)
- Modify: `src/lib.rs` (add `cli`, `platform`, `receipt`, `registry`)
- Create: `tests/common/mod.rs`
- Test: `src/receipt.rs` (`#[cfg(test)]`)
- Test: `src/platform/mod.rs` (`#[cfg(test)]`)
- Test: `tests/receipts.rs`

**Reuse:** 5 — `serde` derive on a closed struct (receipt); 5 — `clap` derive (cli)

**Interfaces:**
- Consumes: `state::{State, Tx, StateError, utc_timestamp}` (Task 1)
- Produces:
  - `receipt::{Receipt, ReceiptBuilder, Scope, CheckRecord, Finding, Unmeasured, Measured, SCHEMA}`
    - `Receipt::new(command: &str, scope: Scope) -> ReceiptBuilder`
    - `Receipt::with_os(command: &str, scope: Scope, os: Option<String>) -> ReceiptBuilder`
    - builder methods `count`, `check`, `finding`, `unmeasured`, `note`, `toolchain`, `scope_mut`, and `finish() -> Receipt`
  - `platform::{MACOS_ONLY_MESSAGE, NotMacOs, check_os(&str) -> Result<(), NotMacOs>, ensure_supported() -> Result<(), NotMacOs>, os_version() -> Option<String>, major_version(&str) -> Option<u32>}`
  - `registry::{Registry, Repo, FILE}`, where `Repo { main: PathBuf, root_commits: BTreeSet<String> }`
  - `cli::{main_with(Vec<OsString>) -> i32, emit(...)}`
  - Test helpers `common::{Scratch, Hive, git, init_repo}`

**Depends-on:** Task 1

- [x] **Step 1: Write the failing tests**

`tests/common/mod.rs`:

```rust
#![allow(dead_code)]
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A scratch directory under /private/tmp, canonicalized, removed on drop.
pub struct Scratch {
    pub path: PathBuf,
}

impl Scratch {
    pub fn new(label: &str) -> Scratch {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let path = Path::new("/private/tmp").join(format!("hive-it-{label}-{}-{n}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Scratch { path: path.canonicalize().unwrap() }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = Command::new("chmod").arg("-R").arg("u+rwx").arg(&self.path).status();
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// One hive installation: its own HIVE_HOME, plus a scratch CARGO_HOME for child cargo.
pub struct Hive {
    pub home: Scratch,
    pub cargo_home: Scratch,
}

impl Hive {
    pub fn new() -> Hive {
        Hive { home: Scratch::new("home"), cargo_home: Scratch::new("cargo-home") }
    }

    pub fn config(&self, json: &str) {
        fs::write(self.home.path.join("config.json"), json).unwrap();
    }

    pub fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cargo-hive"));
        cmd.args(args)
            .env("HIVE_HOME", &self.home.path)
            .env("CARGO_HOME", &self.cargo_home.path)
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_TARGET_DIR")
            .env_remove("CARGO_BUILD_BUILD_DIR");
        cmd
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    /// Runs with `--json` and parses stdout as exactly one JSON value.
    pub fn json(&self, args: &[&str]) -> (Output, serde_json::Value) {
        let mut all = args.to_vec();
        all.push("--json");
        let out = self.run(&all);
        let value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!("stdout was not one JSON receipt ({e}):\n{}\nstderr:\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
        });
        (out, value)
    }
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=hive", "-c", "user.email=hive@example.invalid", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "-c", "protocol.file.allow=always", "-c", "init.defaultBranch=main", "-c", "core.fsmonitor=false"])
        .arg("-C")
        .arg(dir)
        .args(args)
        // Fixtures never see the developer's git config (an fsmonitor there leaves a
        // socket in .git and a daemon running).
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// A git repo at `dir` with a Cargo package at `crate_rel` (relative, "" for the root) and one commit.
pub fn init_repo(dir: &Path, crate_rel: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    let krate = dir.join(crate_rel);
    fs::create_dir_all(krate.join("src")).unwrap();
    fs::write(krate.join("Cargo.toml"), "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n").unwrap();
    fs::write(krate.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n").unwrap();
    git(dir, &["add", "-A"]);
    // The path in the message makes every fixture's root commit unique, even for
    // tests that run in the same second.
    git(dir, &["commit", "-q", "-m", &format!("init {}", dir.display())]);
    dir.canonicalize().unwrap()
}
```

`tests/receipts.rs`:

```rust
mod common;
use common::{init_repo, Hive, Scratch};
use std::fs;

const RECEIPT_KEYS: [&str; 12] = ["schema", "hive_version", "macos_version", "toolchain", "command", "ts", "scope", "counts", "checks", "findings", "unmeasured", "notes"];

#[test]
fn json_mode_prints_exactly_one_receipt_and_nothing_else() {
    let hive = Hive::new();
    let (out, receipt) = hive.json(&["repo", "list"]);
    assert!(out.status.success());
    let obj = receipt.as_object().unwrap();
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort();
    let mut expected = RECEIPT_KEYS.to_vec();
    expected.sort();
    assert_eq!(keys, expected);
    assert_eq!(receipt["schema"], 1);
    assert_eq!(receipt["command"], "repo list");
    assert!(!receipt["hive_version"].as_str().unwrap().is_empty());
    assert!(receipt["macos_version"].as_str().is_some(), "sw_vers is available on the proof machine");
    assert_eq!(receipt["counts"]["repos"], 0);
}

#[test]
fn every_command_appends_its_receipt_to_the_log() {
    let hive = Hive::new();
    let (_, first) = hive.json(&["repo", "list"]);
    let (_, second) = hive.json(&["repo", "list"]);
    let dir = hive.home.path.join("receipts");
    let files: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(files.len(), 1);
    let lines: Vec<serde_json::Value> = fs::read_to_string(&files[0]).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines, vec![first, second]);
}

#[test]
fn a_fresh_machine_needs_no_configuration() {
    let hive = Hive::new();
    fs::remove_dir_all(&hive.home.path).unwrap();
    let out = hive.run(&["repo", "list"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(hive.home.path.join("receipts").is_dir());
    assert!(!hive.home.path.join("config.json").exists());
}

#[test]
fn scope_comes_from_arguments_never_from_the_working_directory() {
    let hive = Hive::new();
    let repos = Scratch::new("cwd");
    let repo = init_repo(&repos.path.join("r"), "");
    let out = hive.command(&["repo", "list", "--json"]).current_dir(&repo).output().unwrap();
    let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(receipt["scope"]["repos"], serde_json::json!([]));
    assert_eq!(receipt["scope"]["checkouts"], serde_json::json!([]));
}

#[test]
fn human_mode_prints_text_not_json() {
    let hive = Hive::new();
    let out = hive.run(&["repo", "list"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("no repositories registered"), "{text}");
    assert!(serde_json::from_str::<serde_json::Value>(&text).is_err());
}
```

Unit tests at the bottom of `src/receipt.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn has_float(v: &serde_json::Value) -> bool {
        match v {
            serde_json::Value::Number(n) => n.is_f64(),
            serde_json::Value::Array(a) => a.iter().any(has_float),
            serde_json::Value::Object(o) => o.values().any(has_float),
            _ => false,
        }
    }

    #[test]
    fn a_receipt_carries_only_counted_values() {
        let mut b = Receipt::with_os("status", Scope::default(), Some("27.0.1".into()));
        b.count("targets", 3).count("units", u64::MAX);
        b.check(CheckRecord { id: "a".into(), verdict: "pass".into(), measured: [("private_bytes".to_string(), Measured::Int(0))].into() });
        b.unmeasured("sizes", "--no-sizes");
        let value = serde_json::to_value(b.finish()).unwrap();
        assert!(!has_float(&value));
        assert_eq!(value["counts"]["units"], serde_json::json!(u64::MAX));
    }

    #[test]
    fn no_receipt_field_can_express_time_saved() {
        let value = serde_json::to_value(Receipt::with_os("x", Scope::default(), None).finish()).unwrap();
        for key in value.as_object().unwrap().keys() {
            for banned in ["saved", "duration", "seconds", "elapsed", "estimate"] {
                assert!(!key.contains(banned), "field {key}");
            }
        }
    }

    #[test]
    fn an_unavailable_os_version_is_reported_as_unmeasured_with_a_reason() {
        let r = Receipt::with_os("x", Scope::default(), None).finish();
        assert_eq!(r.macos_version, None);
        assert!(r.unmeasured.iter().any(|u| u.field == "macos_version" && !u.reason.is_empty()));
    }
}
```

Unit tests at the bottom of `src/platform/mod.rs`:

```rust
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
```

Run: `cargo test` — expect: compile errors, because `cargo_hive::cli`, `receipt`, `platform` and `registry` are missing.

- [x] **Step 2: Implement `src/platform/mod.rs` (above its tests)**

```rust
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
    let out = Command::new("sw_vers").arg("-productVersion").output().ok()?;
    let v = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !v.is_empty()).then_some(v)
}

pub fn major_version(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}
```

- [x] **Step 3: Implement `src/receipt.rs` (above its tests)**

```rust
//! Receipt schema v1: counted or measured values only. There is no float or duration slot.

use crate::state::utc_timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

pub const SCHEMA: u32 = 1;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum Measured {
    Int(u64),
    Bool(bool),
    Text(String),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct Scope {
    pub repos: Vec<PathBuf>,
    pub checkouts: Vec<PathBuf>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CheckRecord {
    pub id: String,
    pub verdict: String,
    pub measured: BTreeMap<String, Measured>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub kind: String,
    pub path: Option<PathBuf>,
    pub detail: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Unmeasured {
    pub field: String,
    pub reason: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub schema: u32,
    pub hive_version: String,
    pub macos_version: Option<String>,
    pub toolchain: Option<String>,
    pub command: String,
    pub ts: String,
    pub scope: Scope,
    pub counts: BTreeMap<String, u64>,
    pub checks: Vec<CheckRecord>,
    pub findings: Vec<Finding>,
    pub unmeasured: Vec<Unmeasured>,
    pub notes: Vec<String>,
}

pub struct ReceiptBuilder {
    r: Receipt,
}

impl Receipt {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(command: &str, scope: Scope) -> ReceiptBuilder {
        Receipt::with_os(command, scope, crate::platform::os_version())
    }

    pub fn with_os(command: &str, scope: Scope, os: Option<String>) -> ReceiptBuilder {
        ReceiptBuilder {
            r: Receipt {
                schema: SCHEMA,
                hive_version: env!("CARGO_PKG_VERSION").to_string(),
                macos_version: os,
                toolchain: None,
                command: command.to_string(),
                ts: utc_timestamp(SystemTime::now()),
                scope,
                counts: BTreeMap::new(),
                checks: Vec::new(),
                findings: Vec::new(),
                unmeasured: Vec::new(),
                notes: Vec::new(),
            },
        }
    }
}

impl ReceiptBuilder {
    pub fn count(&mut self, name: &str, n: u64) -> &mut Self {
        self.r.counts.insert(name.to_string(), n);
        self
    }
    pub fn check(&mut self, c: CheckRecord) -> &mut Self {
        self.r.checks.push(c);
        self
    }
    pub fn finding(&mut self, kind: &str, path: Option<PathBuf>, detail: impl Into<String>) -> &mut Self {
        self.r.findings.push(Finding { kind: kind.to_string(), path, detail: detail.into() });
        self
    }
    pub fn unmeasured(&mut self, field: &str, reason: impl Into<String>) -> &mut Self {
        self.r.unmeasured.push(Unmeasured { field: field.to_string(), reason: reason.into() });
        self
    }
    pub fn note(&mut self, text: impl Into<String>) -> &mut Self {
        self.r.notes.push(text.into());
        self
    }
    pub fn toolchain(&mut self, release: Option<String>) -> &mut Self {
        self.r.toolchain = release;
        self
    }
    pub fn scope_mut(&mut self) -> &mut Scope {
        &mut self.r.scope
    }
    pub fn finish(mut self) -> Receipt {
        if self.r.macos_version.is_none() {
            self.r.unmeasured.push(Unmeasured { field: "macos_version".into(), reason: "sw_vers -productVersion could not be run".into() });
        }
        self.r
    }
}
```

- [x] **Step 4: Implement `src/registry.rs` (types and list), `src/cli.rs`, `src/main.rs`, `src/lib.rs`**

`src/registry.rs`:

```rust
//! Registered repositories. Task 3 adds `register`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

pub const FILE: &str = "registry.json";

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub repos: Vec<Repo>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Repo {
    pub main: PathBuf,
    pub root_commits: BTreeSet<String>,
}
```

`src/cli.rs`:

```rust
//! Thin adapter: arguments → library calls → one receipt.

use crate::platform::{self, MACOS_ONLY_MESSAGE};
use crate::receipt::{Receipt, Scope};
use crate::registry::{self, Registry};
use crate::state::State;
use clap::{Args, Parser, Subcommand};
use std::ffi::OsString;

#[derive(Parser)]
#[command(name = "cargo-hive", bin_name = "cargo hive", version, about = "Cargo build artifacts across worktrees, with measured receipts")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Manage registered repositories
    Repo {
        #[command(subcommand)]
        command: RepoCommand,
    },
}

#[derive(Subcommand)]
pub enum RepoCommand {
    /// List registered repositories
    List(OutputArgs),
}

#[derive(Args, Clone, Copy, Debug)]
pub struct OutputArgs {
    /// Print exactly one JSON receipt on stdout (prose goes to stderr)
    #[arg(long)]
    pub json: bool,
}

/// `cargo hive …` runs `cargo-hive hive …`; drop that inserted word.
fn strip_cargo_subcommand(mut args: Vec<OsString>) -> Vec<OsString> {
    if args.get(1).is_some_and(|a| a == "hive") {
        args.remove(1);
    }
    args
}

pub fn main_with(args: Vec<OsString>) -> i32 {
    if platform::ensure_supported().is_err() {
        eprintln!("{MACOS_ONLY_MESSAGE}");
        return 2;
    }
    let cli = match Cli::try_parse_from(strip_cargo_subcommand(args)) {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return e.exit_code();
        }
    };
    let state = match State::open() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cargo-hive: {e}");
            return 1;
        }
    };
    match cli.command {
        Command::Repo { command: RepoCommand::List(out) } => repo_list(&state, out),
    }
}

/// Logs the receipt, prints it (`--json`) or the human text, and returns `code`.
pub fn emit(state: &State, out: OutputArgs, receipt: Receipt, human: &str, code: i32) -> i32 {
    let line = serde_json::to_string(&receipt).expect("receipts always serialize");
    if let Err(e) = state.locked(|tx| tx.append_receipt_line(&line).map(|_| ())) {
        eprintln!("cargo-hive: could not log the receipt: {e}");
        return 1;
    }
    if out.json {
        println!("{line}");
        eprint!("{human}");
    } else {
        print!("{human}");
    }
    code
}

fn repo_list(state: &State, out: OutputArgs) -> i32 {
    let reg: Registry = match state.read(registry::FILE) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cargo-hive: {e}");
            return 1;
        }
    };
    let mut b = Receipt::new("repo list", Scope { repos: reg.repos.iter().map(|r| r.main.clone()).collect(), checkouts: Vec::new() });
    b.count("repos", reg.repos.len() as u64);
    let mut human = String::new();
    if reg.repos.is_empty() {
        human.push_str("no repositories registered\n");
    }
    for repo in &reg.repos {
        let roots: Vec<&str> = repo.root_commits.iter().map(String::as_str).collect();
        human.push_str(&format!("{}  root {}\n", repo.main.display(), roots.join(",")));
    }
    emit(state, out, b.finish(), &human, 0)
}
```

`src/main.rs`:

```rust
fn main() {
    std::process::exit(cargo_hive::cli::main_with(std::env::args_os().collect()));
}
```

`src/lib.rs`:

```rust
pub mod cli;
pub mod platform;
pub mod receipt;
pub mod registry;
pub mod state;
```

Run: `cargo test` — expect: all tests pass (5 state, 3 receipt, 2 platform, 5 receipts integration). Then run `cargo clippy --all-targets -- -D warnings`.

- [x] **Step 5: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(cli): receipts on every command, json mode, and repo list"`

_Requirements: CORE-1.6, CORE-2.5, CORE-2.6, CORE-5.1, CORE-5.2, CORE-5.3, CORE-5.4, CORE-5.5, CORE-5.6, CORE-5.7_

---

### Task 3: Register a repository, with every refusal

**Files:**
- Create: `src/git.rs`
- Modify: `src/registry.rs` (add `register`, `RegisterOutcome`, `RegisterRefusal`, `has_cargo_toml_within`)
- Modify: `src/cli.rs` (add `repo add <path>`)
- Modify: `src/lib.rs` (add `pub mod git;`)
- Test: `tests/repo.rs`

**Reuse:** 4 — the `git` binary; 3 — `std::process::Command` (git); none — new code (rung 7). No library models "a registered repository" (registry).

**Interfaces:**
- Consumes: `state::{State, Tx}`, `registry::{Registry, Repo, FILE}`, `receipt::Receipt`, `cli::{emit, OutputArgs}`, `tests/common::{Hive, Scratch, git, init_repo}`
- Produces:
  - `git::{Git, GitError, WorktreeRecord}`
    - `Git::at(&Path) -> Git`
    - `toplevel() -> Result<PathBuf, GitError>`
    - `git_dir()`, `common_dir()` (absolute)
    - `superproject() -> Result<Option<PathBuf>, GitError>`
    - `has_commits() -> Result<bool, GitError>`
    - `root_commits() -> Result<BTreeSet<String>, GitError>`
    - `is_shallow() -> Result<bool, GitError>`
    - `worktrees() -> Result<Vec<WorktreeRecord>, GitError>`
  - `registry::{register(&Path, &Tx) -> Result<Result<RegisterOutcome, RegisterRefusal>, StateError>, RegisterOutcome::{Registered(Repo), AlreadyRegistered(Repo)}, RegisterRefusal::{NotGit, Submodule{superproject}, LinkedWorktree{main}, NoCommits, Shallow, NoCargoToml{root}}}`

**Depends-on:** Task 2

- [x] **Step 1: Write the failing tests** — `tests/repo.rs`:

```rust
mod common;
use common::{git, init_repo, Hive, Scratch};
use std::fs;

fn refusal(receipt: &serde_json::Value) -> String {
    receipt["findings"].as_array().unwrap().iter().find(|f| f["kind"] == "refusal").map(|f| f["detail"].as_str().unwrap().to_string()).unwrap_or_default()
}

#[test]
fn registering_a_main_checkout_records_its_realpath_and_root_commits() {
    let hive = Hive::new();
    let s = Scratch::new("reg");
    let repo = init_repo(&s.path.join("app"), "");
    let root = git(&repo, &["rev-list", "--max-parents=0", "HEAD"]);
    let sub = repo.join("src");
    let (out, receipt) = hive.json(&["repo", "add", sub.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(receipt["counts"]["registered"], 1);
    let (_, list) = hive.json(&["repo", "list"]);
    assert_eq!(list["scope"]["repos"], serde_json::json!([repo]));
    let text = String::from_utf8(hive.run(&["repo", "list"]).stdout).unwrap();
    assert!(text.contains(repo.to_str().unwrap()) && text.contains(&root), "{text}");
    let reg: serde_json::Value = serde_json::from_slice(&fs::read(hive.home.path.join("registry.json")).unwrap()).unwrap();
    assert_eq!(reg["repos"][0]["root_commits"], serde_json::json!([root]));
}

#[test]
fn a_cargo_manifest_two_levels_down_is_accepted_and_three_levels_down_is_refused() {
    let hive = Hive::new();
    let s = Scratch::new("depth");
    let ok = init_repo(&s.path.join("ok"), "backend/api");
    assert!(hive.run(&["repo", "add", ok.to_str().unwrap()]).status.success());
    let deep = init_repo(&s.path.join("deep"), "a/b/c");
    let (out, receipt) = hive.json(&["repo", "add", deep.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(refusal(&receipt).contains("Cargo.toml"), "{receipt}");
}

#[test]
fn a_linked_worktree_is_refused_and_names_its_main() {
    let hive = Hive::new();
    let s = Scratch::new("linked");
    let main = init_repo(&s.path.join("main"), "");
    git(&main, &["worktree", "add", "-q", s.path.join("wt").to_str().unwrap()]);
    let (out, receipt) = hive.json(&["repo", "add", s.path.join("wt").to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(refusal(&receipt).contains(main.to_str().unwrap()), "{receipt}");
}

#[test]
fn a_submodule_is_refused_and_names_its_superproject() {
    let hive = Hive::new();
    let s = Scratch::new("submodule");
    let inner = init_repo(&s.path.join("inner"), "");
    let outer = init_repo(&s.path.join("outer"), "");
    git(&outer, &["submodule", "add", "-q", inner.to_str().unwrap(), "sub"]);
    let (out, receipt) = hive.json(&["repo", "add", outer.join("sub").to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(refusal(&receipt).contains(outer.to_str().unwrap()), "{receipt}");
}

#[test]
fn a_repository_without_commits_is_refused() {
    let hive = Hive::new();
    let s = Scratch::new("unborn");
    let dir = s.path.join("empty");
    fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    fs::write(dir.join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0.1.0\"\n").unwrap();
    let (out, receipt) = hive.json(&["repo", "add", dir.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(refusal(&receipt).contains("no commits"), "{receipt}");
}

#[test]
fn a_shallow_clone_is_refused() {
    let hive = Hive::new();
    let s = Scratch::new("shallow");
    let src = init_repo(&s.path.join("src"), "");
    fs::write(src.join("README"), "x").unwrap();
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-q", "-m", "second"]);
    git(&s.path, &["clone", "-q", "--depth", "1", &format!("file://{}", src.display()), "shallow"]);
    let (out, receipt) = hive.json(&["repo", "add", s.path.join("shallow").to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(refusal(&receipt).contains("shallow"), "{receipt}");
}

#[test]
fn a_directory_outside_git_is_refused() {
    let hive = Hive::new();
    let s = Scratch::new("nogit");
    fs::write(s.path.join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0.1.0\"\n").unwrap();
    let (out, receipt) = hive.json(&["repo", "add", s.path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(refusal(&receipt).contains("not inside a git checkout"), "{receipt}");
}

#[test]
fn registering_twice_leaves_the_registry_unchanged() {
    let hive = Hive::new();
    let s = Scratch::new("twice");
    let repo = init_repo(&s.path.join("app"), "");
    assert!(hive.run(&["repo", "add", repo.to_str().unwrap()]).status.success());
    let before = fs::read(hive.home.path.join("registry.json")).unwrap();
    let (out, receipt) = hive.json(&["repo", "add", repo.to_str().unwrap()]);
    assert!(out.status.success());
    assert_eq!(receipt["counts"]["registered"], 0);
    assert!(receipt["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("already registered")));
    assert_eq!(fs::read(hive.home.path.join("registry.json")).unwrap(), before);
}
```

Run: `cargo test --test repo` — expect: failure. `repo add` is an unrecognized subcommand (clap exit code 2), so every test fails.

- [x] **Step 2: Implement `src/git.rs`**

```rust
//! `git` child processes with every `GIT_*` variable scrubbed (an inherited GIT_DIR
//! would override `-C`). Paths come back absolute.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug)]
pub enum GitError {
    NotGit,
    Failed { args: String, stderr: String },
    Spawn(std::io::Error),
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitError::NotGit => write!(f, "not inside a git checkout"),
            GitError::Failed { args, stderr } => write!(f, "git {args} failed: {}", stderr.trim()),
            GitError::Spawn(e) => write!(f, "could not run git: {e}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeRecord {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
    pub locked: bool,
    pub prunable: bool,
}

pub struct Git {
    dir: PathBuf,
}

impl Git {
    pub fn at(dir: &Path) -> Git {
        Git { dir: dir.to_path_buf() }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new("git");
        // A user's `core.fsmonitor=true` would start a daemon that writes a socket into the
        // checkout's .git; hive only reads.
        cmd.args(["-c", "core.fsmonitor=false"]).arg("-C").arg(&self.dir);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                cmd.env_remove(&key);
            }
        }
        cmd
    }

    fn run(&self, args: &[&str]) -> Result<std::process::Output, GitError> {
        let out = self.command().args(args).output().map_err(GitError::Spawn)?;
        if out.status.success() {
            return Ok(out);
        }
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        if stderr.contains("not a git repository") || !self.dir.exists() {
            return Err(GitError::NotGit);
        }
        Err(GitError::Failed { args: args.join(" "), stderr })
    }

    fn line(&self, args: &[&str]) -> Result<String, GitError> {
        Ok(String::from_utf8_lossy(&self.run(args)?.stdout).trim().to_string())
    }

    pub fn toplevel(&self) -> Result<PathBuf, GitError> {
        self.line(&["rev-parse", "--path-format=absolute", "--show-toplevel"]).map(PathBuf::from)
    }

    pub fn git_dir(&self) -> Result<PathBuf, GitError> {
        self.line(&["rev-parse", "--path-format=absolute", "--git-dir"]).map(PathBuf::from)
    }

    pub fn common_dir(&self) -> Result<PathBuf, GitError> {
        self.line(&["rev-parse", "--path-format=absolute", "--git-common-dir"]).map(PathBuf::from)
    }

    pub fn superproject(&self) -> Result<Option<PathBuf>, GitError> {
        let s = self.line(&["rev-parse", "--show-superproject-working-tree"])?;
        Ok((!s.is_empty()).then(|| PathBuf::from(s)))
    }

    pub fn has_commits(&self) -> Result<bool, GitError> {
        let out = self.command().args(["rev-parse", "--verify", "--quiet", "HEAD"]).output().map_err(GitError::Spawn)?;
        Ok(out.status.success())
    }

    pub fn root_commits(&self) -> Result<BTreeSet<String>, GitError> {
        let text = String::from_utf8_lossy(&self.run(&["rev-list", "--max-parents=0", "HEAD"])?.stdout).to_string();
        Ok(text.lines().map(str::to_string).filter(|l| !l.is_empty()).collect())
    }

    pub fn is_shallow(&self) -> Result<bool, GitError> {
        Ok(self.line(&["rev-parse", "--is-shallow-repository"])? == "true")
    }

    /// `git worktree list --porcelain -z`: attributes are NUL-terminated, and a
    /// record ends with an empty attribute.
    pub fn worktrees(&self) -> Result<Vec<WorktreeRecord>, GitError> {
        let out = self.run(&["worktree", "list", "--porcelain", "-z"])?;
        let mut records = Vec::new();
        let mut current: Option<WorktreeRecord> = None;
        for field in out.stdout.split(|b| *b == 0) {
            let field = String::from_utf8_lossy(field);
            if field.is_empty() {
                records.extend(current.take());
                continue;
            }
            let (key, value) = field.split_once(' ').unwrap_or((&field, ""));
            match key {
                "worktree" => {
                    records.extend(current.take());
                    current = Some(WorktreeRecord { path: PathBuf::from(value), ..Default::default() });
                }
                _ => {
                    let Some(rec) = current.as_mut() else { continue };
                    match key {
                        "HEAD" => rec.head = Some(value.to_string()),
                        "branch" => rec.branch = Some(value.to_string()),
                        "bare" => rec.bare = true,
                        "detached" => rec.detached = true,
                        "locked" => rec.locked = true,
                        "prunable" => rec.prunable = true,
                        _ => {}
                    }
                }
            }
        }
        records.extend(current);
        Ok(records)
    }
}
```

- [x] **Step 3: Implement `register` in `src/registry.rs`**

Append to the existing file:

```rust
use crate::git::{Git, GitError};
use crate::state::{StateError, Tx};
use std::fmt;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterOutcome {
    Registered(Repo),
    AlreadyRegistered(Repo),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterRefusal {
    NotGit,
    Submodule { superproject: PathBuf },
    LinkedWorktree { main: PathBuf },
    NoCommits,
    Shallow,
    NoCargoToml { root: PathBuf },
}

impl fmt::Display for RegisterRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegisterRefusal::NotGit => write!(f, "the path is not inside a git checkout"),
            RegisterRefusal::Submodule { superproject } => write!(f, "the path is inside a git submodule; register the superproject {} instead", superproject.display()),
            RegisterRefusal::LinkedWorktree { main } => write!(f, "the path is a linked worktree; register its main checkout {} instead", main.display()),
            RegisterRefusal::NoCommits => write!(f, "the repository has no commits yet"),
            RegisterRefusal::Shallow => write!(f, "the repository is a shallow clone, so its root commits cannot be determined"),
            RegisterRefusal::NoCargoToml { root } => write!(f, "no Cargo.toml within 2 directory levels of {}", root.display()),
        }
    }
}

/// Bounded search: is there a `Cargo.toml` at depth 0, 1 or 2 below `root`?
pub fn has_cargo_toml_within(root: &Path, depth: usize) -> bool {
    if root.join("Cargo.toml").is_file() {
        return true;
    }
    if depth == 0 {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(root) else { return false };
    entries.flatten().any(|e| {
        let name = e.file_name();
        let skip = matches!(name.to_str(), Some("target" | "node_modules" | ".git"));
        !skip && e.file_type().is_ok_and(|t| t.is_dir()) && has_cargo_toml_within(&e.path(), depth - 1)
    })
}

pub fn register(path: &Path, tx: &Tx<'_>) -> Result<Result<RegisterOutcome, RegisterRefusal>, StateError> {
    let refusal = |r| Ok(Err(r));
    let Ok(real) = path.canonicalize() else { return refusal(RegisterRefusal::NotGit) };
    let git = Git::at(&real);
    let top = match git.toplevel() {
        Ok(t) => t,
        Err(_) => return refusal(RegisterRefusal::NotGit),
    };
    let top_git = Git::at(&top);
    if let Ok(Some(superproject)) = top_git.superproject() {
        return refusal(RegisterRefusal::Submodule { superproject });
    }
    let (git_dir, common) = match (top_git.git_dir(), top_git.common_dir()) {
        (Ok(a), Ok(b)) => (a, b),
        _ => return refusal(RegisterRefusal::NotGit),
    };
    let canon = |p: PathBuf| p.canonicalize().unwrap_or(p);
    if canon(git_dir) != canon(common.clone()) {
        let main = canon(common.clone()).parent().map(Path::to_path_buf).unwrap_or(common);
        return refusal(RegisterRefusal::LinkedWorktree { main });
    }
    if !top_git.has_commits().unwrap_or(false) {
        return refusal(RegisterRefusal::NoCommits);
    }
    if top_git.is_shallow().unwrap_or(true) {
        return refusal(RegisterRefusal::Shallow);
    }
    let main = canon(top);
    if !has_cargo_toml_within(&main, 2) {
        return refusal(RegisterRefusal::NoCargoToml { root: main });
    }
    let root_commits = match top_git.root_commits() {
        Ok(r) => r,
        Err(GitError::NotGit) => return refusal(RegisterRefusal::NotGit),
        Err(_) => return refusal(RegisterRefusal::NoCommits),
    };
    let mut reg: Registry = tx.read(FILE)?;
    if let Some(existing) = reg.repos.iter().find(|r| r.main == main) {
        return Ok(Ok(RegisterOutcome::AlreadyRegistered(existing.clone())));
    }
    let repo = Repo { main, root_commits };
    reg.repos.push(repo.clone());
    reg.repos.sort_by(|a, b| a.main.cmp(&b.main));
    tx.write(FILE, &reg)?;
    Ok(Ok(RegisterOutcome::Registered(repo)))
}
```

- [x] **Step 4: Wire `repo add` in `src/cli.rs` and add `pub mod git;` to `src/lib.rs`**

Add the variant to `RepoCommand`:

```rust
    /// Register a repository by its main checkout (any path inside it)
    Add {
        path: std::path::PathBuf,
        #[command(flatten)]
        out: OutputArgs,
    },
```

Add the dispatch arm in `main_with`: `Command::Repo { command: RepoCommand::Add { path, out } } => repo_add(&state, &path, out),`. Then add:

```rust
fn repo_add(state: &State, path: &std::path::Path, out: OutputArgs) -> i32 {
    use crate::registry::{register, RegisterOutcome};
    let result = state.locked(|tx| register(path, tx));
    let mut b = Receipt::new("repo add", Scope::default());
    match result {
        Err(e) => {
            eprintln!("cargo-hive: {e}");
            1
        }
        Ok(Ok(RegisterOutcome::Registered(repo))) => {
            b.count("registered", 1);
            b.scope_mut().repos.push(repo.main.clone());
            emit(state, out, b.finish(), &format!("registered {}\n", repo.main.display()), 0)
        }
        Ok(Ok(RegisterOutcome::AlreadyRegistered(repo))) => {
            b.count("registered", 0);
            b.scope_mut().repos.push(repo.main.clone());
            b.note(format!("{} is already registered", repo.main.display()));
            emit(state, out, b.finish(), &format!("{} is already registered\n", repo.main.display()), 0)
        }
        Ok(Err(refusal)) => {
            b.count("registered", 0);
            b.finding("refusal", Some(path.to_path_buf()), refusal.to_string());
            emit(state, out, b.finish(), &format!("refused: {refusal}\n"), 1)
        }
    }
}
```

`src/lib.rs` now lists `cli, git, platform, receipt, registry, state` (alphabetical).

Run: `cargo test --test repo` — expect: 8 passed. Then run `cargo test` and `cargo clippy --all-targets -- -D warnings`.

- [x] **Step 5: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(registry): repo add with submodule, worktree, unborn and shallow refusals"`

_Requirements: CORE-2.1, CORE-2.2, CORE-2.3, CORE-2.4, CORE-2.5, CORE-2.7, CORE-2.8, CORE-2.9_

---

### Task 4: Classify a checkout (rules C1)

**Files:**
- Create: `src/classify.rs`
- Modify: `src/lib.rs` (add `pub mod classify;`)
- Test: `src/classify.rs` (`#[cfg(test)]`)

**Reuse:** none — new code (rung 7). A pure function.

**Interfaces:**
- Consumes: none
- Produces:
  - `classify::{Class::{Protected, Secondary}, CheckoutKind::{Main, Linked{main: PathBuf}, Clone}, CheckoutFacts{realpath: Result<PathBuf, String>, kind: CheckoutKind, git: Result<(), String>}}`
  - `classify::classify(&CheckoutFacts, registered_mains: &[PathBuf], secondary_roots: &[PathBuf]) -> (Class, String)`

**Depends-on:** Task 1

- [x] **Step 1: Write the failing table test** at the bottom of `src/classify.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn facts(path: &str, kind: CheckoutKind) -> CheckoutFacts {
        CheckoutFacts { realpath: Ok(p(path)), kind, git: Ok(()) }
    }

    #[test]
    fn rules_apply_in_order_and_unknown_is_protected() {
        let mains = [p("/r/main"), p("/private/tmp/runs/shared")];
        let roots = [p("/private/tmp"), p("/var/folders/x/T")];
        let linked = |m: &str| CheckoutKind::Linked { main: p(m) };
        let cases: Vec<(CheckoutFacts, Class)> = vec![
            (facts("/r/main", CheckoutKind::Main), Class::Protected),
            (facts("/private/tmp/runs/shared", CheckoutKind::Clone), Class::Protected),
            (facts("/r/main/.worktrees/lane1", linked("/r/main")), Class::Secondary),
            (facts("/r/main/.claude/worktrees/x", linked("/r/main")), Class::Secondary),
            (facts("/r/sibling-wt", linked("/r/main")), Class::Protected),
            (facts("/private/tmp/run7/klynt", CheckoutKind::Clone), Class::Secondary),
            (facts("/private/tmp/wt", linked("/r/main")), Class::Secondary),
            (facts("/private/tmpfoo/klynt", CheckoutKind::Clone), Class::Protected),
            (facts("/private/tmp", CheckoutKind::Clone), Class::Protected),
            (facts("/Users/me/other", CheckoutKind::Clone), Class::Protected),
            (CheckoutFacts { realpath: Err("No such file".into()), kind: CheckoutKind::Clone, git: Ok(()) }, Class::Protected),
            (CheckoutFacts { realpath: Ok(p("/private/tmp/x")), kind: CheckoutKind::Clone, git: Err("git failed".into()) }, Class::Protected),
        ];
        for (f, want) in cases {
            let (got, reason) = classify(&f, &mains, &roots);
            assert_eq!(got, want, "{:?} → {reason}", f.realpath);
            assert!(!reason.is_empty());
        }
    }
}
```

Run: `cargo test --lib classify` — expect: compile error, because `classify` and its types are not defined.

- [x] **Step 2: Implement `src/classify.rs` (above the tests)**

```rust
//! C1 classification: (1) a registered main is protected; (2) a linked worktree inside
//! its own main is secondary; (3) a checkout under a secondary root is secondary;
//! (4) everything else is protected. Unclassifiable is protected.

use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    Protected,
    Secondary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckoutKind {
    Main,
    Linked { main: PathBuf },
    Clone,
}

#[derive(Debug, Clone)]
pub struct CheckoutFacts {
    pub realpath: Result<PathBuf, String>,
    pub kind: CheckoutKind,
    pub git: Result<(), String>,
}

/// Component-wise "strictly below": `/private/tmpfoo` is not below `/private/tmp`.
fn strictly_below(path: &Path, root: &Path) -> bool {
    path != root && path.starts_with(root)
}

pub fn classify(facts: &CheckoutFacts, registered_mains: &[PathBuf], secondary_roots: &[PathBuf]) -> (Class, String) {
    let path = match (&facts.realpath, &facts.git) {
        (Err(e), _) => return (Class::Protected, format!("unclassifiable: {e}")),
        (_, Err(e)) => return (Class::Protected, format!("unclassifiable: {e}")),
        (Ok(p), Ok(())) => p,
    };
    if registered_mains.iter().any(|m| m == path) {
        return (Class::Protected, "registered main checkout".into());
    }
    if let CheckoutKind::Linked { main } = &facts.kind {
        if strictly_below(path, main) {
            return (Class::Secondary, format!("linked worktree inside its main {}", main.display()));
        }
    }
    if let Some(root) = secondary_roots.iter().find(|r| strictly_below(path, r)) {
        return (Class::Secondary, format!("under secondary root {}", root.display()));
    }
    (Class::Protected, "no secondary rule matched".into())
}
```

Run: `cargo test --lib classify` — expect: 1 passed (12 table rows).

- [x] **Step 3: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(classify): pure C1 classification that fails closed"`

_Requirements: CORE-3.2, CORE-3.4_

---

### Task 5: `status` lists every checkout of a repo with its class

**Files:**
- Modify: `src/state.rs` (add `Config`, `CONFIG_FILE`, `State::config`)
- Create: `src/discover.rs` (checkouts part)
- Modify: `src/cli.rs` (add `status [<repo>…]`)
- Modify: `src/lib.rs` (add `pub mod discover;`)
- Test: `tests/status_checkouts.rs`

**Reuse:** 3 — `std::fs::read_dir` in depth-counted loops, and `std::fs::canonicalize` for realpaths

**Interfaces:**
- Consumes: `git::Git` (Task 3), `classify::{classify, CheckoutFacts, CheckoutKind, Class}` (Task 4), `registry::{Registry, Repo, FILE}`, `receipt::Receipt`, `cli::{emit, OutputArgs}`, `tests/common::{Hive, Scratch, git, init_repo}`
- Produces:
  - `state::{Config{secondary_roots: Vec<PathBuf>, cache_roots: Vec<PathBuf>}, CONFIG_FILE}` and `State::config(&self) -> Result<Config, StateError>`
  - `discover::{Checkout{path, kind, class, reason, missing}, checkouts(&Repo, registered_mains: &[PathBuf], &Config) -> Vec<Checkout>, git_checkouts_within(root: &Path, depth: usize) -> Vec<PathBuf>}`

**Depends-on:** Task 3, Task 4

- [x] **Step 1: Write the failing tests** — `tests/status_checkouts.rs`:

```rust
mod common;
use common::{git, init_repo, Hive, Scratch};
use std::path::Path;

fn class_of(receipt: &serde_json::Value, path: &Path) -> String {
    receipt["findings"].as_array().unwrap().iter()
        .find(|f| f["path"] == serde_json::json!(path) && f["kind"].as_str().unwrap().starts_with("checkout."))
        .map(|f| f["kind"].as_str().unwrap().trim_start_matches("checkout.").to_string())
        .unwrap_or_else(|| "absent".into())
}

fn register(hive: &Hive, repo: &Path) {
    let out = hive.run(&["repo", "add", repo.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn main_worktrees_and_clones_are_listed_with_their_class() {
    let hive = Hive::new();
    let s = Scratch::new("classes");
    let sec = s.path.join("sec");
    std::fs::create_dir_all(&sec).unwrap();
    hive.config(&format!("{{\"secondary_roots\": [\"{}\"]}}", sec.display()));
    let main = init_repo(&s.path.join("main"), "");
    git(&main, &["worktree", "add", "-q", main.join(".worktrees/lane1").to_str().unwrap()]);
    git(&main, &["worktree", "add", "-q", s.path.join("sibling").to_str().unwrap()]);
    git(&sec, &["clone", "-q", main.to_str().unwrap(), "run1/app"]);
    register(&hive, &main);
    let (out, receipt) = hive.json(&["status", main.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(class_of(&receipt, &main), "protected");
    assert_eq!(class_of(&receipt, &main.join(".worktrees/lane1")), "secondary");
    assert_eq!(class_of(&receipt, &s.path.join("sibling")), "protected");
    assert_eq!(class_of(&receipt, &sec.join("run1/app")), "secondary");
    assert_eq!(receipt["counts"]["checkouts"], 4);
}

#[test]
fn a_missing_worktree_is_listed_as_missing_even_when_locked() {
    let hive = Hive::new();
    let s = Scratch::new("missing");
    let main = init_repo(&s.path.join("main"), "");
    let gone = main.join(".worktrees/gone");
    git(&main, &["worktree", "add", "-q", gone.to_str().unwrap()]);
    git(&main, &["worktree", "lock", gone.to_str().unwrap()]);
    std::fs::remove_dir_all(&gone).unwrap();
    register(&hive, &main);
    let (_, receipt) = hive.json(&["status", main.to_str().unwrap()]);
    assert_eq!(class_of(&receipt, &gone), "missing");
    assert_eq!(receipt["counts"]["missing"], 1);
    let at_gone: Vec<&str> = receipt["findings"].as_array().unwrap().iter().filter(|f| f["path"] == serde_json::json!(gone)).map(|f| f["kind"].as_str().unwrap()).collect();
    assert_eq!(at_gone, vec!["checkout.missing"], "a missing worktree appears in no other result");
    let counts = &receipt["counts"];
    assert_eq!(counts["protected"].as_u64().unwrap() + counts["secondary"].as_u64().unwrap(), counts["checkouts"].as_u64().unwrap() - 1);
}

#[test]
fn secondary_roots_are_compared_as_realpaths() {
    let hive = Hive::new();
    let s = Scratch::new("realpath");
    let real_root = s.path.join("real-sec");
    std::fs::create_dir_all(&real_root).unwrap();
    let alias = s.path.join("alias-sec");
    std::os::unix::fs::symlink(&real_root, &alias).unwrap();
    hive.config(&format!("{{\"secondary_roots\": [\"{}\"]}}", alias.display()));
    let main = init_repo(&s.path.join("main"), "");
    git(&real_root, &["clone", "-q", main.to_str().unwrap(), "app"]);
    register(&hive, &main);
    let (_, receipt) = hive.json(&["status", main.to_str().unwrap()]);
    assert_eq!(class_of(&receipt, &real_root.join("app")), "secondary");
}

#[test]
fn without_configuration_private_tmp_is_a_secondary_root() {
    let hive = Hive::new();
    let s = Scratch::new("defaults"); // lives under /private/tmp
    let main = init_repo(&s.path.join("main"), "");
    git(&s.path, &["clone", "-q", main.to_str().unwrap(), "clone"]);
    register(&hive, &main);
    let (_, receipt) = hive.json(&["status", main.to_str().unwrap()]);
    // s.path is level 1 below /private/tmp, so its `clone` is level 2: inside the walk.
    assert_eq!(class_of(&receipt, &s.path.join("clone")), "secondary");
    assert!(receipt["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("/private/tmp")));
}

#[test]
fn configured_secondary_roots_replace_the_defaults() {
    let hive = Hive::new();
    let s = Scratch::new("replace");
    let other = s.path.join("other");
    std::fs::create_dir_all(&other).unwrap();
    hive.config(&format!("{{\"secondary_roots\": [\"{}\"]}}", other.display()));
    let main = init_repo(&s.path.join("main"), "");
    let lane = s.path.join("lane");
    git(&main, &["worktree", "add", "-q", lane.to_str().unwrap()]);
    register(&hive, &main);
    let (_, receipt) = hive.json(&["status", main.to_str().unwrap()]);
    assert_eq!(class_of(&receipt, &lane), "protected", "a sibling worktree under /private/tmp is protected once the defaults are replaced");
}

#[test]
fn status_without_arguments_reports_every_registered_repo_from_any_cwd() {
    let hive = Hive::new();
    let s = Scratch::new("all");
    let a = init_repo(&s.path.join("a"), "");
    let b = init_repo(&s.path.join("b"), "");
    register(&hive, &a);
    register(&hive, &b);
    let out = hive.command(&["status", "--json"]).current_dir(&a).output().unwrap();
    let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let mut repos: Vec<String> = receipt["scope"]["repos"].as_array().unwrap().iter().map(|r| r.as_str().unwrap().to_string()).collect();
    repos.sort();
    assert_eq!(repos, vec![a.display().to_string(), b.display().to_string()]);
}

#[test]
fn discovery_stops_at_two_levels_and_never_enters_node_modules() {
    let hive = Hive::new();
    let s = Scratch::new("bounded");
    let sec = s.path.join("sec");
    std::fs::create_dir_all(sec.join("a/b")).unwrap();
    std::fs::create_dir_all(sec.join("node_modules")).unwrap();
    hive.config(&format!("{{\"secondary_roots\": [\"{}\"]}}", sec.display()));
    let main = init_repo(&s.path.join("main"), "");
    git(&sec.join("a/b"), &["clone", "-q", main.to_str().unwrap(), "deep"]);
    git(&sec.join("node_modules"), &["clone", "-q", main.to_str().unwrap(), "pkg"]);
    git(&sec, &["clone", "-q", main.to_str().unwrap(), "near"]);
    register(&hive, &main);
    let (_, receipt) = hive.json(&["status", main.to_str().unwrap()]);
    assert_eq!(class_of(&receipt, &sec.join("near")), "secondary");
    assert_eq!(class_of(&receipt, &sec.join("a/b/deep")), "absent");
    assert_eq!(class_of(&receipt, &sec.join("node_modules/pkg")), "absent");
}

#[test]
fn an_unregistered_repo_argument_is_refused() {
    let hive = Hive::new();
    let s = Scratch::new("unknown");
    let (out, receipt) = hive.json(&["status", s.path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(receipt["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "refusal"));
}
```

Run: `cargo test --test status_checkouts` — expect: failure, because `status` is an unrecognized subcommand.

- [x] **Step 2: Add `Config` to `src/state.rs`**

```rust
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
        let cache = file.cache_roots.unwrap_or_else(|| home.iter().map(|h| h.join(".cache/cargo-target")).collect());
        Ok(Config { secondary_roots: dedup(secondary), cache_roots: dedup(cache) })
    }
}
```

- [x] **Step 3: Implement `src/discover.rs`**

```rust
//! Checkouts of a registered repo and (Task 13) its Cargo target dirs. Every walk is
//! depth-bounded and never enters `node_modules`, `target` or `.git`.

use crate::classify::{classify, CheckoutFacts, CheckoutKind, Class};
use crate::git::Git;
use crate::registry::Repo;
use crate::state::Config;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const SKIP_DIRS: [&str; 3] = ["node_modules", "target", ".git"];

#[derive(Debug, Clone, Serialize)]
pub struct Checkout {
    pub path: PathBuf,
    pub kind: CheckoutKind,
    pub class: Class,
    pub reason: String,
    pub missing: bool,
}

fn skipped(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|n| SKIP_DIRS.contains(&n))
}

/// Directories at most `depth` levels below `root` that are git checkouts (`.git` file
/// or directory). A found checkout is not descended into.
pub fn git_checkouts_within(root: &Path, depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if depth == 0 {
        return found;
    }
    let Ok(entries) = std::fs::read_dir(root) else { return found };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| !skipped(&e.file_name()) && e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    for dir in dirs {
        if dir.join(".git").exists() {
            found.push(dir);
        } else {
            found.extend(git_checkouts_within(&dir, depth - 1));
        }
    }
    found
}

pub fn checkouts(repo: &Repo, registered_mains: &[PathBuf], cfg: &Config) -> Vec<Checkout> {
    let mut out: Vec<Checkout> = Vec::new();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let push = |out: &mut Vec<Checkout>, path: PathBuf, kind: CheckoutKind, git: Result<(), String>| {
        let realpath = path.canonicalize().map_err(|e| e.to_string());
        let facts = CheckoutFacts { realpath: realpath.clone(), kind: kind.clone(), git };
        let (class, reason) = classify(&facts, registered_mains, &cfg.secondary_roots);
        out.push(Checkout { path: realpath.unwrap_or(path), kind, class, reason, missing: false });
    };
    match Git::at(&repo.main).worktrees() {
        Ok(records) => {
            for rec in records.into_iter().filter(|r| !r.bare) {
                let is_main = rec.path.canonicalize().ok().as_ref() == Some(&repo.main) || rec.path == repo.main;
                let kind = if is_main { CheckoutKind::Main } else { CheckoutKind::Linked { main: repo.main.clone() } };
                if !rec.path.exists() {
                    out.push(Checkout { path: rec.path, kind, class: Class::Protected, reason: "missing: the worktree directory does not exist".into(), missing: true });
                    continue;
                }
                seen.insert(rec.path.canonicalize().unwrap_or(rec.path.clone()));
                push(&mut out, rec.path, kind, Ok(()));
            }
        }
        Err(e) => {
            seen.insert(repo.main.clone());
            push(&mut out, repo.main.clone(), CheckoutKind::Main, Err(e.to_string()));
        }
    }
    let mut clones: Vec<PathBuf> = Vec::new();
    for root in &cfg.secondary_roots {
        for dir in git_checkouts_within(root, 2) {
            let real = dir.canonicalize().unwrap_or(dir.clone());
            if seen.contains(&real) {
                continue;
            }
            let g = Git::at(&real);
            let primary = matches!((g.git_dir(), g.common_dir()), (Ok(a), Ok(b)) if a == b);
            let not_sub = matches!(g.superproject(), Ok(None));
            let shares_root = g.root_commits().is_ok_and(|roots| !roots.is_disjoint(&repo.root_commits));
            if primary && not_sub && shares_root {
                seen.insert(real.clone());
                clones.push(real);
            }
        }
    }
    clones.sort();
    for c in clones {
        push(&mut out, c, CheckoutKind::Clone, Ok(()));
    }
    out
}
```

- [x] **Step 4: Add `status` to `src/cli.rs`**

Add the variant:

```rust
    /// Show checkouts (and, from Task 13, Cargo target dirs) of registered repos
    Status {
        /// Registered repos to report; none means every registered repo
        repos: Vec<std::path::PathBuf>,
        #[command(flatten)]
        out: OutputArgs,
    },
```

Add the dispatch arm `Command::Status { repos, out } => status(&state, &repos, out),`. Then add:

```rust
fn status(state: &State, args: &[std::path::PathBuf], out: OutputArgs) -> i32 {
    use crate::classify::Class;
    use crate::discover;
    let (reg, cfg) = match (state.read::<Registry>(registry::FILE), state.config()) {
        (Ok(r), Ok(c)) => (r, c),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("cargo-hive: {e}");
            return 1;
        }
    };
    let mut b = Receipt::new("status", Scope::default());
    b.note(format!("secondary_roots: {}", cfg.secondary_roots.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")));
    b.note(format!("cache_roots: {}", cfg.cache_roots.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")));
    let mut code = 0;
    let mut selected = Vec::new();
    if args.is_empty() {
        selected = reg.repos.clone();
    }
    for arg in args {
        let real = arg.canonicalize().unwrap_or(arg.clone());
        match reg.repos.iter().find(|r| r.main == real) {
            Some(r) => selected.push(r.clone()),
            None => {
                b.finding("refusal", Some(arg.clone()), "not a registered repository; run `cargo hive repo add` first");
                code = 1;
            }
        }
    }
    let mains: Vec<_> = reg.repos.iter().map(|r| r.main.clone()).collect();
    let (mut total, mut protected, mut secondary, mut missing) = (0u64, 0u64, 0u64, 0u64);
    let mut human = String::new();
    for repo in &selected {
        b.scope_mut().repos.push(repo.main.clone());
        human.push_str(&format!("{}\n", repo.main.display()));
        for c in discover::checkouts(repo, &mains, &cfg) {
            total += 1;
            let label = if c.missing {
                missing += 1;
                "missing"
            } else if c.class == Class::Secondary {
                secondary += 1;
                "secondary"
            } else {
                protected += 1;
                "protected"
            };
            human.push_str(&format!("  {label:<9} {}  ({})\n", c.path.display(), c.reason));
            b.finding(&format!("checkout.{label}"), Some(c.path.clone()), c.reason.clone());
            b.scope_mut().checkouts.push(c.path);
        }
    }
    b.count("checkouts", total).count("protected", protected).count("secondary", secondary).count("missing", missing);
    emit(state, out, b.finish(), &human, code)
}
```

`src/lib.rs` gains `pub mod discover;`.

Run: `cargo test --test status_checkouts` — expect: 8 passed. Then run `cargo test` and `cargo clippy --all-targets -- -D warnings`.

- [x] **Step 5: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(status): list each checkout as protected, secondary or missing"`

_Requirements: CORE-3.1, CORE-3.3, CORE-3.5, CORE-3.6, CORE-3.7, CORE-3.8, CORE-6.2_

---

### Task 6: Read a profile dir's fingerprints (signature, units, edges)

**Files:**
- Create: `src/fingerprint.rs`
- Modify: `src/lib.rs` (add `pub mod fingerprint;`)
- Test: `src/fingerprint.rs` (`#[cfg(test)]`)

**Reuse:** 5 — `serde_json` (`Value`, read as `u64`); the graph logic is new (rung 7)

**Interfaces:**
- Consumes: none
- Produces: `fingerprint::{KEYS, Format::{Supported, Unsupported(String)}, Unit{dir_name, kind, hash_hex, deps: Vec<u64>}, UnitGraph{units, edges: Vec<(usize, usize)>}, EdgeCounts{resolved, unresolved, ambiguous}, ProfileReport{path, format, units: usize, edges: Option<EdgeCounts>, graph: Option<UnitGraph>}, read_profile(&Path) -> ProfileReport, le_hex(u64) -> String}`

**Depends-on:** Task 1

- [x] **Step 1: Write the failing tests** at the bottom of `src/fingerprint.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch() -> PathBuf {
        // Tests run in parallel threads of one process: the counter keeps their dirs apart.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let k = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("hive-fp-{}-{k}-{n}", std::process::id()));
        fs::create_dir_all(p.join(".fingerprint")).unwrap();
        p
    }

    fn unit_json(deps: &[u64]) -> String {
        let deps: Vec<String> = deps.iter().map(|d| format!("[1,\"dep\",false,{d}]")).collect();
        format!(
            "{{\"rustc\":11722972603611519644,\"features\":\"[]\",\"declared_features\":\"[]\",\"target\":1,\"profile\":2,\"path\":3,\"deps\":[{}],\"local\":[],\"rustflags\":[],\"config\":4,\"compile_kind\":0}}",
            deps.join(",")
        )
    }

    fn write_unit(profile: &Path, dir: &str, stem: &str, value: u64, deps: &[u64]) {
        let d = profile.join(".fingerprint").join(dir);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join(stem), le_hex(value)).unwrap();
        fs::write(d.join(format!("{stem}.json")), unit_json(deps)).unwrap();
        fs::write(d.join("invoked.timestamp"), "This file has an mtime of when this was started.").unwrap();
    }

    #[test]
    fn edges_resolve_through_little_endian_hex_of_the_dependency_fingerprint() {
        assert_eq!(le_hex(0x0102_0304_0506_0708), "0807060504030201");
        let p = scratch();
        let (a, b, c) = (0x0102_0304_0506_0708u64, 17_112_352_188_342_185_299u64, 42u64);
        write_unit(&p, "b-0000000000000001", "lib-b", b, &[]);
        write_unit(&p, "a-0000000000000002", "lib-a", a, &[b]);
        write_unit(&p, "c-0000000000000003", "bin-c", c, &[a, b]);
        fs::create_dir_all(p.join(".fingerprint/empty-0000000000000004")).unwrap();
        let r = read_profile(&p);
        assert_eq!(r.format, Format::Supported);
        assert_eq!(r.units, 3, "the empty unit dir is ignored");
        assert_eq!(r.edges, Some(EdgeCounts { resolved: 3, unresolved: 0, ambiguous: 0 }));
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn unresolved_and_ambiguous_edges_are_counted() {
        let p = scratch();
        write_unit(&p, "x-0000000000000001", "lib-x", 7, &[]);
        write_unit(&p, "y-0000000000000002", "lib-y", 7, &[]);
        write_unit(&p, "z-0000000000000003", "test-z", 9, &[7, 1234]);
        let r = read_profile(&p);
        assert_eq!(r.edges, Some(EdgeCounts { resolved: 0, unresolved: 1, ambiguous: 1 }));
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn doc_and_run_build_script_units_are_accepted() {
        let p = scratch();
        write_unit(&p, "d-0000000000000001", "doc-lib-d", 5, &[]);
        write_unit(&p, "e-0000000000000002", "run-build-script-build-script-build", 6, &[5]);
        let r = read_profile(&p);
        assert_eq!(r.format, Format::Supported);
        let kinds: Vec<String> = r.graph.unwrap().units.iter().map(|u| u.kind.clone()).collect();
        assert!(kinds.contains(&"doc-lib".to_string()) && kinds.contains(&"run-build-script".to_string()), "{kinds:?}");
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn any_deviation_from_the_signature_makes_the_whole_profile_unsupported() {
        type Corrupt = Box<dyn Fn(&Path)>;
        let cases: Vec<(&str, Corrupt)> = vec![
            ("extra key", Box::new(|d: &Path| fs::write(d.join("lib-q.json"), unit_json(&[]).replacen('{', "{\"version\":1,", 1)).unwrap())),
            ("missing key", Box::new(|d: &Path| fs::write(d.join("lib-q.json"), unit_json(&[]).replace(",\"compile_kind\":0", "")).unwrap())),
            ("deps entry shape", Box::new(|d: &Path| fs::write(d.join("lib-q.json"), unit_json(&[]).replace("\"deps\":[]", "\"deps\":[[1,\"x\",false]]")).unwrap())),
            ("short hash", Box::new(|d: &Path| fs::write(d.join("lib-q"), "abc").unwrap())),
            ("uppercase hash", Box::new(|d: &Path| fs::write(d.join("lib-q"), "ABCDEF0123456789").unwrap())),
            ("two json files", Box::new(|d: &Path| fs::write(d.join("lib-r.json"), unit_json(&[])).unwrap())),
        ];
        for (name, corrupt) in cases {
            let p = scratch();
            write_unit(&p, "ok-0000000000000001", "lib-ok", 1, &[]);
            write_unit(&p, "q-0000000000000002", "lib-q", 2, &[]);
            corrupt(&p.join(".fingerprint/q-0000000000000002"));
            let r = read_profile(&p);
            assert!(matches!(r.format, Format::Unsupported(_)), "{name}: {:?}", r.format);
            assert_eq!(r.edges, None, "{name}");
            fs::remove_dir_all(p).unwrap();
        }
    }

    #[test]
    fn a_profile_without_a_fingerprint_dir_is_unsupported() {
        let p = scratch();
        fs::remove_dir_all(p.join(".fingerprint")).unwrap();
        fs::create_dir_all(p.join("build/pkg/0123456789abcdef/fingerprint")).unwrap();
        let r = read_profile(&p);
        assert!(matches!(r.format, Format::Unsupported(ref why) if why.contains(".fingerprint")));
        assert_eq!((r.units, r.edges), (0, None));
        fs::remove_dir_all(p).unwrap();
    }
}
```

Run: `cargo test --lib fingerprint` — expect: compile error, because `read_profile`, `le_hex` and the types are missing.

- [x] **Step 2: Implement `src/fingerprint.rs` (above the tests)**

```rust
//! Cargo `.fingerprint/` reader (ADR-0001). A unit dir holds exactly one `<H>.json`
//! and a sibling `<H>` with 16 lowercase hex chars. Its JSON must have exactly the 11
//! known keys with known types. Any deviation makes the whole profile dir unsupported.

use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const KEYS: [&str; 11] = ["rustc", "features", "declared_features", "target", "profile", "path", "deps", "local", "rustflags", "config", "compile_kind"];
const KINDS: [&str; 9] = ["run-build-script", "build-script", "doc-lib", "doc-bin", "test", "lib", "bin", "example", "bench"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "reason", rename_all = "lowercase")]
pub enum Format {
    Supported,
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unit {
    pub dir_name: String,
    pub kind: String,
    pub hash_hex: String,
    pub deps: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct EdgeCounts {
    pub resolved: u64,
    pub unresolved: u64,
    pub ambiguous: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnitGraph {
    pub units: Vec<Unit>,
    pub edges: Vec<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileReport {
    pub path: PathBuf,
    pub format: Format,
    pub units: usize,
    pub edges: Option<EdgeCounts>,
    #[serde(skip)]
    pub graph: Option<UnitGraph>,
}

/// Cargo's `to_hex`: the u64's 8 bytes in little-endian order, as lowercase hex.
pub fn le_hex(value: u64) -> String {
    value.to_le_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

fn kind_of(stem: &str) -> String {
    KINDS.iter().find(|k| stem == **k || stem.starts_with(&format!("{k}-"))).map(|k| k.to_string()).unwrap_or_else(|| "other".into())
}

fn is_hash(bytes: &[u8]) -> bool {
    bytes.len() == 16 && bytes.iter().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}

fn check_types(obj: &Map<String, Value>) -> Result<Vec<u64>, String> {
    if obj.len() != KEYS.len() || !KEYS.iter().all(|k| obj.contains_key(*k)) {
        let mut keys: Vec<&String> = obj.keys().collect();
        keys.sort();
        return Err(format!("unexpected key set {keys:?}"));
    }
    for k in ["rustc", "target", "profile", "path", "config"] {
        obj[k].as_u64().ok_or_else(|| format!("`{k}` is not an unsigned integer"))?;
    }
    if !(obj["compile_kind"].is_u64() || obj["compile_kind"].is_i64()) {
        return Err("`compile_kind` is not an integer".into());
    }
    for k in ["features", "declared_features"] {
        if !obj[k].is_string() {
            return Err(format!("`{k}` is not a string"));
        }
    }
    for k in ["local", "rustflags"] {
        if !obj[k].is_array() {
            return Err(format!("`{k}` is not an array"));
        }
    }
    let deps = obj["deps"].as_array().ok_or("`deps` is not an array")?;
    deps.iter()
        .map(|d| match d.as_array().map(Vec::as_slice) {
            Some([a, b, c, f]) if a.is_u64() && b.is_string() && c.is_boolean() && f.is_u64() => Ok(f.as_u64().unwrap()),
            _ => Err(format!("`deps` entry {d} is not [u64, string, bool, u64]")),
        })
        .collect()
}

fn read_unit(dir: &Path, json_name: &str) -> Result<Unit, String> {
    let stem = json_name.trim_end_matches(".json");
    let hash = fs::read(dir.join(stem)).map_err(|e| format!("hash file `{stem}`: {e}"))?;
    if !is_hash(&hash) {
        return Err(format!("hash file `{stem}` is not 16 lowercase hex characters"));
    }
    let bytes = fs::read(dir.join(json_name)).map_err(|e| format!("{json_name}: {e}"))?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{json_name}: {e}"))?;
    let obj = value.as_object().ok_or_else(|| format!("{json_name} is not a JSON object"))?;
    let deps = check_types(obj)?;
    Ok(Unit {
        dir_name: dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        kind: kind_of(stem),
        hash_hex: String::from_utf8(hash).expect("checked ASCII hex"),
        deps,
    })
}

pub fn read_profile(profile_dir: &Path) -> ProfileReport {
    let path = profile_dir.to_path_buf();
    let unsupported = |units: usize, reason: String| ProfileReport { path: path.clone(), format: Format::Unsupported(reason), units, edges: None, graph: None };
    let fp = profile_dir.join(".fingerprint");
    if !fp.is_dir() {
        return unsupported(0, "no .fingerprint/ directory (newer or unknown Cargo layout)".into());
    }
    let entries = match fs::read_dir(&fp) {
        Ok(e) => e,
        Err(e) => return unsupported(0, format!("cannot read .fingerprint/: {e}")),
    };
    let mut units = Vec::new();
    let mut counted = 0usize;
    let mut problem: Option<String> = None;
    let mut dirs: Vec<PathBuf> = entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        let names: Vec<String> = match fs::read_dir(&dir) {
            Ok(e) => e.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect(),
            Err(e) => {
                problem.get_or_insert(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        if names.is_empty() {
            continue;
        }
        let jsons: Vec<&String> = names.iter().filter(|n| n.ends_with(".json")).collect();
        if jsons.len() != 1 {
            problem.get_or_insert(format!("{}: {} fingerprint JSON files", dir.display(), jsons.len()));
            continue;
        }
        counted += 1;
        match read_unit(&dir, jsons[0]) {
            Ok(u) => units.push(u),
            Err(reason) => {
                problem.get_or_insert(format!("{}: {reason}", dir.display()));
            }
        }
    }
    if let Some(reason) = problem {
        return unsupported(counted, reason);
    }
    let mut index: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, u) in units.iter().enumerate() {
        index.entry(u.hash_hex.as_str()).or_default().push(i);
    }
    let mut counts = EdgeCounts::default();
    let mut edges = Vec::new();
    for (i, u) in units.iter().enumerate() {
        for dep in &u.deps {
            match index.get(le_hex(*dep).as_str()).map(Vec::as_slice) {
                Some([j]) => {
                    counts.resolved += 1;
                    edges.push((i, *j));
                }
                Some(_) => counts.ambiguous += 1,
                None => counts.unresolved += 1,
            }
        }
    }
    ProfileReport { path, format: Format::Supported, units: counted, edges: Some(counts), graph: Some(UnitGraph { units, edges }) }
}
```

Run: `cargo test --lib fingerprint` — expect: 5 passed.

- [x] **Step 3: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(fingerprint): exact-signature reader with little-endian edge resolution"`

_Requirements: CORE-4.7, CORE-4.8, CORE-4.14_

---

### Task 7: APFS file operations, clones and sharing attributes

**Files:**
- Modify: `src/platform/mod.rs` (add `TempDir`, `Sharing`, trait `Fs` with its first nine methods, `pub mod apfs`)
- Create: `src/platform/apfs.rs`
- Test: `src/platform/apfs.rs` (`#[cfg(test)]`)

**Reuse:** 5 — `libc` (`clonefile`, `getattrlist`, `mkdtemp`); 3 — `std::fs` (`hard_link`, `File::set_modified`, `MetadataExt::ino`); the trait shape is new (rung 7)

**Interfaces:**
- Consumes: `platform::{NotMacOs}` (Task 2)
- Produces:
  - `platform::TempDir::{adopt(PathBuf) -> TempDir, path(&self) -> &Path}`, which removes its directory on drop
  - `platform::Sharing{private_bytes: u64, clone_id: u64, may_share: bool, shares_all: bool}`
  - `platform::Fs` with `temp_dir(&self, parent: &Path, prefix: &str) -> io::Result<TempDir>`, `write_file(&Path, &[u8])`, `read(&Path) -> io::Result<Vec<u8>>`, `hardlink(src, dst)`, `set_mtime(&Path, SystemTime)`, `mtime(&Path) -> io::Result<SystemTime>`, `inode(&Path) -> io::Result<u64>`, `clone_file(src, dst)`, `sharing(&Path) -> io::Result<Sharing>`
  - `platform::apfs::Apfs::{new(holder: PathBuf) -> Apfs, holder(&self) -> &Path}` (macOS only); `holder` is the executable that implements `__hold-lock` (Task 8)

**Depends-on:** Task 2

- [x] **Step 1: Write the failing tests** at the bottom of the new `src/platform/apfs.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn apfs() -> Apfs {
        Apfs::new(PathBuf::from("/nonexistent/cargo-hive"))
    }

    fn tmp(label: &str) -> TempDir {
        apfs().temp_dir(Path::new("/private/tmp"), &format!("hive-apfs-{label}-")).unwrap()
    }

    #[test]
    fn a_temp_dir_is_created_with_its_prefix_and_removed_on_drop() {
        let t = tmp("drop");
        let path = t.path().to_path_buf();
        assert!(path.is_dir());
        assert!(path.file_name().unwrap().to_str().unwrap().starts_with("hive-apfs-drop-"));
        apfs().write_file(&path.join("f"), b"x").unwrap();
        drop(t);
        assert!(!path.exists());
    }

    #[test]
    fn a_clone_shares_every_block_until_it_is_rewritten() {
        let t = tmp("clone");
        let (src, dst) = (t.path().join("src"), t.path().join("dst"));
        apfs().write_file(&src, &vec![7u8; 1 << 20]).unwrap();
        let alone = apfs().sharing(&src).unwrap();
        assert_eq!(alone.private_bytes, 1 << 20);
        assert!(!alone.shares_all && !alone.may_share);
        apfs().clone_file(&src, &dst).unwrap();
        let (s, d) = (apfs().sharing(&src).unwrap(), apfs().sharing(&dst).unwrap());
        assert_eq!((s.private_bytes, d.private_bytes), (0, 0));
        assert!(d.shares_all && d.may_share);
        assert_eq!(s.clone_id, d.clone_id);
        apfs().write_file(&dst, &vec![9u8; 1 << 20]).unwrap();
        let rewritten = apfs().sharing(&dst).unwrap();
        assert_ne!(rewritten.clone_id, s.clone_id);
        assert!(!rewritten.shares_all);
        assert_eq!(apfs().read(&dst).unwrap()[0], 9);
    }

    #[test]
    fn hardlinks_share_an_inode_and_mtimes_can_be_set() {
        let t = tmp("links");
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        apfs().write_file(&a, b"same").unwrap();
        apfs().hardlink(&a, &b).unwrap();
        assert_eq!(apfs().inode(&a).unwrap(), apfs().inode(&b).unwrap());
        let fixed = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        apfs().set_mtime(&a, fixed).unwrap();
        assert_eq!(apfs().mtime(&b).unwrap(), fixed);
    }
}
```

The expected values come from a probe on the proof machine (2026-10-27, and SPIKES.md §11). For a 4 MiB file, `getattrlist` returned 28 bytes. The source had private bytes equal to its size and flags `0x1`. Its fresh clone had private 0, the same clone id, and flags `0x41`. A whole-file rewrite of the clone, in place or by rename, gave it a new clone id and cleared `0x40`.

Run: `cargo test --lib platform::apfs` — expect: compile error, because `Apfs`, `Fs` and `TempDir` are missing.

- [x] **Step 2: Add the types and the trait to `src/platform/mod.rs`** (above its tests)

```rust
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
```

- [x] **Step 3: Implement `src/platform/apfs.rs`** (above its tests)

```rust
//! macOS `Fs`: APFS clones and sharing attributes through `libc`.

use super::{Fs, Sharing, TempDir};
use std::ffi::{CString, OsString};
use std::fs;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// `<sys/attr.h>` extended flags that `libc` does not define.
const EF_MAY_SHARE_BLOCKS: u64 = 0x1;
const EF_SHARES_ALL_BLOCKS: u64 = 0x40;

pub struct Apfs {
    holder: PathBuf,
}

impl Apfs {
    pub fn new(holder: PathBuf) -> Apfs {
        Apfs { holder }
    }

    /// The executable that implements the hidden `__hold-lock` command.
    pub fn holder(&self) -> &Path {
        &self.holder
    }
}

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
}

impl Fs for Apfs {
    fn temp_dir(&self, parent: &Path, prefix: &str) -> io::Result<TempDir> {
        let mut template = c_path(&parent.join(format!("{prefix}XXXXXXXX")))?.into_bytes_with_nul();
        // SAFETY: `template` is a writable NUL-terminated buffer that mkdtemp fills in place.
        if unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) }.is_null() {
            return Err(io::Error::last_os_error());
        }
        template.pop();
        Ok(TempDir::adopt(PathBuf::from(OsString::from_vec(template))))
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        fs::write(path, bytes)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }

    fn hardlink(&self, src: &Path, dst: &Path) -> io::Result<()> {
        fs::hard_link(src, dst)
    }

    fn set_mtime(&self, path: &Path, t: SystemTime) -> io::Result<()> {
        fs::File::options().write(true).open(path)?.set_modified(t)
    }

    fn mtime(&self, path: &Path) -> io::Result<SystemTime> {
        fs::metadata(path)?.modified()
    }

    fn inode(&self, path: &Path) -> io::Result<u64> {
        Ok(fs::metadata(path)?.ino())
    }

    fn clone_file(&self, src: &Path, dst: &Path) -> io::Result<()> {
        let (s, d) = (c_path(src)?, c_path(dst)?);
        // SAFETY: both are valid NUL-terminated paths for the duration of the call.
        if unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), 0) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn sharing(&self, path: &Path) -> io::Result<Sharing> {
        let p = c_path(path)?;
        let mut list = libc::attrlist {
            bitmapcount: libc::ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: 0,
            volattr: 0,
            dirattr: 0,
            fileattr: 0,
            // With FSOPT_ATTR_CMN_EXTENDED this group carries the common-extended attributes.
            forkattr: libc::ATTR_CMNEXT_PRIVATESIZE | libc::ATTR_CMNEXT_CLONEID | libc::ATTR_CMNEXT_EXT_FLAGS,
        };
        let mut buf = [0u8; 64];
        // SAFETY: `list` and `buf` outlive the call; the kernel writes at most `buf.len()` bytes.
        let rc = unsafe {
            libc::getattrlist(p.as_ptr(), (&mut list as *mut libc::attrlist).cast(), buf.as_mut_ptr().cast(), buf.len(), libc::FSOPT_ATTR_CMN_EXTENDED)
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        // A u32 total length, then the attributes in bit order: PRIVATESIZE (off_t),
        // CLONEID (u64), EXT_FLAGS (u64).
        let word = |range: std::ops::Range<usize>| -> [u8; 8] { buf[range].try_into().expect("8 bytes") };
        let len = u32::from_ne_bytes(buf[0..4].try_into().expect("4 bytes"));
        if len < 28 {
            return Err(io::Error::other(format!("getattrlist returned {len} bytes; expected 28")));
        }
        let private = i64::from_ne_bytes(word(4..12));
        let flags = u64::from_ne_bytes(word(20..28));
        Ok(Sharing {
            private_bytes: u64::try_from(private).unwrap_or(0),
            clone_id: u64::from_ne_bytes(word(12..20)),
            may_share: flags & EF_MAY_SHARE_BLOCKS != 0,
            shares_all: flags & EF_SHARES_ALL_BLOCKS != 0,
        })
    }
}
```

Run: `cargo test --lib platform::apfs` — expect: 3 passed. Then run `cargo clippy --all-targets -- -D warnings`.

- [x] **Step 4: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(platform): APFS temp dirs, clones and block-sharing attributes"`

_Requirements: CORE-1.1_

---

### Task 8: Applesauce compression, and the lock holder and probe

**Files:**
- Modify: `src/platform/mod.rs` (add `CompressOutcome`, `LockKind`, `LockState`, `LockHolder`, three `Fs` methods, `hold_lock_forever`)
- Modify: `src/platform/apfs.rs` (implement them; Applesauce progress adapter)
- Modify: `src/cli.rs` (hidden `__hold-lock`, handled before the state root is opened)
- Test: `src/platform/apfs.rs` (`#[cfg(test)]`), `tests/platform_locks.rs`

**Reuse:** 5 — `applesauce` (`FileCompressor::recursive_compress`, `progress::{Progress, Task, SkipReason}`, `compressor::Kind::Lzfse`); 5 — `libc` (`flock`, `fcntl`, `UF_COMPRESSED`); 3 — `std::process::Command` for the holder process

**Interfaces:**
- Consumes: `platform::{Fs, TempDir}`, `platform::apfs::Apfs` (Task 7); `cli` (Task 2)
- Produces:
  - `platform::CompressOutcome::{Compressed, SkippedHardLink, Failed(String)}`
  - `platform::LockKind::{Flock, Fcntl}` with `as_arg(self) -> &'static str` and `parse(&str) -> Option<LockKind>`
  - `platform::LockState::{Free, Held}`
  - `platform::LockHolder::{new(impl FnOnce() + Send + 'static) -> LockHolder, release(self)}`, which also releases on drop
  - `Fs::compress(&self, &Path) -> CompressOutcome`, `Fs::hold_lock(&self, LockKind, &Path) -> io::Result<LockHolder>`, `Fs::probe_lock(&self, &Path) -> io::Result<LockState>`
  - `platform::hold_lock_forever(LockKind, &Path) -> io::Result<std::convert::Infallible>`
  - The hidden command `cargo-hive __hold-lock <flock|fcntl> <path>`. It prints `held` once it holds the lock, then waits to be killed. A bad kind exits 2.

**Depends-on:** Task 3, Task 7

- [x] **Step 1: Write the failing tests**

Add to the `tests` module of `src/platform/apfs.rs`:

```rust
    use crate::platform::CompressOutcome;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn compression_keeps_content_and_mtime_and_skips_hardlinked_files() {
        let t = tmp("compress");
        let content: Vec<u8> = (0..4 << 20).map(|i| (i % 251) as u8).collect();
        let fixed = UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let single = t.path().join("single");
        apfs().write_file(&single, &content).unwrap();
        apfs().set_mtime(&single, fixed).unwrap();
        assert_eq!(apfs().compress(&single), CompressOutcome::Compressed);
        assert_eq!(apfs().read(&single).unwrap(), content);
        assert_eq!(apfs().mtime(&single).unwrap(), fixed);

        let (a, b) = (t.path().join("a"), t.path().join("b"));
        apfs().write_file(&a, &content[..1 << 20]).unwrap();
        apfs().hardlink(&a, &b).unwrap();
        apfs().set_mtime(&a, fixed).unwrap();
        let inode = apfs().inode(&a).unwrap();
        assert_eq!(apfs().compress(&a), CompressOutcome::SkippedHardLink);
        assert_eq!(apfs().inode(&a).unwrap(), inode);
        assert_eq!(apfs().read(&a).unwrap(), &content[..1 << 20]);
        assert_eq!(apfs().mtime(&a).unwrap(), fixed);
        assert_eq!(fs::symlink_metadata(&a).unwrap().st_flags() & libc::UF_COMPRESSED, 0);
    }
```

`tests/platform_locks.rs`:

```rust
#![cfg(target_os = "macos")]
mod common;
use cargo_hive::platform::apfs::Apfs;
use cargo_hive::platform::{Fs, LockKind, LockState};
use common::{Hive, Scratch};
use std::io::{BufRead, BufReader};
use std::process::Stdio;
use std::time::{Duration, Instant};

fn apfs() -> Apfs {
    Apfs::new(env!("CARGO_BIN_EXE_cargo-hive").into())
}

#[test]
fn a_lock_held_by_another_process_is_reported_held_without_waiting() {
    let s = Scratch::new("locks");
    for kind in [LockKind::Flock, LockKind::Fcntl] {
        let path = s.path.join(format!("{}.lock", kind.as_arg()));
        std::fs::write(&path, b"").unwrap();
        let holder = apfs().hold_lock(kind, &path).unwrap();
        let started = Instant::now();
        assert_eq!(apfs().probe_lock(&path).unwrap(), LockState::Held, "{kind:?}");
        assert!(started.elapsed() < Duration::from_secs(1), "{kind:?}: the probe waited");
        holder.release();
        assert_eq!(apfs().probe_lock(&path).unwrap(), LockState::Free, "{kind:?}");
    }
}

#[test]
fn an_unknown_lock_kind_is_rejected() {
    let hive = Hive::new();
    let s = Scratch::new("badkind");
    let out = hive.run(&["__hold-lock", "nope", s.path.join("x").to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn the_lock_holder_never_creates_hive_state() {
    let hive = Hive::new();
    let s = Scratch::new("holder-state");
    std::fs::remove_dir_all(&hive.home.path).unwrap();
    let path = s.path.join("x.lock");
    let mut child = hive.command(&["__hold-lock", "flock", path.to_str().unwrap()]).stdout(Stdio::piped()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(line.trim(), "held");
    assert!(!hive.home.path.exists());
}
```

Run: `cargo test --lib platform::apfs` and `cargo test --test platform_locks`. Expect compile errors: `compress`, `hold_lock`, `probe_lock`, `LockKind` and the rest are missing.

- [x] **Step 2: Add the types and trait methods to `src/platform/mod.rs`**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompressOutcome {
    Compressed,
    SkippedHardLink,
    Failed(String),
}

impl std::fmt::Display for CompressOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompressOutcome::Compressed => write!(f, "compressed"),
            CompressOutcome::SkippedHardLink => write!(f, "skipped: hard link"),
            CompressOutcome::Failed(why) => write!(f, "failed: {why}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockKind {
    Flock,
    Fcntl,
}

impl LockKind {
    pub fn as_arg(self) -> &'static str {
        match self {
            LockKind::Flock => "flock",
            LockKind::Fcntl => "fcntl",
        }
    }

    pub fn parse(arg: &str) -> Option<LockKind> {
        match arg {
            "flock" => Some(LockKind::Flock),
            "fcntl" => Some(LockKind::Fcntl),
            _ => None,
        }
    }
}

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
        LockHolder { release: Some(Box::new(release)) }
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

/// Body of the hidden `__hold-lock` command: take the lock, print `held`, wait to be killed.
pub fn hold_lock_forever(kind: LockKind, path: &Path) -> io::Result<std::convert::Infallible> {
    #[cfg(target_os = "macos")]
    {
        apfs::hold_lock_forever(kind, path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (kind, path);
        Err(io::Error::new(io::ErrorKind::Unsupported, MACOS_ONLY_MESSAGE))
    }
}
```

Add to `trait Fs`:

```rust
    /// Applesauce LZFSE compression of one file. A file with other hard links is skipped.
    fn compress(&self, path: &Path) -> CompressOutcome;
    /// Holds `kind` on `path` in another process until the holder is released.
    fn hold_lock(&self, kind: LockKind, path: &Path) -> io::Result<LockHolder>;
    /// `flock(LOCK_SH | LOCK_NB)` on a read-only descriptor; never waits.
    fn probe_lock(&self, path: &Path) -> io::Result<LockState>;
```

- [x] **Step 3: Implement them in `src/platform/apfs.rs`**

Add the imports `use super::{CompressOutcome, LockHolder, LockKind, LockState};`, `use applesauce::compressor::Kind;`, `use applesauce::progress::{Progress, SkipReason, Task};`, `use applesauce::FileCompressor;`, `use std::io::{BufRead, BufReader, Write};`, `use std::os::macos::fs::MetadataExt as _;`, `use std::os::unix::io::AsRawFd;`, `use std::process::{Command, Stdio};`, `use std::sync::atomic::{AtomicBool, Ordering};` and `use std::sync::{Arc, Mutex, PoisonError};`. Then add:

```rust
fn record(slot: &Mutex<Option<String>>, why: String) {
    slot.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert(why);
}

/// Turns Applesauce's progress callbacks into one outcome for one file.
#[derive(Default)]
struct Watch {
    hard_link: AtomicBool,
    failure: Arc<Mutex<Option<String>>>,
}

struct WatchTask {
    failure: Arc<Mutex<Option<String>>>,
}

impl Progress for Watch {
    type Task = WatchTask;

    fn error(&self, path: &Path, message: &str) {
        record(&self.failure, format!("{}: {message}", path.display()));
    }

    fn file_skipped(&self, _path: &Path, why: SkipReason) {
        match why {
            SkipReason::HardLink => self.hard_link.store(true, Ordering::SeqCst),
            other => record(&self.failure, format!("skipped: {other}")),
        }
    }

    fn file_task(&self, _path: &Path, _size: u64) -> WatchTask {
        WatchTask { failure: Arc::clone(&self.failure) }
    }
}

impl Task for WatchTask {
    fn increment(&self, _amt: u64) {}

    fn error(&self, message: &str) {
        record(&self.failure, message.to_string());
    }

    fn not_compressible_enough(&self, _path: &Path) {
        record(&self.failure, "not compressible enough".to_string());
    }
}

/// Body of `__hold-lock`: take the lock (blocking), report it, then wait to be killed.
pub fn hold_lock_forever(kind: LockKind, path: &Path) -> io::Result<std::convert::Infallible> {
    let file = fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
    let fd = file.as_raw_fd();
    let rc = match kind {
        // SAFETY: `fd` is valid while `file` lives, which is forever below.
        LockKind::Flock => unsafe { libc::flock(fd, libc::LOCK_EX) },
        LockKind::Fcntl => {
            // SAFETY: an all-zero `flock` struct is valid; the fields that matter are set next.
            let mut lock: libc::flock = unsafe { std::mem::zeroed() };
            lock.l_type = libc::F_WRLCK as libc::c_short;
            lock.l_whence = libc::SEEK_SET as libc::c_short;
            // SAFETY: `lock` outlives the call and `fd` is valid.
            unsafe { libc::fcntl(fd, libc::F_SETLKW, &lock) }
        }
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut out = io::stdout().lock();
    writeln!(out, "held")?;
    out.flush()?;
    drop(out);
    loop {
        std::thread::park();
    }
}
```

Add to `impl Fs for Apfs`:

```rust
    fn compress(&self, path: &Path) -> CompressOutcome {
        let watch = Watch::default();
        let mut compressor = FileCompressor::new();
        let _stats = compressor.recursive_compress([path], Kind::Lzfse, 0.95, 5, &watch, true);
        if let Some(why) = watch.failure.lock().unwrap_or_else(PoisonError::into_inner).take() {
            return CompressOutcome::Failed(why);
        }
        if watch.hard_link.load(Ordering::SeqCst) {
            return CompressOutcome::SkippedHardLink;
        }
        match fs::symlink_metadata(path) {
            Ok(m) if m.st_flags() & libc::UF_COMPRESSED != 0 => CompressOutcome::Compressed,
            Ok(_) => CompressOutcome::Failed("the file is not marked UF_COMPRESSED afterwards".into()),
            Err(e) => CompressOutcome::Failed(e.to_string()),
        }
    }

    fn hold_lock(&self, kind: LockKind, path: &Path) -> io::Result<LockHolder> {
        let mut child = Command::new(&self.holder)
            .arg("__hold-lock")
            .arg(kind.as_arg())
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut line = String::new();
        let read = child.stdout.take().map(|o| BufReader::new(o).read_line(&mut line));
        if line.trim() != "held" {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other(format!("the lock holder did not take the lock ({read:?})")));
        }
        Ok(LockHolder::new(move || {
            let _ = child.kill();
            let _ = child.wait();
        }))
    }

    fn probe_lock(&self, path: &Path) -> io::Result<LockState> {
        let file = fs::File::open(path)?;
        // SAFETY: `file` owns a valid descriptor for the call; closing it drops the shared lock.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0 {
            return Ok(LockState::Free);
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            Ok(LockState::Held)
        } else {
            Err(err)
        }
    }
```

- [x] **Step 4: Add the hidden command to `src/cli.rs`**

Add the variant to `Command`:

```rust
    /// Hold a lock until killed (used by doctor's lock check)
    #[command(name = "__hold-lock", hide = true)]
    HoldLock { kind: String, path: std::path::PathBuf },
```

In `main_with`, right after a successful parse and before `State::open()`, add the following. The holder must never create hive state:

```rust
    if let Command::HoldLock { kind, path } = &cli.command {
        return hold_lock(kind, path);
    }
```

Add the arm `Command::HoldLock { .. } => unreachable!("handled before the state root is opened"),` to the dispatch `match`. Then add:

```rust
fn hold_lock(kind: &str, path: &std::path::Path) -> i32 {
    let Some(kind) = platform::LockKind::parse(kind) else {
        eprintln!("cargo-hive: unknown lock kind {kind:?} (expected flock or fcntl)");
        return 2;
    };
    match platform::hold_lock_forever(kind, path) {
        Ok(never) => match never {},
        Err(e) => {
            eprintln!("cargo-hive: could not hold the lock on {}: {e}", path.display());
            1
        }
    }
}
```

Run: `cargo test --lib platform::apfs` and `cargo test --test platform_locks` — expect: 4 and 3 passed. Then run `cargo test` and `cargo clippy --all-targets -- -D warnings`.

- [x] **Step 5: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(platform): applesauce compression, lock holder process and non-blocking probe"`

_Requirements: CORE-1.1_

---

### Task 9: Owner-authentication availability, and the platform handle

**Files:**
- Modify: `src/platform/mod.rs` (add `Availability`, trait `Approve`, `Platform`, `current()`, `pub mod local_auth`)
- Create: `src/platform/local_auth.rs`
- Test: `src/platform/local_auth.rs` (`#[cfg(test)]`)

**Reuse:** 5 — `objc2-local-authentication` (`LAContext::new`, `canEvaluatePolicy_error`, `LAPolicy::DeviceOwnerAuthentication`)

**Interfaces:**
- Consumes: `platform::{Fs, NotMacOs}`, `platform::apfs::Apfs` (Tasks 2, 7, 8)
- Produces:
  - `platform::Availability::{Available, Unavailable(String)}`
  - `platform::Approve` with `owner_auth_available(&self) -> Availability`
  - `platform::Platform{fs: Box<dyn Fs>, approve: Box<dyn Approve>}`
  - `platform::current() -> Result<Platform, NotMacOs>`
  - `platform::local_auth::LocalAuth`

**Depends-on:** Task 8

- [x] **Step 1: Write the failing test** at the bottom of the new `src/platform/local_auth.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn availability_is_answered_without_waiting_for_a_person() {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(LocalAuth.owner_auth_available());
        });
        let answer = rx.recv_timeout(Duration::from_secs(20)).expect("the availability query blocked; it must only query");
        if let Availability::Unavailable(reason) = answer {
            assert!(!reason.is_empty());
        }
    }
}
```

Run: `cargo test --lib platform::local_auth` — expect: compile error, because `LocalAuth`, `Approve` and `Availability` are missing.

- [x] **Step 2: Add to `src/platform/mod.rs`**

```rust
#[cfg(target_os = "macos")]
pub mod local_auth;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Available,
    Unavailable(String),
}

/// Owner approval. This feature only asks whether it is available; ROAD-7 adds `evaluate`.
pub trait Approve {
    fn owner_auth_available(&self) -> Availability;
}

pub struct Platform {
    pub fs: Box<dyn Fs>,
    pub approve: Box<dyn Approve>,
}

#[cfg(target_os = "macos")]
pub fn current() -> Result<Platform, NotMacOs> {
    let holder = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("cargo-hive"));
    Ok(Platform { fs: Box::new(apfs::Apfs::new(holder)), approve: Box::new(local_auth::LocalAuth) })
}

#[cfg(not(target_os = "macos"))]
pub fn current() -> Result<Platform, NotMacOs> {
    Err(NotMacOs)
}
```

- [x] **Step 3: Implement `src/platform/local_auth.rs`** (above its test)

```rust
//! macOS `Approve`: device-owner authentication availability. `canEvaluatePolicy:error:`
//! only queries; `evaluatePolicy` is never called here.

use super::{Approve, Availability};
use objc2_local_authentication::{LAContext, LAPolicy};

pub struct LocalAuth;

impl Approve for LocalAuth {
    fn owner_auth_available(&self) -> Availability {
        // SAFETY: the context is created and queried on this thread only, and the query
        // presents no UI.
        unsafe {
            let context = LAContext::new();
            match context.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthentication) {
                Ok(()) => Availability::Available,
                Err(e) => Availability::Unavailable(format!("{} (LAError {})", e.localizedDescription(), e.code())),
            }
        }
    }
}
```

Run: `cargo test --lib platform::local_auth` — expect: 1 passed. Then run `cargo clippy --all-targets -- -D warnings`.

- [x] **Step 4: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(platform): owner-authentication availability without any dialog"`

_Requirements: CORE-1.1_

---

### Task 10: `doctor` checks a–e, the capability map and the `require` gate

**Files:**
- Create: `src/doctor.rs`
- Modify: `src/cli.rs` (add `doctor`)
- Modify: `src/lib.rs` (add `pub mod doctor;`)
- Test: `src/doctor.rs` (`#[cfg(test)]`, with a fake `Fs` and `Approve`), `tests/doctor.rs`

**Reuse:** none — new code (rung 7), composing `platform`, `fingerprint` and `state`

**Interfaces:**
- Consumes: `platform::{Fs, Approve, Availability, CompressOutcome, LockKind, LockState, LockHolder, Sharing, TempDir, current, os_version, major_version}` (Tasks 2, 7–9); `receipt::{Receipt, Scope, CheckRecord, Measured}`; `state::State`; `cli::{emit, OutputArgs}`
- Produces:
  - `doctor::{CAPABILITIES_FILE, MACOS_FLOOR}`
  - `doctor::CheckId::{A, B, C, D, E, F}` with `letter(self) -> &'static str` and `disables(self) -> &'static [Capability]`
  - `doctor::Verdict::{Pass, Fail, Unavailable}` with `as_str`
  - `doctor::Check{id, verdict, measured: BTreeMap<String, Measured>}`
  - `doctor::Capability::{Seeding, StoreCompression, EvictionAccounting, Deletion, Approval, FingerprintReading}` with `describe`
  - `doctor::Capabilities{toolchain_release: Option<String>, disabled: BTreeMap<Capability, BTreeSet<CheckId>>}`, stored as `capabilities.json`
  - `doctor::Toolchain{name: Option<String>, cargo: PathBuf, rustc: PathBuf, release: String}` and `doctor::resolve_toolchain(cwd: &Path) -> Result<Toolchain, String>`
  - `doctor::DoctorEnv{tmp_parent: PathBuf, macos: Option<String>, toolchain: Result<Toolchain, String>}`
  - `doctor::DoctorReport{macos, below_floor, toolchain_release, checks: Vec<Check>, stale_removed: u64}`
  - `doctor::DoctorError::TempDir(io::Error)`
  - `doctor::run(&dyn Fs, &dyn Approve, &DoctorEnv) -> Result<DoctorReport, DoctorError>`
  - `doctor::capabilities_for(&DoctorReport) -> Capabilities` and `doctor::floor_note(&DoctorReport) -> Option<String>`
  - `doctor::CapabilityRefusal::{RunDoctor(String), Disabled{capability, checks}}`
  - `doctor::require(&Capabilities, Capability, active_release: Option<&str>) -> Result<(), CapabilityRefusal>`

**Depends-on:** Task 9

Rule settled here: a check whose verdict is not `pass` disables its capabilities, so `unavailable` disables them too. CORE-1.3 names failures. An unavailable check, for example owner authentication with no enrolled credential, cannot be trusted either, so the gate fails closed.

- [x] **Step 1: Write the failing unit tests** at the bottom of the new `src/doctor.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Availability, Sharing, TempDir};
    use std::collections::HashMap;
    use std::os::unix::fs::MetadataExt;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    #[derive(Clone)]
    struct Knobs {
        clone_private: u64,
        shares_all: bool,
        rewrite_keeps_id: bool,
        single: CompressOutcome,
        linked: CompressOutcome,
        compress_touches_mtime: bool,
        probe_sees_locks: bool,
    }

    impl Default for Knobs {
        fn default() -> Knobs {
            Knobs {
                clone_private: 0,
                shares_all: true,
                rewrite_keeps_id: false,
                single: CompressOutcome::Compressed,
                linked: CompressOutcome::SkippedHardLink,
                compress_touches_mtime: false,
                probe_sees_locks: true,
            }
        }
    }

    /// Real files in a scratch dir; sharing, compression and locks are scripted by `Knobs`.
    struct FakeFs {
        knobs: Knobs,
        ids: Mutex<HashMap<PathBuf, u64>>,
        next: AtomicU64,
        held: Arc<AtomicBool>,
    }

    impl FakeFs {
        fn new(knobs: Knobs) -> FakeFs {
            FakeFs { knobs, ids: Mutex::new(HashMap::new()), next: AtomicU64::new(1), held: Arc::new(AtomicBool::new(false)) }
        }
    }

    impl Fs for FakeFs {
        fn temp_dir(&self, parent: &Path, prefix: &str) -> io::Result<TempDir> {
            let path = parent.join(format!("{prefix}{}", self.next.fetch_add(1, Ordering::SeqCst)));
            std::fs::create_dir_all(&path)?;
            Ok(TempDir::adopt(path))
        }
        fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
            std::fs::write(path, bytes)?;
            let mut ids = self.ids.lock().unwrap();
            if !(self.knobs.rewrite_keeps_id && ids.contains_key(path)) {
                ids.insert(path.to_path_buf(), self.next.fetch_add(1, Ordering::SeqCst));
            }
            Ok(())
        }
        fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
            std::fs::read(path)
        }
        fn hardlink(&self, src: &Path, dst: &Path) -> io::Result<()> {
            std::fs::hard_link(src, dst)
        }
        fn set_mtime(&self, path: &Path, t: SystemTime) -> io::Result<()> {
            std::fs::File::options().write(true).open(path)?.set_modified(t)
        }
        fn mtime(&self, path: &Path) -> io::Result<SystemTime> {
            std::fs::metadata(path)?.modified()
        }
        fn inode(&self, path: &Path) -> io::Result<u64> {
            Ok(std::fs::metadata(path)?.ino())
        }
        fn clone_file(&self, src: &Path, dst: &Path) -> io::Result<()> {
            std::fs::copy(src, dst)?;
            let mut ids = self.ids.lock().unwrap();
            let id = ids[src];
            ids.insert(dst.to_path_buf(), id);
            Ok(())
        }
        fn sharing(&self, path: &Path) -> io::Result<Sharing> {
            let ids = self.ids.lock().unwrap();
            let id = ids.get(path).copied().unwrap_or(0);
            let shared = ids.values().filter(|v| **v == id).count() > 1;
            let len = std::fs::metadata(path)?.len();
            Ok(Sharing { private_bytes: if shared { self.knobs.clone_private } else { len }, clone_id: id, may_share: shared, shares_all: shared && self.knobs.shares_all })
        }
        fn compress(&self, path: &Path) -> CompressOutcome {
            if self.knobs.compress_touches_mtime {
                let _ = self.set_mtime(path, SystemTime::now());
            }
            match std::fs::metadata(path) {
                Ok(m) if m.nlink() > 1 => self.knobs.linked.clone(),
                _ => self.knobs.single.clone(),
            }
        }
        fn hold_lock(&self, _kind: LockKind, _path: &Path) -> io::Result<LockHolder> {
            self.held.store(true, Ordering::SeqCst);
            let held = Arc::clone(&self.held);
            Ok(LockHolder::new(move || held.store(false, Ordering::SeqCst)))
        }
        fn probe_lock(&self, _path: &Path) -> io::Result<LockState> {
            Ok(if self.knobs.probe_sees_locks && self.held.load(Ordering::SeqCst) { LockState::Held } else { LockState::Free })
        }
    }

    struct FakeApprove(Availability);

    impl Approve for FakeApprove {
        fn owner_auth_available(&self) -> Availability {
            self.0.clone()
        }
    }

    fn scratch(label: &str) -> PathBuf {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("hive-doctor-unit-{label}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn env(parent: &Path, macos: &str) -> DoctorEnv {
        DoctorEnv { tmp_parent: parent.to_path_buf(), macos: Some(macos.into()), toolchain: Err("not used by this test".into()) }
    }

    /// Capabilities from checks a–e only: check f needs a real toolchain (Task 11).
    fn caps_a_to_e(report: &DoctorReport) -> Capabilities {
        let mut only = report.clone();
        only.checks.retain(|c| c.id != CheckId::F);
        capabilities_for(&only)
    }

    #[test]
    fn a_healthy_machine_passes_checks_a_to_e_and_disables_nothing() {
        let parent = scratch("healthy");
        let report = run(&FakeFs::new(Knobs::default()), &FakeApprove(Availability::Available), &env(&parent, "27.0.1")).unwrap();
        for c in report.checks.iter().filter(|c| c.id != CheckId::F) {
            assert_eq!(c.verdict, Verdict::Pass, "{:?}: {:?}", c.id, c.measured);
            assert!(!c.measured.is_empty(), "{:?} has no measured value", c.id);
        }
        assert!(caps_a_to_e(&report).disabled.is_empty());
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn each_failing_check_disables_exactly_its_capabilities() {
        use Capability::*;
        let cases: Vec<(&str, Knobs, CheckId, Vec<Capability>)> = vec![
            ("clone keeps private bytes", Knobs { clone_private: 4096, ..Knobs::default() }, CheckId::A, vec![Seeding]),
            ("compression fails", Knobs { single: CompressOutcome::Failed("x".into()), ..Knobs::default() }, CheckId::B, vec![StoreCompression]),
            ("hard-linked file compressed", Knobs { linked: CompressOutcome::Compressed, ..Knobs::default() }, CheckId::B, vec![StoreCompression]),
            ("compression moves the mtime", Knobs { compress_touches_mtime: true, ..Knobs::default() }, CheckId::B, vec![StoreCompression]),
            ("no shares-all flag", Knobs { shares_all: false, ..Knobs::default() }, CheckId::C, vec![EvictionAccounting]),
            ("rewrite keeps the clone id", Knobs { rewrite_keeps_id: true, ..Knobs::default() }, CheckId::C, vec![EvictionAccounting]),
            ("probe misses held locks", Knobs { probe_sees_locks: false, ..Knobs::default() }, CheckId::D, vec![Deletion]),
        ];
        for (name, knobs, failing, mut want) in cases {
            let parent = scratch("table");
            let report = run(&FakeFs::new(knobs), &FakeApprove(Availability::Available), &env(&parent, "27.0.1")).unwrap();
            for c in report.checks.iter().filter(|c| c.id != CheckId::F) {
                let expected = if c.id == failing { Verdict::Fail } else { Verdict::Pass };
                assert_eq!(c.verdict, expected, "{name}: check {:?} {:?}", c.id, c.measured);
            }
            want.sort();
            assert_eq!(caps_a_to_e(&report).disabled.keys().copied().collect::<Vec<_>>(), want, "{name}");
            std::fs::remove_dir_all(parent).unwrap();
        }
    }

    #[test]
    fn unavailable_owner_authentication_disables_approval() {
        let parent = scratch("noauth");
        let report = run(&FakeFs::new(Knobs::default()), &FakeApprove(Availability::Unavailable("no passcode".into())), &env(&parent, "27.0.1")).unwrap();
        let e = report.checks.iter().find(|c| c.id == CheckId::E).unwrap();
        assert_eq!(e.verdict, Verdict::Unavailable);
        assert_eq!(e.measured["reason"], Measured::Text("no passcode".into()));
        assert_eq!(caps_a_to_e(&report).disabled.get(&Capability::Approval), Some(&BTreeSet::from([CheckId::E])));
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn check_f_disables_reading_deletion_and_seeding() {
        let mut want = vec![Capability::FingerprintReading, Capability::Deletion, Capability::Seeding];
        want.sort();
        let mut got = CheckId::F.disables().to_vec();
        got.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn a_macos_below_the_floor_is_flagged_and_every_check_still_runs() {
        let parent = scratch("floor");
        let report = run(&FakeFs::new(Knobs::default()), &FakeApprove(Availability::Available), &env(&parent, "26.4")).unwrap();
        assert!(report.below_floor);
        for id in [CheckId::A, CheckId::B, CheckId::C, CheckId::D, CheckId::E] {
            assert!(report.checks.iter().any(|c| c.id == id), "{id:?} did not run");
        }
        let note = floor_note(&report).expect("a note for a version below the floor");
        assert!(note.contains("26.4") && note.contains("below the declared floor 27"), "{note}");
        let current = run(&FakeFs::new(Knobs::default()), &FakeApprove(Availability::Available), &env(&parent, "27.0.1")).unwrap();
        assert_eq!(floor_note(&current), None);
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn require_refuses_without_results_for_another_toolchain_and_for_a_disabled_capability() {
        let none = Capabilities::default();
        assert!(matches!(require(&none, Capability::FingerprintReading, Some("1.97.1")), Err(CapabilityRefusal::RunDoctor(_))));
        let other = Capabilities { toolchain_release: Some("1.96.1".into()), disabled: BTreeMap::new() };
        let e = require(&other, Capability::FingerprintReading, Some("1.97.1")).unwrap_err().to_string();
        assert!(e.contains("cargo hive doctor") && e.contains("1.96.1"), "{e}");
        assert!(matches!(require(&other, Capability::FingerprintReading, None), Err(CapabilityRefusal::RunDoctor(_))));
        let disabled = BTreeMap::from([(Capability::FingerprintReading, BTreeSet::from([CheckId::F]))]);
        let off = Capabilities { toolchain_release: Some("1.97.1".into()), disabled };
        let e = require(&off, Capability::FingerprintReading, Some("1.97.1")).unwrap_err().to_string();
        assert!(e.contains("(f)"), "{e}");
        assert_eq!(require(&off, Capability::Approval, Some("1.97.1")), Ok(()));
    }

    #[test]
    fn the_active_toolchain_release_comes_from_rustc() {
        let t = resolve_toolchain(&std::env::temp_dir()).unwrap();
        assert!(t.release.starts_with(|c: char| c.is_ascii_digit()), "{t:?}");
    }
}
```

Run: `cargo test --lib doctor` — expect: compile error, because the doctor module does not exist.

- [x] **Step 2: Implement `src/doctor.rs`** (above the tests)

```rust
//! `cargo hive doctor`: capability checks in a fresh temp dir, the check → capability map,
//! and the `require` gate every capability-dependent command calls.

use crate::platform::{Approve, Availability, CompressOutcome, Fs, LockHolder, LockKind, LockState};
use crate::platform::major_version;
use crate::receipt::Measured;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, UNIX_EPOCH};

pub const CAPABILITIES_FILE: &str = "capabilities.json";
pub const MACOS_FLOOR: u32 = 27;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckId {
    A,
    B,
    C,
    D,
    E,
    F,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Seeding,
    StoreCompression,
    EvictionAccounting,
    Deletion,
    Approval,
    FingerprintReading,
}

impl CheckId {
    pub fn letter(self) -> &'static str {
        match self {
            CheckId::A => "a",
            CheckId::B => "b",
            CheckId::C => "c",
            CheckId::D => "d",
            CheckId::E => "e",
            CheckId::F => "f",
        }
    }

    /// The capabilities this check vouches for: a check that does not pass disables them.
    pub fn disables(self) -> &'static [Capability] {
        match self {
            CheckId::A => &[Capability::Seeding],
            CheckId::B => &[Capability::StoreCompression],
            CheckId::C => &[Capability::EvictionAccounting],
            CheckId::D => &[Capability::Deletion],
            CheckId::E => &[Capability::Approval],
            CheckId::F => &[Capability::FingerprintReading, Capability::Deletion, Capability::Seeding],
        }
    }
}

impl Capability {
    pub fn describe(self) -> &'static str {
        match self {
            Capability::Seeding => "seeding",
            Capability::StoreCompression => "store compression",
            Capability::EvictionAccounting => "store eviction accounting and private-byte receipts",
            Capability::Deletion => "deletion",
            Capability::Approval => "approval",
            Capability::FingerprintReading => "fingerprint graph reading",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Pass,
    Fail,
    Unavailable,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Fail => "fail",
            Verdict::Unavailable => "unavailable",
        }
    }
}

type Measures = BTreeMap<String, Measured>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub id: CheckId,
    pub verdict: Verdict,
    pub measured: Measures,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    /// `None` until doctor has run.
    pub toolchain_release: Option<String>,
    pub disabled: BTreeMap<Capability, BTreeSet<CheckId>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Toolchain {
    pub name: Option<String>,
    pub cargo: PathBuf,
    pub rustc: PathBuf,
    pub release: String,
}

pub struct DoctorEnv {
    pub tmp_parent: PathBuf,
    pub macos: Option<String>,
    pub toolchain: Result<Toolchain, String>,
}

#[derive(Debug, Clone)]
pub struct DoctorReport {
    pub macos: Option<String>,
    pub below_floor: bool,
    pub toolchain_release: Option<String>,
    pub checks: Vec<Check>,
    pub stale_removed: u64,
}

#[derive(Debug)]
pub enum DoctorError {
    TempDir(io::Error),
}

impl fmt::Display for DoctorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DoctorError::TempDir(e) => write!(f, "could not create doctor's temporary directory: {e}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityRefusal {
    RunDoctor(String),
    Disabled { capability: Capability, checks: BTreeSet<CheckId> },
}

impl fmt::Display for CapabilityRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CapabilityRefusal::RunDoctor(why) => write!(f, "run `cargo hive doctor` first: {why}"),
            CapabilityRefusal::Disabled { capability, checks } => {
                let ids: Vec<String> = checks.iter().map(|c| format!("({})", c.letter())).collect();
                write!(f, "{} is disabled on this machine because doctor check {} did not pass", capability.describe(), ids.join(", "))
            }
        }
    }
}

/// The gate: capability `cap` may be used only if doctor ran for the active toolchain
/// release and none of the checks vouching for `cap` failed.
pub fn require(caps: &Capabilities, cap: Capability, active_release: Option<&str>) -> Result<(), CapabilityRefusal> {
    let Some(recorded) = caps.toolchain_release.as_deref() else {
        return Err(CapabilityRefusal::RunDoctor("no doctor results are recorded for this toolchain".into()));
    };
    let Some(active) = active_release else {
        return Err(CapabilityRefusal::RunDoctor("the active Rust toolchain could not be determined".into()));
    };
    if recorded != active {
        return Err(CapabilityRefusal::RunDoctor(format!("doctor ran with Rust {recorded}, but the active toolchain is Rust {active}")));
    }
    match caps.disabled.get(&cap) {
        Some(checks) => Err(CapabilityRefusal::Disabled { capability: cap, checks: checks.clone() }),
        None => Ok(()),
    }
}

pub fn capabilities_for(report: &DoctorReport) -> Capabilities {
    let mut disabled: BTreeMap<Capability, BTreeSet<CheckId>> = BTreeMap::new();
    for check in report.checks.iter().filter(|c| c.verdict != Verdict::Pass) {
        for cap in check.id.disables() {
            disabled.entry(*cap).or_default().insert(check.id);
        }
    }
    Capabilities { toolchain_release: report.toolchain_release.clone(), disabled }
}

/// The report line for a macOS below the declared floor; doctor reports it and still runs every check.
pub fn floor_note(report: &DoctorReport) -> Option<String> {
    report.below_floor.then(|| {
        let v = report.macos.clone().unwrap_or_default();
        format!("macOS {v} is below the declared floor {MACOS_FLOOR}; every check still ran, and its results decide support here")
    })
}

fn rustup_which(tool: &str, cwd: &Path) -> Result<Option<PathBuf>, String> {
    match Command::new("rustup").args(["which", tool]).current_dir(cwd).env("RUSTUP_AUTO_INSTALL", "0").stdin(Stdio::null()).output() {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("could not run rustup: {e}")),
        Ok(out) if out.status.success() => Ok(Some(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))),
        Ok(out) => Err(format!("rustup which {tool} failed: {}", String::from_utf8_lossy(&out.stderr).trim())),
    }
}

/// The toolchain a build started in `cwd` would use: rustup's real binaries, or the
/// `cargo` and `rustc` on PATH when rustup is absent.
pub fn resolve_toolchain(cwd: &Path) -> Result<Toolchain, String> {
    let (cargo, rustc) = match (rustup_which("cargo", cwd)?, rustup_which("rustc", cwd)?) {
        (Some(c), Some(r)) => (c, r),
        _ => (PathBuf::from("cargo"), PathBuf::from("rustc")),
    };
    // `…/toolchains/<name>/bin/rustc`
    let name = rustc
        .parent()
        .and_then(Path::parent)
        .filter(|dir| dir.parent().is_some_and(|p| p.ends_with("toolchains")))
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().to_string());
    let out = Command::new(&rustc)
        .arg("-vV")
        .current_dir(cwd)
        .env("RUSTUP_AUTO_INSTALL", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run {}: {e}", rustc.display()))?;
    if !out.status.success() {
        return Err(format!("{} -vV failed: {}", rustc.display(), String::from_utf8_lossy(&out.stderr).trim()));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let release = text.lines().find_map(|l| l.strip_prefix("release: ")).map(str::trim).ok_or("rustc -vV printed no release line")?.to_string();
    Ok(Toolchain { name, cargo, rustc, release })
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| ((i % 251) as u8).wrapping_add(seed)).collect()
}

fn failed(id: CheckId, e: impl fmt::Display) -> Check {
    Check { id, verdict: Verdict::Fail, measured: Measures::from([("error".to_string(), Measured::Text(e.to_string()))]) }
}

fn verdict(ok: bool) -> Verdict {
    if ok {
        Verdict::Pass
    } else {
        Verdict::Fail
    }
}

/// (a) A clone shares all its blocks: zero private bytes for source and clone.
fn check_clone_sharing(fs: &dyn Fs, dir: &Path) -> Check {
    let attempt = || -> io::Result<Check> {
        let (src, dst) = (dir.join("a-src"), dir.join("a-clone"));
        fs.write_file(&src, &pattern(1 << 20, 0))?;
        fs.clone_file(&src, &dst)?;
        let (s, d) = (fs.sharing(&src)?, fs.sharing(&dst)?);
        let measured = Measures::from([
            ("src_private_bytes".to_string(), Measured::Int(s.private_bytes)),
            ("clone_private_bytes".to_string(), Measured::Int(d.private_bytes)),
        ]);
        Ok(Check { id: CheckId::A, verdict: verdict(s.private_bytes == 0 && d.private_bytes == 0), measured })
    };
    attempt().unwrap_or_else(|e| failed(CheckId::A, e))
}

/// (b) Compression round-trips a single-link file and leaves a hard-linked one alone.
fn check_compression(fs: &dyn Fs, dir: &Path) -> Check {
    let attempt = || -> io::Result<Check> {
        let fixed = UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let single = dir.join("b-single");
        let content = pattern(4 << 20, 0);
        fs.write_file(&single, &content)?;
        fs.set_mtime(&single, fixed)?;
        let single_outcome = fs.compress(&single);
        let single_identical = fs.read(&single)? == content;
        let single_mtime = fs.mtime(&single)? == fixed;
        let (first, second) = (dir.join("b-link1"), dir.join("b-link2"));
        let linked = pattern(1 << 20, 1);
        fs.write_file(&first, &linked)?;
        fs.hardlink(&first, &second)?;
        fs.set_mtime(&first, fixed)?;
        let inode = fs.inode(&first)?;
        let link_outcome = fs.compress(&first);
        let link_unchanged = fs.inode(&first)? == inode && fs.read(&first)? == linked && fs.mtime(&first)? == fixed;
        let ok = single_outcome == CompressOutcome::Compressed && single_identical && single_mtime && link_outcome == CompressOutcome::SkippedHardLink && link_unchanged;
        let measured = Measures::from([
            ("single_outcome".to_string(), Measured::Text(single_outcome.to_string())),
            ("single_identical".to_string(), Measured::Bool(single_identical)),
            ("single_mtime_unchanged".to_string(), Measured::Bool(single_mtime)),
            ("hardlink_outcome".to_string(), Measured::Text(link_outcome.to_string())),
            ("hardlink_unchanged".to_string(), Measured::Bool(link_unchanged)),
        ]);
        Ok(Check { id: CheckId::B, verdict: verdict(ok), measured })
    };
    attempt().unwrap_or_else(|e| failed(CheckId::B, e))
}

/// (c) A clone carries SHARES_ALL and its source's clone id; a whole-file rewrite gives it a new id.
fn check_sharing_flags(fs: &dyn Fs, dir: &Path) -> Check {
    let attempt = || -> io::Result<Check> {
        let (src, dst) = (dir.join("c-src"), dir.join("c-clone"));
        fs.write_file(&src, &pattern(1 << 20, 2))?;
        fs.clone_file(&src, &dst)?;
        let (s, d) = (fs.sharing(&src)?, fs.sharing(&dst)?);
        fs.write_file(&dst, &pattern(1 << 20, 3))?;
        let after = fs.sharing(&dst)?;
        let same_id = d.clone_id == s.clone_id;
        let new_id = after.clone_id != s.clone_id;
        let measured = Measures::from([
            ("clone_shares_all".to_string(), Measured::Bool(d.shares_all)),
            ("same_clone_id".to_string(), Measured::Bool(same_id)),
            ("new_clone_id_after_rewrite".to_string(), Measured::Bool(new_id)),
        ]);
        Ok(Check { id: CheckId::C, verdict: verdict(d.shares_all && same_id && new_id), measured })
    };
    attempt().unwrap_or_else(|e| failed(CheckId::C, e))
}

/// (d) A non-blocking probe sees a lock another process holds, for both lock kinds.
fn check_lock_probe(fs: &dyn Fs, dir: &Path) -> Check {
    let attempt = || -> io::Result<Check> {
        let mut measured = Measures::new();
        let mut ok = true;
        for kind in [LockKind::Flock, LockKind::Fcntl] {
            let path = dir.join(format!("d-{}.lock", kind.as_arg()));
            fs.write_file(&path, b"")?;
            let holder: LockHolder = fs.hold_lock(kind, &path)?;
            let held = fs.probe_lock(&path)? == LockState::Held;
            holder.release();
            let free = fs.probe_lock(&path)? == LockState::Free;
            measured.insert(format!("{}_reported_held", kind.as_arg()), Measured::Bool(held));
            measured.insert(format!("{}_free_after_release", kind.as_arg()), Measured::Bool(free));
            ok &= held && free;
        }
        Ok(Check { id: CheckId::D, verdict: verdict(ok), measured })
    };
    attempt().unwrap_or_else(|e| failed(CheckId::D, e))
}

/// (e) Owner authentication: availability only, never a dialog.
fn check_owner_auth(approve: &dyn Approve) -> Check {
    match approve.owner_auth_available() {
        Availability::Available => Check { id: CheckId::E, verdict: Verdict::Pass, measured: Measures::from([("available".to_string(), Measured::Bool(true))]) },
        Availability::Unavailable(reason) => Check { id: CheckId::E, verdict: Verdict::Unavailable, measured: Measures::from([("reason".to_string(), Measured::Text(reason))]) },
    }
}

pub fn run(fs: &dyn Fs, approve: &dyn Approve, env: &DoctorEnv) -> Result<DoctorReport, DoctorError> {
    let tmp = fs.temp_dir(&env.tmp_parent, &format!("hive-doctor-{}-", std::process::id())).map_err(DoctorError::TempDir)?;
    let dir = tmp.path();
    let steps: [&dyn Fn() -> Check; 5] = [
        &|| check_clone_sharing(fs, dir),
        &|| check_compression(fs, dir),
        &|| check_sharing_flags(fs, dir),
        &|| check_lock_probe(fs, dir),
        &|| check_owner_auth(approve),
    ];
    let mut checks = Vec::new();
    for step in steps {
        checks.push(step());
    }
    Ok(DoctorReport {
        macos: env.macos.clone(),
        below_floor: env.macos.as_deref().and_then(major_version).is_some_and(|m| m < MACOS_FLOOR),
        toolchain_release: env.toolchain.as_ref().ok().map(|t| t.release.clone()),
        checks,
        stale_removed: 0,
    })
}
```

`tmp` lives until `run` returns, so the temp dir is removed on every return path and on unwind.

Run: `cargo test --lib doctor` — expect: 7 passed.

- [x] **Step 3: Write the failing integration test** — the new `tests/doctor.rs`:

```rust
#![cfg(target_os = "macos")]
mod common;
use common::Hive;
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

/// Waits for `child` up to `limit`; a hang fails the test instead of blocking the suite.
pub fn wait_within(mut child: Child, limit: Duration) -> Output {
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > limit {
            let _ = child.kill();
            panic!("doctor did not finish within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().unwrap()
}

fn doctor(hive: &Hive) -> (Output, serde_json::Value) {
    let child = hive.command(&["doctor", "--json"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let out = wait_within(child, Duration::from_secs(300));
    let receipt = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    (out, receipt)
}

const EXPECTED_IDS: [&str; 5] = ["a", "b", "c", "d", "e"];

#[test]
fn doctor_reports_every_check_with_its_measured_value_and_records_capabilities() {
    let hive = Hive::new();
    let (out, receipt) = doctor(&hive);
    let checks = receipt["checks"].as_array().unwrap();
    let ids: Vec<&str> = checks.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, EXPECTED_IDS);
    for c in checks {
        assert!(["pass", "fail", "unavailable"].contains(&c["verdict"].as_str().unwrap()), "{c}");
        assert!(!c["measured"].as_object().unwrap().is_empty(), "{c}");
        if c["id"] != "e" {
            assert_eq!(c["verdict"], "pass", "on the proof machine every filesystem check passes: {c}");
        }
    }
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let caps: serde_json::Value = serde_json::from_slice(&std::fs::read(hive.home.path.join("capabilities.json")).unwrap()).unwrap();
    assert_eq!(caps["toolchain_release"], receipt["toolchain"]);
    assert!(receipt["toolchain"].as_str().is_some());
}
```

Run: `cargo test --test doctor` — expect: failure, because `doctor` is an unrecognized subcommand.

- [x] **Step 4: Add `doctor` to `src/cli.rs`**

Add the variant:

```rust
    /// Check which hive capabilities work on this machine
    Doctor {
        #[command(flatten)]
        out: OutputArgs,
    },
```

Add the dispatch arm `Command::Doctor { out } => doctor_cmd(&state, out),`. Then add:

```rust
fn measured_text(m: &std::collections::BTreeMap<String, crate::receipt::Measured>) -> String {
    use crate::receipt::Measured;
    m.iter()
        .map(|(k, v)| match v {
            Measured::Int(n) => format!("{k}={n}"),
            Measured::Bool(b) => format!("{k}={b}"),
            Measured::Text(t) => format!("{k}={t:?}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn doctor_cmd(state: &State, out: OutputArgs) -> i32 {
    use crate::doctor::{self, DoctorEnv, Verdict};
    use crate::receipt::CheckRecord;
    let Ok(platform) = platform::current() else {
        eprintln!("{MACOS_ONLY_MESSAGE}");
        return 2;
    };
    let macos = platform::os_version();
    let env = DoctorEnv { tmp_parent: std::env::temp_dir(), macos: macos.clone(), toolchain: doctor::resolve_toolchain(state.root()) };
    let report = match doctor::run(&*platform.fs, &*platform.approve, &env) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cargo-hive: {e}");
            return 1;
        }
    };
    let caps = doctor::capabilities_for(&report);
    if let Err(e) = state.locked(|tx| tx.write(doctor::CAPABILITIES_FILE, &caps)) {
        eprintln!("cargo-hive: {e}");
        return 1;
    }
    let mut b = Receipt::with_os("doctor", Scope::default(), macos);
    b.toolchain(report.toolchain_release.clone());
    if let Err(reason) = &env.toolchain {
        b.unmeasured("toolchain", reason.clone());
    }
    let mut human = String::new();
    let (mut pass, mut fail, mut unavailable) = (0u64, 0u64, 0u64);
    for c in &report.checks {
        match c.verdict {
            Verdict::Pass => pass += 1,
            Verdict::Fail => fail += 1,
            Verdict::Unavailable => unavailable += 1,
        }
        human.push_str(&format!("({}) {:<11} {}\n", c.id.letter(), c.verdict.as_str(), measured_text(&c.measured)));
        b.check(CheckRecord { id: c.id.letter().into(), verdict: c.verdict.as_str().into(), measured: c.measured.clone() });
    }
    for (cap, checks) in &caps.disabled {
        let ids: Vec<String> = checks.iter().map(|c| format!("({})", c.letter())).collect();
        let line = format!("{} is disabled: doctor check {} did not pass", cap.describe(), ids.join(", "));
        human.push_str(&format!("{line}\n"));
        b.finding("capability.disabled", None, line);
    }
    if let Some(line) = doctor::floor_note(&report) {
        human.push_str(&format!("{line}\n"));
        b.note(line);
    }
    b.count("pass", pass).count("fail", fail).count("unavailable", unavailable).count("stale_temp_dirs_removed", report.stale_removed);
    emit(state, out, b.finish(), &human, i32::from(fail > 0))
}
```

`src/lib.rs` gains `pub mod doctor;`.

Run: `cargo test --test doctor` — expect: 1 passed. Then run `cargo test` and `cargo clippy --all-targets -- -D warnings`.

- [x] **Step 5: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(doctor): filesystem, lock and auth checks with a capability gate"`

_Requirements: CORE-1.1, CORE-1.2, CORE-1.3, CORE-1.4, CORE-1.7, CORE-1.10_

---

### Task 11: Doctor check (f): the active toolchain's fingerprint format

**Files:**
- Modify: `src/doctor.rs` (add `check_format` and the throwaway crate; `run` gains check f)
- Test: `tests/doctor.rs`

**Reuse:** 4 — the toolchain's real `cargo` from `rustup which` (Decision 7); 3 — `std::process::Command` with `env_clear`; existing — `fingerprint::read_profile` (Task 6)

**Interfaces:**
- Consumes: `fingerprint::{read_profile, Format}` (Task 6); `doctor::{Toolchain, Check, CheckId, Verdict}` (Task 10)
- Produces: `run` now returns checks a–f; `doctor::PROBE_CRATE: [(&str, &str); 5]`

**Depends-on:** Task 6, Task 10

- [x] **Step 1: Make the integration test expect six checks**

In `tests/doctor.rs`, change `EXPECTED_IDS` to `["a", "b", "c", "d", "e", "f"]` with type `[&str; 6]`. Then add:

```rust
#[test]
fn doctor_finishes_with_no_terminal_attached() {
    // `doctor()` spawns with stdin from /dev/null and no TTY; `wait_within` fails on a hang.
    let hive = Hive::new();
    let (_, receipt) = doctor(&hive);
    let f = receipt["checks"].as_array().unwrap().iter().find(|c| c["id"] == "f").unwrap().clone();
    assert_eq!(f["verdict"], "pass", "{f}");
    assert!(f["measured"]["units"].as_u64().unwrap() >= 4, "{f}");
    assert_eq!(f["measured"]["edges_unresolved"], 0, "{f}");
    assert_eq!(f["measured"]["edges_ambiguous"], 0, "{f}");
}
```

Run: `cargo test --test doctor` — expect: both tests fail, because only five checks are reported.

- [x] **Step 2: Implement check f in `src/doctor.rs`**

```rust
/// A crate with a lib, a bin, a build script and an integration test, and no dependencies.
pub const PROBE_CRATE: [(&str, &str); 5] = [
    ("Cargo.toml", "[package]\nname = \"hive-doctor-probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n"),
    ("build.rs", "fn main() {\n    println!(\"cargo:rerun-if-changed=build.rs\");\n}\n"),
    ("src/lib.rs", "pub fn answer() -> u32 {\n    42\n}\n"),
    ("src/main.rs", "fn main() {\n    println!(\"{}\", hive_doctor_probe::answer());\n}\n"),
    ("tests/it.rs", "#[test]\nfn answers() {\n    assert_eq!(hive_doctor_probe::answer(), 42);\n}\n"),
];

/// The allowlisted environment for the probe build (Decision 7); nothing else is inherited.
fn probe_command(tc: &Toolchain, krate: &Path, target: &Path, cargo_home: &Path) -> Command {
    let mut cmd = Command::new(&tc.cargo);
    cmd.args(["build", "--offline", "--all-targets", "--quiet"]).current_dir(krate).env_clear();
    for key in ["PATH", "HOME", "TMPDIR", "RUSTUP_HOME"] {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    if let Some(name) = &tc.name {
        cmd.env("RUSTUP_TOOLCHAIN", name);
    }
    cmd.env("RUSTUP_AUTO_INSTALL", "0").env("CARGO_HOME", cargo_home).env("CARGO_TARGET_DIR", target).env("RUSTC", &tc.rustc).stdin(Stdio::null()).stdout(Stdio::null());
    cmd
}

fn tail(text: &str, max: usize) -> String {
    let start = text.len().saturating_sub(max);
    let start = (start..=text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(text.len());
    text[start..].trim().to_string()
}

/// (f) The fingerprints the active toolchain writes match the supported signature.
fn check_format(fs: &dyn Fs, dir: &Path, toolchain: &Result<Toolchain, String>) -> Check {
    let tc = match toolchain {
        Ok(t) => t,
        Err(reason) => return Check { id: CheckId::F, verdict: Verdict::Unavailable, measured: Measures::from([("reason".to_string(), Measured::Text(reason.clone()))]) },
    };
    let attempt = || -> io::Result<Check> {
        let (krate, target, cargo_home) = (dir.join("f-crate"), dir.join("f-target"), dir.join("f-cargo-home"));
        for (rel, body) in PROBE_CRATE {
            let path = krate.join(rel);
            std::fs::create_dir_all(path.parent().expect("probe files live in the crate"))?;
            fs.write_file(&path, body.as_bytes())?;
        }
        std::fs::create_dir_all(&cargo_home)?;
        let stderr_path = dir.join("f-cargo.stderr");
        let mut child = probe_command(tc, &krate, &target, &cargo_home).stderr(std::fs::File::create(&stderr_path)?).spawn()?;
        let status = child.wait()?;
        if !status.success() {
            let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
            return Ok(Check {
                id: CheckId::F,
                verdict: Verdict::Fail,
                measured: Measures::from([
                    ("cargo_status".to_string(), Measured::Text(status.to_string())),
                    ("stderr_tail".to_string(), Measured::Text(tail(&stderr, 600))),
                ]),
            });
        }
        let report = crate::fingerprint::read_profile(&target.join("debug"));
        let mut measured = Measures::from([
            ("toolchain".to_string(), Measured::Text(tc.release.clone())),
            ("units".to_string(), Measured::Int(report.units as u64)),
        ]);
        let ok = match (&report.format, &report.edges) {
            (crate::fingerprint::Format::Supported, Some(e)) => {
                measured.insert("format".into(), Measured::Text("supported".into()));
                measured.insert("edges_resolved".into(), Measured::Int(e.resolved));
                measured.insert("edges_unresolved".into(), Measured::Int(e.unresolved));
                measured.insert("edges_ambiguous".into(), Measured::Int(e.ambiguous));
                report.units > 0 && e.resolved > 0 && e.unresolved == 0 && e.ambiguous == 0
            }
            (crate::fingerprint::Format::Unsupported(why), _) => {
                measured.insert("format".into(), Measured::Text(format!("unsupported: {why}")));
                false
            }
            (crate::fingerprint::Format::Supported, None) => false,
        };
        Ok(Check { id: CheckId::F, verdict: verdict(ok), measured })
    };
    attempt().unwrap_or_else(|e| failed(CheckId::F, e))
}
```

In `run`, after the loop over `steps`, add `checks.push(check_format(fs, dir, &env.toolchain));`.

Run: `cargo test --test doctor` — expect: 2 passed. Then run `cargo test --lib doctor`. Expect 7 passed: the unit tests pass `toolchain: Err(..)`, so f is `unavailable` there, and `caps_a_to_e` ignores it.

- [x] **Step 3: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(doctor): check f builds a throwaway crate and reads its fingerprints"`

_Requirements: CORE-1.1, CORE-1.2, CORE-6.4_

---

### Task 12: Doctor cleanup on signals and panics, and the stale temp-dir sweep

**Files:**
- Modify: `src/doctor.rs` (add `CANCEL`, `install_signal_handlers`, `checkpoint`, `sweep_stale`, `DoctorError::Cancelled`; cancellable wait in check f)
- Modify: `src/cli.rs` (install handlers; exit 130 on cancel)
- Test: `src/doctor.rs` (`#[cfg(test)]`), `tests/doctor.rs`

**Reuse:** 5 — `libc` (`sigaction`, `sigemptyset`, `kill(pid, 0)`, `ESRCH`); 3 — `std::os::unix::process::CommandExt::process_group`

**Interfaces:**
- Consumes: `doctor::{run, DoctorEnv, DoctorError}` (Tasks 10–11)
- Produces: `doctor::{CANCEL: AtomicBool, install_signal_handlers() -> io::Result<()>, sweep_stale(&Path) -> u64}`, `DoctorError::Cancelled`, and `DoctorReport.stale_removed` filled in. `cargo hive doctor` exits 130 when interrupted.

**Depends-on:** Task 11

- [x] **Step 1: Write the failing tests**

Add to the `tests` module of `src/doctor.rs`. Add `panic_in_compress: bool` to `Knobs` with default `false`, and make `FakeFs::compress` start with `assert!(!self.knobs.panic_in_compress, "injected panic");`. `CANCEL` is process-global, so these tests serialize on `SERIAL`. Then:

```rust
    static SERIAL: Mutex<()> = Mutex::new(());

    fn leftovers(parent: &Path) -> Vec<String> {
        std::fs::read_dir(parent).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).filter(|n| n.starts_with("hive-doctor-")).collect()
    }

    #[test]
    fn the_temp_dir_is_removed_after_success_after_a_failed_check_and_after_a_panic() {
        let _serial = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let parent = scratch("cleanup");
        run(&FakeFs::new(Knobs::default()), &FakeApprove(Availability::Available), &env(&parent, "27.0.1")).unwrap();
        assert_eq!(leftovers(&parent), Vec::<String>::new(), "after success");
        run(&FakeFs::new(Knobs { probe_sees_locks: false, ..Knobs::default() }), &FakeApprove(Availability::Available), &env(&parent, "27.0.1")).unwrap();
        assert_eq!(leftovers(&parent), Vec::<String>::new(), "after a failed check");
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(&FakeFs::new(Knobs { panic_in_compress: true, ..Knobs::default() }), &FakeApprove(Availability::Available), &env(&parent, "27.0.1"))
        }));
        assert!(panicked.is_err());
        assert_eq!(leftovers(&parent), Vec::<String>::new(), "after a panic");
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn a_cancelled_run_stops_and_removes_its_temp_dir() {
        let _serial = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let parent = scratch("cancel");
        CANCEL.store(true, Ordering::SeqCst);
        let result = run(&FakeFs::new(Knobs::default()), &FakeApprove(Availability::Available), &env(&parent, "27.0.1"));
        CANCEL.store(false, Ordering::SeqCst);
        assert!(matches!(result, Err(DoctorError::Cancelled)));
        assert_eq!(leftovers(&parent), Vec::<String>::new());
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn the_sweep_removes_only_dirs_of_dead_doctor_processes() {
        let parent = scratch("sweep");
        let mut dead = Command::new("true").spawn().unwrap();
        let dead_pid = dead.id();
        dead.wait().unwrap();
        let mut alive = Command::new("sleep").arg("30").spawn().unwrap();
        let stale = parent.join(format!("hive-doctor-{dead_pid}-abcd1234"));
        let live = parent.join(format!("hive-doctor-{}-abcd1234", alive.id()));
        let odd = parent.join("hive-doctor-notapid-abcd1234");
        let file = parent.join(format!("hive-doctor-{dead_pid}-file"));
        for d in [&stale, &live, &odd] {
            std::fs::create_dir_all(d.join("inner")).unwrap();
        }
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(sweep_stale(&parent), 1);
        assert!(!stale.exists() && live.exists() && odd.exists() && file.exists());
        alive.kill().unwrap();
        alive.wait().unwrap();
        std::fs::remove_dir_all(parent).unwrap();
    }
```

The unit tests that call `run` without taking `SERIAL` (from Task 10) do not touch `CANCEL`. They can still observe it while `a_cancelled_run…` holds it set, so make every test that calls `run` take `SERIAL` first: add `let _serial = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);` as the first line of each.

Add to `tests/doctor.rs`:

```rust
use common::Scratch;

fn doctor_dirs(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("hive-doctor-")).collect()
}

#[test]
fn doctor_leaves_no_temp_dir_behind_and_sweeps_dead_runs() {
    let hive = Hive::new();
    let tmp = Scratch::new("doctor-tmpdir");
    let mut dead = std::process::Command::new("true").spawn().unwrap();
    let dead_pid = dead.id();
    dead.wait().unwrap();
    std::fs::create_dir_all(tmp.path.join(format!("hive-doctor-{dead_pid}-leftover1"))).unwrap();
    let child = hive.command(&["doctor", "--json"]).env("TMPDIR", &tmp.path).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let out = wait_within(child, Duration::from_secs(300));
    let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(receipt["counts"]["stale_temp_dirs_removed"], 1);
    assert_eq!(doctor_dirs(&tmp.path), Vec::<std::path::PathBuf>::new());
}

#[test]
fn an_interrupted_doctor_exits_130_and_removes_its_temp_dir() {
    for signal in ["-TERM", "-INT"] {
        let hive = Hive::new();
        let tmp = Scratch::new("doctor-signal");
        let child = hive.command(&["doctor"]).env("TMPDIR", &tmp.path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let pid = child.id().to_string();
        // Wait until the probe crate exists, so the signal lands inside the build of check f.
        let started = Instant::now();
        while !doctor_dirs(&tmp.path).iter().any(|d| d.join("f-crate/Cargo.toml").exists()) {
            assert!(started.elapsed() < Duration::from_secs(60), "doctor never reached check f");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(std::process::Command::new("kill").args([signal, &pid]).status().unwrap().success());
        let out = wait_within(child, Duration::from_secs(60));
        assert_eq!(out.status.code(), Some(130), "{signal}");
        assert_eq!(doctor_dirs(&tmp.path), Vec::<std::path::PathBuf>::new(), "{signal}");
    }
}
```

Run: `cargo test --lib doctor` and `cargo test --test doctor` — expect: compile errors, because `CANCEL`, `sweep_stale` and `DoctorError::Cancelled` are missing.

- [x] **Step 2: Implement in `src/doctor.rs`**

```rust
use std::os::unix::process::CommandExt;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set by SIGINT/SIGTERM; doctor polls it between steps and while check f builds.
pub static CANCEL: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_signal: libc::c_int) {
    CANCEL.store(true, Ordering::SeqCst);
}

pub fn install_signal_handlers() -> io::Result<()> {
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

fn checkpoint() -> Result<(), DoctorError> {
    if CANCEL.load(Ordering::SeqCst) {
        Err(DoctorError::Cancelled)
    } else {
        Ok(())
    }
}

/// Removes `hive-doctor-<pid>-*` dirs under `parent` whose process no longer exists.
pub fn sweep_stale(parent: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(parent) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.strip_prefix("hive-doctor-")).and_then(|rest| rest.split_once('-')).and_then(|(pid, _)| pid.parse::<i32>().ok()) else {
            continue;
        };
        if pid <= 0 || pid as u32 == std::process::id() || !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        // SAFETY: signal 0 delivers nothing; it only asks whether `pid` exists.
        let gone = unsafe { libc::kill(pid, 0) } == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if gone && std::fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}
```

Add the variant `Cancelled` to `DoctorError`, with the Display arm `DoctorError::Cancelled => write!(f, "interrupted; the temporary directory was removed"),`.

Replace `check_format` and `run` with these final versions. The probe build runs in its own process group: on cancel the whole group (cargo and its rustc children) is killed, and a Ctrl-C at the terminal does not reach it directly. The closure returns `io::Result<Result<Check, DoctorError>>`, so `?` keeps working for IO, and an IO failure becomes a failed check:

```rust
/// (f) The fingerprints the active toolchain writes match the supported signature.
fn check_format(fs: &dyn Fs, dir: &Path, toolchain: &Result<Toolchain, String>) -> Result<Check, DoctorError> {
    let tc = match toolchain {
        Ok(t) => t,
        Err(reason) => return Ok(Check { id: CheckId::F, verdict: Verdict::Unavailable, measured: Measures::from([("reason".to_string(), Measured::Text(reason.clone()))]) }),
    };
    let attempt = || -> io::Result<Result<Check, DoctorError>> {
        let (krate, target, cargo_home) = (dir.join("f-crate"), dir.join("f-target"), dir.join("f-cargo-home"));
        for (rel, body) in PROBE_CRATE {
            let path = krate.join(rel);
            std::fs::create_dir_all(path.parent().expect("probe files live in the crate"))?;
            fs.write_file(&path, body.as_bytes())?;
        }
        std::fs::create_dir_all(&cargo_home)?;
        let stderr_path = dir.join("f-cargo.stderr");
        let mut child = probe_command(tc, &krate, &target, &cargo_home).stderr(std::fs::File::create(&stderr_path)?).process_group(0).spawn()?;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if CANCEL.load(Ordering::SeqCst) {
                // SAFETY: the child leads its own process group; this signals only that group.
                unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                let _ = child.wait();
                return Ok(Err(DoctorError::Cancelled));
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        if !status.success() {
            let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
            return Ok(Ok(Check {
                id: CheckId::F,
                verdict: Verdict::Fail,
                measured: Measures::from([
                    ("cargo_status".to_string(), Measured::Text(status.to_string())),
                    ("stderr_tail".to_string(), Measured::Text(tail(&stderr, 600))),
                ]),
            }));
        }
        let report = crate::fingerprint::read_profile(&target.join("debug"));
        let mut measured = Measures::from([
            ("toolchain".to_string(), Measured::Text(tc.release.clone())),
            ("units".to_string(), Measured::Int(report.units as u64)),
        ]);
        let ok = match (&report.format, &report.edges) {
            (crate::fingerprint::Format::Supported, Some(e)) => {
                measured.insert("format".into(), Measured::Text("supported".into()));
                measured.insert("edges_resolved".into(), Measured::Int(e.resolved));
                measured.insert("edges_unresolved".into(), Measured::Int(e.unresolved));
                measured.insert("edges_ambiguous".into(), Measured::Int(e.ambiguous));
                report.units > 0 && e.resolved > 0 && e.unresolved == 0 && e.ambiguous == 0
            }
            (crate::fingerprint::Format::Unsupported(why), _) => {
                measured.insert("format".into(), Measured::Text(format!("unsupported: {why}")));
                false
            }
            (crate::fingerprint::Format::Supported, None) => false,
        };
        Ok(Ok(Check { id: CheckId::F, verdict: verdict(ok), measured }))
    };
    match attempt() {
        Ok(result) => result,
        Err(e) => Ok(failed(CheckId::F, e)),
    }
}

pub fn run(fs: &dyn Fs, approve: &dyn Approve, env: &DoctorEnv) -> Result<DoctorReport, DoctorError> {
    let stale_removed = sweep_stale(&env.tmp_parent);
    let tmp = fs.temp_dir(&env.tmp_parent, &format!("hive-doctor-{}-", std::process::id())).map_err(DoctorError::TempDir)?;
    let dir = tmp.path();
    let steps: [&dyn Fn() -> Check; 5] = [
        &|| check_clone_sharing(fs, dir),
        &|| check_compression(fs, dir),
        &|| check_sharing_flags(fs, dir),
        &|| check_lock_probe(fs, dir),
        &|| check_owner_auth(approve),
    ];
    let mut checks = Vec::new();
    for step in steps {
        checkpoint()?;
        checks.push(step());
    }
    checkpoint()?;
    checks.push(check_format(fs, dir, &env.toolchain)?);
    Ok(DoctorReport {
        macos: env.macos.clone(),
        below_floor: env.macos.as_deref().and_then(major_version).is_some_and(|m| m < MACOS_FLOOR),
        toolchain_release: env.toolchain.as_ref().ok().map(|t| t.release.clone()),
        checks,
        stale_removed,
    })
}
```

Every return path, including `?` on `Cancelled`, drops `tmp` and so removes the temp dir.

- [x] **Step 3: Handle cancellation in `src/cli.rs`**

In `doctor_cmd`, before `doctor::run`:

```rust
    if let Err(e) = doctor::install_signal_handlers() {
        eprintln!("cargo-hive: could not install signal handlers: {e}");
        return 1;
    }
```

Then add the match arm `Err(doctor::DoctorError::Cancelled) => { eprintln!("cargo-hive: doctor {}", doctor::DoctorError::Cancelled); return 130; }` before the generic `Err(e)` arm. No receipt is logged, because the command did not complete.

Run: `cargo test --lib doctor` and `cargo test --test doctor` — expect: 10 and 4 passed. Then run `cargo test` and `cargo clippy --all-targets -- -D warnings`.

- [x] **Step 4: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(doctor): remove the temp dir on signals and panics, sweep dead runs"`

_Requirements: CORE-1.8, CORE-1.9_

---

### Task 13: `status` refuses until doctor results exist for the active toolchain

**Files:**
- Modify: `src/cli.rs` (the gate at the top of `status`)
- Modify: `tests/common/mod.rs` (add `Hive::ready`, `Hive::write_ready_capabilities`; `Hive::config` defaults `cache_roots` to a scratch dir)
- Modify: `tests/status_checkouts.rs` (use `Hive::ready()`)
- Test: `tests/capability_gate.rs`

**Reuse:** existing — `doctor::{require, resolve_toolchain, Capabilities, Capability}` (Task 10)

**Interfaces:**
- Consumes: `doctor::{CAPABILITIES_FILE, Capabilities, Capability, require, resolve_toolchain}` (Task 10); `cli::status` (Task 5)
- Produces: `status` exits 1 with a `refusal` finding when `require(…, Capability::FingerprintReading, active)` refuses. It never scans before the gate. Test helpers: `Hive::ready() -> Hive` and `Hive::write_ready_capabilities(&self)`. `Hive::config` adds `"cache_roots": ["<home>/no-cache"]` when the given JSON has no `cache_roots`, so tests never walk the real `~/.cache/cargo-target`.

**Depends-on:** Task 5, Task 10

- [x] **Step 1: Add the helpers to `tests/common/mod.rs`**

Replace `Hive::config` and add two methods:

```rust
    /// Writes config.json. `cache_roots` defaults to `<home>/no-cache`, so tests never walk
    /// the real ~/.cache/cargo-target; pass it explicitly to override.
    pub fn config(&self, json: &str) {
        let mut value: serde_json::Value = serde_json::from_str(json).unwrap();
        value.as_object_mut().unwrap().entry("cache_roots").or_insert_with(|| serde_json::json!([self.home.path.join("no-cache")]));
        fs::write(self.home.path.join("config.json"), value.to_string()).unwrap();
    }

    /// A hive whose doctor results are recorded for the active toolchain with every
    /// capability enabled, and whose cache root is a scratch dir.
    pub fn ready() -> Hive {
        let hive = Hive::new();
        hive.write_ready_capabilities();
        hive.config("{}");
        hive
    }

    pub fn write_ready_capabilities(&self) {
        let release = cargo_hive::doctor::resolve_toolchain(&self.home.path).expect("the proof machine has a Rust toolchain").release;
        let caps = serde_json::json!({ "toolchain_release": release, "disabled": {} });
        fs::write(self.home.path.join("capabilities.json"), caps.to_string()).unwrap();
    }
```

In `tests/status_checkouts.rs`, replace every `Hive::new()` with `Hive::ready()`.

- [x] **Step 2: Write the failing tests** — `tests/capability_gate.rs`:

```rust
mod common;
use common::{init_repo, Hive, Scratch};
use std::fs;

fn refusal(receipt: &serde_json::Value) -> String {
    receipt["findings"].as_array().unwrap().iter().find(|f| f["kind"] == "refusal").map(|f| f["detail"].as_str().unwrap().to_string()).unwrap_or_default()
}

fn registered(hive: &Hive, s: &Scratch) -> String {
    let repo = init_repo(&s.path.join("app"), "");
    assert!(hive.run(&["repo", "add", repo.to_str().unwrap()]).status.success());
    repo.display().to_string()
}

#[test]
fn status_without_doctor_results_is_refused_and_asks_for_doctor() {
    let hive = Hive::new();
    let s = Scratch::new("gate-none");
    let repo = registered(&hive, &s);
    let (out, receipt) = hive.json(&["status", &repo]);
    assert_eq!(out.status.code(), Some(1));
    assert!(refusal(&receipt).contains("cargo hive doctor"), "{receipt}");
    assert!(receipt["findings"].as_array().unwrap().iter().all(|f| !f["kind"].as_str().unwrap().starts_with("checkout.")), "nothing is scanned before the gate");
}

#[test]
fn doctor_results_for_another_toolchain_are_refused() {
    let hive = Hive::new();
    let s = Scratch::new("gate-other");
    let repo = registered(&hive, &s);
    fs::write(hive.home.path.join("capabilities.json"), r#"{"toolchain_release":"0.0.0-elsewhere","disabled":{}}"#).unwrap();
    let (out, receipt) = hive.json(&["status", &repo]);
    assert_eq!(out.status.code(), Some(1));
    let why = refusal(&receipt);
    assert!(why.contains("cargo hive doctor") && why.contains("0.0.0-elsewhere"), "{why}");
}

#[test]
fn a_disabled_capability_names_the_failing_doctor_check() {
    let hive = Hive::ready();
    let s = Scratch::new("gate-disabled");
    let repo = registered(&hive, &s);
    let path = hive.home.path.join("capabilities.json");
    let mut caps: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    caps["disabled"] = serde_json::json!({ "fingerprint_reading": ["f"] });
    fs::write(&path, caps.to_string()).unwrap();
    let (out, receipt) = hive.json(&["status", &repo]);
    assert_eq!(out.status.code(), Some(1));
    let why = refusal(&receipt);
    assert!(why.contains("(f)") && why.contains("fingerprint graph reading"), "{why}");
}

#[test]
fn doctor_results_for_the_active_toolchain_let_status_run() {
    let hive = Hive::ready();
    let s = Scratch::new("gate-ok");
    let repo = registered(&hive, &s);
    let (out, receipt) = hive.json(&["status", &repo]);
    assert!(out.status.success(), "{receipt}");
    assert!(receipt["toolchain"].as_str().is_some());
}
```

Run: `cargo test --test capability_gate` — expect: the first three tests fail, because `status` exits 0 with no refusal.

- [x] **Step 3: Add the gate to `status` in `src/cli.rs`**

Right after the two `b.note(…)` lines, insert:

```rust
    let caps: crate::doctor::Capabilities = match state.read(crate::doctor::CAPABILITIES_FILE) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cargo-hive: {e}");
            return 1;
        }
    };
    let release = crate::doctor::resolve_toolchain(state.root()).ok().map(|t| t.release);
    b.toolchain(release.clone());
    if let Err(refusal) = crate::doctor::require(&caps, crate::doctor::Capability::FingerprintReading, release.as_deref()) {
        b.finding("refusal", None, refusal.to_string());
        return emit(state, out, b.finish(), &format!("refused: {refusal}\n"), 1);
    }
```

Run: `cargo test --test capability_gate --test status_checkouts` — expect: 4 and 8 passed. Then run `cargo test` and `cargo clippy --all-targets -- -D warnings`.

- [x] **Step 4: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(status): refuse until doctor results exist for the active toolchain"`

_Requirements: CORE-1.4, CORE-1.10_

---

### Task 14: `status` lists every Cargo target dir with its owner and fingerprint report

**Files:**
- Create: `src/cargo_target.rs`
- Modify: `src/discover.rs` (add `SNAPSHOT_FILE`, `Snapshot`, `CachedMetadata`, `workspace_dirs`, `find_targets`, `TargetScan`, `targets`)
- Modify: `src/cli.rs` (replace `status`; add `report_targets`)
- Modify: `src/lib.rs` (add `pub mod cargo_target;`)
- Modify: `src/state.rs` (one unit test for the documented defaults)
- Modify: `tests/common/mod.rs` (add `Hive::build_crate`)
- Test: `src/cargo_target.rs` (`#[cfg(test)]`), `tests/status_targets.rs`

**Reuse:** 4 — `cargo metadata` (Decision 4); 3 — `std::fs` for tag and layout probes, and `std::thread::scope` for up to 4 parallel resolutions; existing — `fingerprint::read_profile` (Task 6)

**Interfaces:**
- Consumes: `discover::{Checkout, SKIP_DIRS}` (Task 5); `fingerprint::{read_profile, Format, ProfileReport}` (Task 6); `state::Config`; the Task 13 gate
- Produces:
  - `cargo_target::{TAG_LINE, TARGET_VARS, Layout::{Fingerprint, NewBuildDir}}`
  - `cargo_target::identify(&Path) -> Option<Result<Vec<(PathBuf, Layout)>, String>>`
  - `cargo_target::Resolved{workspace_root, target_directory, build_directory}` and `cargo_target::resolve(&Path, impl FnMut() -> bool) -> Result<Resolved, String>`
  - `cargo_target::{realpath_lossy, caller_overrides}`
  - `cargo_target::Owner::{Owned{checkout}, Unresolved{reason}, OrphanCandidate{owned_under_caller_env: Option<String>}}`
  - `cargo_target::TargetDir{path, owner, profiles: Vec<ProfileReport>, size: Option<u64>, unreadable: Option<String>}`
  - `discover::{SNAPSHOT_FILE, Snapshot{metadata, unsupported_profiles}, CachedMetadata{stamp, resolved}, workspace_dirs, find_targets, TargetScan{targets, metadata_calls, metadata_cache_hits, ignored_vars, walk_faults, cleared_profiles}, targets(&[Checkout], &Config, all_repos: bool, &mut Snapshot, cancel: impl Fn() -> bool + Sync) -> TargetScan}`
  - Receipt names:
    - counts `targets`, `owned`, `orphan_candidates`, `unresolved_owners`, `unreadable`, `unsupported_profiles`, `units`, `edges_resolved`, `edges_unresolved`, `edges_ambiguous`, `metadata_calls`, `metadata_cache_hits`, plus `units:<profile>` and `edges_{resolved,unresolved,ambiguous}:<profile>` for each profile dir
    - findings `target.owned`, `target.orphan_candidate`, `target.unresolved`, `target.unreadable`, `profile.unsupported`
  - `Hive::build_crate(&self, dir: &Path, target_dir: Option<&Path>)`

**Depends-on:** Task 6, Task 13

Rules settled here:
- A target dir under a cache root is an orphan candidate only when the status run covers every registered repository. Otherwise it is `unresolved`, because a repository outside the run may own it.
- A target found in a checkout is an orphan candidate only when every queried workspace resolved.
- A resolved owner is attributed to the known checkout that most deeply contains the workspace root.

The build-dir probe (2026-10-25) found the following with `build.build-dir` set:
- The build dir has `CACHEDIR.TAG` and `debug/.fingerprint`, so it is a target here.
- The artifact dir has a tag and uplifted artifacts but no fingerprints, so it is not a target under CORE-4.12. Its bytes go unreported in this feature; ROAD-3 (explain) is where to surface them.

Realigned 2026-10-28: the close-fix rounds (`6edf221`, `55c9827`, `5505db9`) added the `walk_faults` list and the `cancel` callback to `targets` and `resolve_workspaces`, and `status` installs its signal handlers before calling `targets`. The signatures and calls in this task now show those parameters; the bodies below are the plan as executed, and `src/discover.rs` is the current code.

- [x] **Step 1: Write the failing unit tests** at the bottom of the new `src/cargo_target.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(label: &str) -> PathBuf {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("hive-target-{label}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn tag(dir: &Path, line: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("CACHEDIR.TAG"), format!("Signature: 8a477f597d28d172789f06886806bc55\n{line}\n")).unwrap();
    }

    fn mkdir(p: PathBuf) {
        fs::create_dir_all(p).unwrap();
    }

    #[test]
    fn a_target_needs_cargos_tag_line_and_a_profile_with_fingerprints() {
        let root = scratch("identify");
        let plain = root.join("plain");
        mkdir(plain.join("debug/.fingerprint"));
        assert_eq!(identify(&plain), None, "no tag");
        let other = root.join("other");
        tag(&other, "# This file is a cache directory tag created by someone else.");
        mkdir(other.join("debug/.fingerprint"));
        assert_eq!(identify(&other), None, "someone else's tag");
        let artifacts = root.join("artifacts");
        tag(&artifacts, TAG_LINE);
        mkdir(artifacts.join("debug/deps"));
        assert_eq!(identify(&artifacts), None, "uplifted artifacts only (a target dir whose build dir is elsewhere)");
        let old = root.join("old");
        tag(&old, TAG_LINE);
        for p in ["debug/.fingerprint", "release/.fingerprint", "aarch64-apple-darwin/debug/.fingerprint", "doc/fixture"] {
            mkdir(old.join(p));
        }
        assert_eq!(
            identify(&old),
            Some(Ok(vec![(old.join("aarch64-apple-darwin/debug"), Layout::Fingerprint), (old.join("debug"), Layout::Fingerprint), (old.join("release"), Layout::Fingerprint)]))
        );
        let new = root.join("new");
        tag(&new, TAG_LINE);
        mkdir(new.join("debug/build/serde/0123456789abcdef/fingerprint"));
        assert_eq!(identify(&new), Some(Ok(vec![(new.join("debug"), Layout::NewBuildDir)])));
        let not_hash = root.join("not-hash");
        tag(&not_hash, TAG_LINE);
        mkdir(not_hash.join("debug/build/serde-0123/out"));
        assert_eq!(identify(&not_hash), None);
        let locked = root.join("locked");
        tag(&locked, TAG_LINE);
        mkdir(locked.join("debug/.fingerprint"));
        fs::set_permissions(locked.join("debug"), fs::Permissions::from_mode(0o000)).unwrap();
        let got = identify(&locked);
        fs::set_permissions(locked.join("debug"), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(&got, Some(Err(why)) if why.contains("debug")), "{got:?}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_missing_tail_is_appended_to_the_realpath_of_its_nearest_existing_ancestor() {
        let root = scratch("lossy");
        assert_eq!(realpath_lossy(&root.join("a/b")), root.canonicalize().unwrap().join("a/b"));
        fs::remove_dir_all(root).unwrap();
    }
}
```

Add to the `tests` module of `src/state.rs`:

```rust
    #[test]
    fn without_a_config_file_the_documented_defaults_apply_per_key() {
        let root = scratch("defaults");
        let state = State::open_at(root.clone()).unwrap();
        let canon = |p: PathBuf| p.canonicalize().unwrap_or(p);
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let cfg = state.config().unwrap();
        assert_eq!(cfg.cache_roots, vec![canon(home.join(".cache/cargo-target"))]);
        assert_eq!(cfg.secondary_roots[0], canon(PathBuf::from("/private/tmp")));
        if let Some(t) = std::env::var_os("TMPDIR") {
            assert!(cfg.secondary_roots.contains(&canon(PathBuf::from(t))));
        }
        fs::write(root.join(CONFIG_FILE), br#"{"cache_roots": ["/private/tmp"]}"#).unwrap();
        let cfg = state.config().unwrap();
        assert_eq!(cfg.cache_roots, vec![PathBuf::from("/private/tmp")]);
        assert_eq!(cfg.secondary_roots[0], PathBuf::from("/private/tmp"), "an unset key keeps its default");
        fs::remove_dir_all(root).unwrap();
    }
```

Add to `tests/common/mod.rs`:

```rust
impl Hive {
    /// `cargo build --offline` in `dir` with the scratch CARGO_HOME, into `dir/target`
    /// unless `target_dir` is given.
    pub fn build_crate(&self, dir: &Path, target_dir: Option<&Path>) {
        let mut cmd = Command::new("cargo");
        cmd.args(["build", "--offline", "--quiet"])
            .current_dir(dir)
            .env("CARGO_HOME", &self.cargo_home.path)
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_TARGET_DIR")
            .env_remove("CARGO_BUILD_BUILD_DIR");
        if let Some(t) = target_dir {
            cmd.env("CARGO_TARGET_DIR", t);
        }
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "cargo build failed: {}", String::from_utf8_lossy(&out.stderr));
    }
}
```

`tests/status_targets.rs`:

```rust
mod common;
use common::{init_repo, Hive, Scratch};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const TAG: &str = "# This file is a cache directory tag created by cargo.";

fn register(hive: &Hive, repo: &Path) {
    let out = hive.run(&["repo", "add", repo.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

fn status(hive: &Hive, args: &[&str]) -> serde_json::Value {
    let mut all = vec!["status"];
    all.extend_from_slice(args);
    let (out, receipt) = hive.json(&all);
    assert!(out.status.success(), "{receipt}\n{}", String::from_utf8_lossy(&out.stderr));
    receipt
}

/// The ownership finding (`target.owned`, `target.orphan_candidate`, `target.unresolved`) for `path`.
fn target_kind(receipt: &serde_json::Value, path: &Path) -> Option<(String, String)> {
    receipt["findings"].as_array().unwrap().iter().find(|f| {
        let kind = f["kind"].as_str().unwrap();
        f["path"] == serde_json::json!(path) && kind.starts_with("target.") && kind != "target.unreadable"
    }).map(|f| (f["kind"].as_str().unwrap().to_string(), f["detail"].as_str().unwrap().to_string()))
}

fn finding(receipt: &serde_json::Value, kind: &str, path: &Path) -> Option<String> {
    receipt["findings"].as_array().unwrap().iter().find(|f| f["kind"] == kind && f["path"] == serde_json::json!(path)).map(|f| f["detail"].as_str().unwrap().to_string())
}

fn count(receipt: &serde_json::Value, key: &str) -> Option<u64> {
    receipt["counts"][key].as_u64()
}

fn add_bin(repo: &Path) {
    fs::write(repo.join("src/main.rs"), "fn main() {\n    println!(\"{}\", fixture::answer());\n}\n").unwrap();
}

/// A dir Cargo would recognize: its tag line plus a profile with an (empty) `.fingerprint/`.
fn fake_target(dir: &Path) {
    fs::create_dir_all(dir.join("debug/.fingerprint")).unwrap();
    fs::write(dir.join("CACHEDIR.TAG"), format!("Signature: 8a477f597d28d172789f06886806bc55\n{TAG}\n")).unwrap();
}

#[test]
fn a_built_checkout_reports_its_target_as_owned_with_units_and_resolved_edges() {
    let hive = Hive::ready();
    let s = Scratch::new("owned");
    let repo = init_repo(&s.path.join("app"), "");
    add_bin(&repo);
    hive.build_crate(&repo, None);
    register(&hive, &repo);
    let r = status(&hive, &[repo.to_str().unwrap()]);
    let target = repo.join("target");
    let (kind, detail) = target_kind(&r, &target).expect("the target is listed");
    assert_eq!(kind, "target.owned");
    assert!(detail.contains(repo.to_str().unwrap()), "{detail}");
    let profile = target.join("debug").display().to_string();
    // lib + bin: two units, and the bin's dependency on the lib is the one edge.
    assert_eq!(count(&r, &format!("units:{profile}")), Some(2));
    assert_eq!(count(&r, &format!("edges_resolved:{profile}")), Some(1));
    assert_eq!(count(&r, &format!("edges_unresolved:{profile}")), Some(0));
    assert_eq!(count(&r, &format!("edges_ambiguous:{profile}")), Some(0));
}

#[test]
fn a_cache_root_target_no_checkout_resolves_to_is_an_orphan_candidate_only_when_every_repo_is_in_scope() {
    let hive = Hive::ready();
    let s = Scratch::new("orphan");
    let cache = s.path.join("cache");
    hive.config(&format!("{{\"cache_roots\": [\"{}\"]}}", cache.display()));
    let a = init_repo(&s.path.join("a"), "");
    let b = init_repo(&s.path.join("b"), "");
    hive.build_crate(&a, Some(&cache.join("old-a")));
    register(&hive, &a);
    register(&hive, &b);
    let everything = status(&hive, &[]);
    assert_eq!(target_kind(&everything, &cache.join("old-a")).unwrap().0, "target.orphan_candidate");
    let scoped = status(&hive, &[a.to_str().unwrap()]);
    let (kind, detail) = target_kind(&scoped, &cache.join("old-a")).unwrap();
    assert_eq!(kind, "target.unresolved");
    assert!(detail.contains("without arguments"), "{detail}");
}

#[test]
fn a_build_dir_outside_every_search_root_is_owned_and_its_artifact_dir_is_not_a_target() {
    let hive = Hive::ready();
    let s = Scratch::new("builddir");
    let repo = init_repo(&s.path.join("app"), "");
    let bd = s.path.join("elsewhere/bd");
    fs::create_dir_all(repo.join(".cargo")).unwrap();
    fs::write(repo.join(".cargo/config.toml"), format!("[build]\nbuild-dir = \"{}\"\n", bd.display())).unwrap();
    hive.build_crate(&repo, None);
    register(&hive, &repo);
    let r = status(&hive, &[repo.to_str().unwrap()]);
    assert_eq!(target_kind(&r, &bd).unwrap().0, "target.owned");
    assert_eq!(target_kind(&r, &repo.join("target")), None);
}

#[test]
fn a_profile_in_the_newer_layout_is_unsupported_has_no_edges_and_is_recorded() {
    let hive = Hive::ready();
    let s = Scratch::new("newlayout");
    let repo = init_repo(&s.path.join("app"), "");
    let t = repo.join("weird");
    fs::create_dir_all(t.join("debug/build/fixture/0123456789abcdef/fingerprint")).unwrap();
    fs::write(t.join("CACHEDIR.TAG"), format!("{TAG}\n")).unwrap();
    register(&hive, &repo);
    let r = status(&hive, &[repo.to_str().unwrap()]);
    let profile = t.join("debug");
    assert!(finding(&r, "profile.unsupported", &profile).unwrap().contains(".fingerprint"));
    assert_eq!(count(&r, &format!("edges_resolved:{}", profile.display())), None);
    let snapshot: serde_json::Value = serde_json::from_slice(&fs::read(hive.home.path.join("snapshot.json")).unwrap()).unwrap();
    assert!(snapshot["unsupported_profiles"][profile.to_str().unwrap()].is_string(), "{snapshot}");
}

#[test]
fn a_corrupted_fingerprint_makes_its_profile_unsupported() {
    let hive = Hive::ready();
    let s = Scratch::new("corrupt");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    let fp = repo.join("target/debug/.fingerprint");
    let unit = fs::read_dir(&fp).unwrap().map(|e| e.unwrap().path()).find(|p| p.file_name().unwrap().to_string_lossy().starts_with("fixture-")).unwrap();
    let json = fs::read_dir(&unit).unwrap().map(|e| e.unwrap().path()).find(|p| p.extension().is_some_and(|x| x == "json")).unwrap();
    let text = fs::read_to_string(&json).unwrap().replacen('{', "{\"version\":1,", 1);
    fs::write(&json, text).unwrap();
    register(&hive, &repo);
    let r = status(&hive, &[repo.to_str().unwrap()]);
    assert!(finding(&r, "profile.unsupported", &repo.join("target/debug")).is_some(), "{r}");
}

#[test]
fn an_unreadable_target_is_reported_and_the_others_are_still_listed() {
    let hive = Hive::ready();
    let s = Scratch::new("unreadable");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    fake_target(&repo.join("extra"));
    fs::set_permissions(repo.join("target/debug"), fs::Permissions::from_mode(0o000)).unwrap();
    register(&hive, &repo);
    let r = status(&hive, &[repo.to_str().unwrap()]);
    fs::set_permissions(repo.join("target/debug"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(finding(&r, "target.unreadable", &repo.join("target")).unwrap().contains("ermission denied"), "{r}");
    assert!(target_kind(&r, &repo.join("extra")).is_some());
}

#[test]
fn target_search_is_bounded_and_never_enters_node_modules_or_another_target() {
    let hive = Hive::ready();
    let s = Scratch::new("bounds");
    let cache = s.path.join("cache");
    hive.config(&format!("{{\"cache_roots\": [\"{}\"]}}", cache.display()));
    let repo = init_repo(&s.path.join("app"), "");
    let present: Vec<PathBuf> = vec![repo.join("a/b/t3"), repo.join("outer"), cache.join("x/t2")];
    let absent: Vec<PathBuf> = vec![repo.join("a/b/c/t4"), repo.join("node_modules/t"), repo.join("outer/inner"), cache.join("x/y/t3")];
    for dir in present.iter().chain(&absent) {
        fake_target(dir);
    }
    register(&hive, &repo);
    let r = status(&hive, &[]);
    for p in &present {
        assert!(target_kind(&r, p).is_some(), "{} should be listed", p.display());
    }
    for p in &absent {
        assert_eq!(target_kind(&r, p), None, "{} must not be listed", p.display());
    }
}

#[test]
fn the_callers_target_dir_variable_is_ignored_and_noted() {
    let hive = Hive::ready();
    let s = Scratch::new("callerenv");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    register(&hive, &repo);
    let out = hive.command(&["status", repo.to_str().unwrap(), "--json"]).env("CARGO_TARGET_DIR", s.path.join("elsewhere")).output().unwrap();
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(target_kind(&r, &repo.join("target")).unwrap().0, "target.owned");
    assert!(r["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("CARGO_TARGET_DIR")), "{r}");
}

#[test]
fn metadata_results_are_cached_until_a_stamped_file_changes() {
    let hive = Hive::ready();
    let s = Scratch::new("cache-stamp");
    let repo = init_repo(&s.path.join("app"), "");
    register(&hive, &repo);
    let first = status(&hive, &[repo.to_str().unwrap()]);
    assert_eq!((count(&first, "metadata_calls"), count(&first, "metadata_cache_hits")), (Some(1), Some(0)));
    let second = status(&hive, &[repo.to_str().unwrap()]);
    assert_eq!((count(&second, "metadata_calls"), count(&second, "metadata_cache_hits")), (Some(0), Some(1)));
    let manifest = repo.join("Cargo.toml");
    let later = fs::metadata(&manifest).unwrap().modified().unwrap() + std::time::Duration::from_secs(5);
    fs::File::options().write(true).open(&manifest).unwrap().set_modified(later).unwrap();
    let third = status(&hive, &[repo.to_str().unwrap()]);
    assert_eq!(count(&third, "metadata_calls"), Some(1));
}
```

Run: `cargo test --lib -- cargo_target state` and `cargo test --test status_targets`. Expect compile errors and failures: `cargo_target` does not exist, and status reports no targets.

- [x] **Step 2: Implement `src/cargo_target.rs`** (above its tests)

```rust
//! Cargo target dirs: identification by tag and fingerprint layout, and
//! ownership through `cargo metadata` with the caller's target overrides scrubbed (Decision 4).

use crate::fingerprint::ProfileReport;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const TAG_LINE: &str = "# This file is a cache directory tag created by cargo.";
pub const TARGET_VARS: [&str; 3] = ["CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    Fingerprint,
    NewBuildDir,
}

fn is_dir(path: &Path) -> Result<bool, String> {
    match fs::metadata(path) {
        Ok(m) => Ok(m.is_dir()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn is_hex16(name: &str) -> bool {
    name.len() == 16 && name.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn sorted_dirs(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut dirs: Vec<PathBuf> = entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect();
    dirs.sort();
    Ok(dirs)
}

/// `Some(layout)` when `dir` holds `.fingerprint/` or `build/<pkg>/<16 hex>/fingerprint/`.
fn profile_layout(dir: &Path) -> Result<Option<Layout>, String> {
    if is_dir(&dir.join(".fingerprint"))? {
        return Ok(Some(Layout::Fingerprint));
    }
    let build = dir.join("build");
    if !is_dir(&build)? {
        return Ok(None);
    }
    for pkg in sorted_dirs(&build)? {
        let Ok(hashes) = sorted_dirs(&pkg) else { continue };
        if hashes.iter().any(|h| h.file_name().and_then(|n| n.to_str()).is_some_and(is_hex16) && h.join("fingerprint").is_dir()) {
            return Ok(Some(Layout::NewBuildDir));
        }
    }
    Ok(None)
}

/// Profile dirs: direct children, or `<triple>/<profile>/` under `--target` builds.
fn profile_dirs(target: &Path) -> Result<Vec<(PathBuf, Layout)>, String> {
    let mut out = Vec::new();
    for child in sorted_dirs(target)? {
        if let Some(layout) = profile_layout(&child)? {
            out.push((child, layout));
            continue;
        }
        if let Ok(grandchildren) = sorted_dirs(&child) {
            for g in grandchildren {
                if let Some(layout) = profile_layout(&g)? {
                    out.push((g, layout));
                }
            }
        }
    }
    Ok(out)
}

/// `Some(Ok(profiles))` for a Cargo target dir; `Some(Err(reason))` for a dir
/// Cargo tagged whose profiles cannot be read; `None` for anything else.
pub fn identify(dir: &Path) -> Option<Result<Vec<(PathBuf, Layout)>, String>> {
    let tag = fs::read(dir.join("CACHEDIR.TAG")).ok()?;
    if !String::from_utf8_lossy(&tag).lines().any(|l| l.trim_end() == TAG_LINE) {
        return None;
    }
    match profile_dirs(dir) {
        Ok(profiles) if profiles.is_empty() => None,
        Ok(profiles) => Some(Ok(profiles)),
        Err(why) => Some(Err(why)),
    }
}

/// The realpath of `p`, or of its nearest existing ancestor with the missing tail appended.
pub fn realpath_lossy(p: &Path) -> PathBuf {
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cur = p;
    loop {
        if let Ok(real) = cur.canonicalize() {
            return tail.iter().rev().fold(real, |acc, name| acc.join(name));
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                cur = parent;
            }
            _ => return p.to_path_buf(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resolved {
    pub workspace_root: PathBuf,
    pub target_directory: PathBuf,
    pub build_directory: PathBuf,
}

/// Exactly `cargo metadata --no-deps --format-version 1 --locked --offline` in
/// `workspace_dir`, with the three target variables and every `GIT_*` removed (F9).
pub fn resolve(workspace_dir: &Path, cancel: impl FnMut() -> bool) -> Result<Resolved, String> {
    let mut cmd = Command::new("cargo");
    cmd.args(["metadata", "--no-deps", "--format-version", "1", "--locked", "--offline"])
        .current_dir(workspace_dir)
        .env("RUSTUP_AUTO_INSTALL", "0")
        .stdin(Stdio::null());
    for var in TARGET_VARS {
        cmd.env_remove(var);
    }
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(&key);
        }
    }
    let out = cmd.output().map_err(|e| format!("could not run cargo metadata: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("cargo metadata failed: {}", stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output").trim()));
    }
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata printed invalid JSON: {e}"))?;
    let path = |key: &str| value.get(key).and_then(serde_json::Value::as_str).map(PathBuf::from);
    let target_directory = path("target_directory").ok_or("cargo metadata printed no target_directory")?;
    let build_directory = path("build_directory").unwrap_or_else(|| target_directory.clone());
    let workspace_root = path("workspace_root").ok_or("cargo metadata printed no workspace_root")?;
    Ok(Resolved {
        workspace_root: realpath_lossy(&workspace_root),
        target_directory: realpath_lossy(&target_directory),
        build_directory: realpath_lossy(&build_directory),
    })
}

/// The target variables set in hive's own environment, with their realpaths.
pub fn caller_overrides() -> Vec<(String, PathBuf)> {
    TARGET_VARS
        .iter()
        .filter_map(|var| std::env::var_os(var).filter(|v| !v.is_empty()).map(|v| (var.to_string(), realpath_lossy(Path::new(&v)))))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Owner {
    Owned { checkout: PathBuf },
    Unresolved { reason: String },
    OrphanCandidate { owned_under_caller_env: Option<String> },
}

#[derive(Debug, Clone, Serialize)]
pub struct TargetDir {
    pub path: PathBuf,
    pub owner: Owner,
    pub profiles: Vec<ProfileReport>,
    pub size: Option<u64>,
    pub unreadable: Option<String>,
}
```

- [x] **Step 3: Add target discovery to `src/discover.rs`**

Change the imports to also bring in `crate::cargo_target::{self, Layout, Owner, Resolved, TargetDir}`, `crate::fingerprint::{self, Format}`, `serde::Deserialize`, `std::collections::BTreeMap` and `std::time::UNIX_EPOCH`. Then append:

```rust
pub const SNAPSHOT_FILE: &str = "snapshot.json";
const TARGET_SKIP: [&str; 2] = ["node_modules", ".git"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    /// `cargo metadata` results by workspace dir; valid while every stamped file keeps its mtime.
    pub metadata: BTreeMap<PathBuf, CachedMetadata>,
    /// Profile dirs last seen with an unsupported fingerprint format, and why.
    pub unsupported_profiles: BTreeMap<PathBuf, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CachedMetadata {
    pub stamp: BTreeMap<PathBuf, Option<u64>>,
    pub resolved: Resolved,
}

fn mtime_nanos(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    u64::try_from(modified.duration_since(UNIX_EPOCH).ok()?.as_nanos()).ok()
}

/// Files whose change can change what `cargo metadata` resolves for `workspace_dir` (Decision 4).
fn stamp_files(workspace_dir: &Path, workspace_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in [workspace_dir, workspace_root] {
        files.push(dir.join("Cargo.toml"));
        files.push(dir.join("Cargo.lock"));
    }
    for dir in workspace_dir.ancestors() {
        for name in ["rust-toolchain", "rust-toolchain.toml", ".cargo/config", ".cargo/config.toml"] {
            files.push(dir.join(name));
        }
    }
    let cargo_home = std::env::var_os("CARGO_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")));
    if let Some(home) = cargo_home {
        files.push(home.join("config"));
        files.push(home.join("config.toml"));
    }
    files.sort();
    files.dedup();
    files
}

fn cached(snapshot: &Snapshot, dir: &Path) -> Option<Resolved> {
    let entry = snapshot.metadata.get(dir)?;
    entry.stamp.iter().all(|(file, t)| mtime_nanos(file) == *t).then(|| entry.resolved.clone())
}

/// Dirs at depth 0–2 of a checkout that hold a `Cargo.toml`, with their depth.
pub fn workspace_dirs(checkout: &Path) -> Vec<(usize, PathBuf)> {
    let mut out = Vec::new();
    let mut level = vec![checkout.to_path_buf()];
    for depth in 0..=2 {
        let mut next = Vec::new();
        for dir in &level {
            if dir.join("Cargo.toml").is_file() {
                out.push((depth, dir.clone()));
            }
            if depth == 2 {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            let mut kids: Vec<PathBuf> = entries.flatten().filter(|e| !skipped(&e.file_name()) && e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect();
            kids.sort();
            next.extend(kids);
        }
        level = next;
    }
    out
}

type Found = (PathBuf, Result<Vec<(PathBuf, Layout)>, String>);

/// Cargo target dirs at most `depth` levels below `root`. A found target is not entered,
/// and neither is `node_modules` or `.git`.
pub fn find_targets(root: &Path, depth: usize, out: &mut Vec<Found>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else { return };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_name().to_str().is_none_or(|n| !TARGET_SKIP.contains(&n)) && e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    for dir in dirs {
        match cargo_target::identify(&dir) {
            Some(found) => out.push((dir, found)),
            None => find_targets(&dir, depth - 1, out),
        }
    }
}

struct Resolution {
    dir: PathBuf,
    result: Result<Resolved, String>,
    cached: bool,
}

/// Shallowest dirs first; a dir inside an already resolved workspace root is not queried.
/// Uncached calls run up to 4 at a time.
fn resolve_workspaces(checkouts: &[PathBuf], snapshot: &mut Snapshot, faults: &mut Vec<WalkFault>, cancel: &(impl Fn() -> bool + Sync)) -> Vec<Resolution> {
    let mut by_depth: BTreeMap<usize, BTreeSet<PathBuf>> = BTreeMap::new();
    for checkout in checkouts {
        for (depth, dir) in workspace_dirs(checkout) {
            by_depth.entry(depth).or_default().insert(dir);
        }
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut out = Vec::new();
    for dirs in by_depth.into_values() {
        let mut uncached = Vec::new();
        for dir in dirs {
            if roots.iter().any(|r| dir.starts_with(r)) {
                continue;
            }
            match cached(snapshot, &dir) {
                Some(r) => {
                    roots.push(r.workspace_root.clone());
                    out.push(Resolution { dir, result: Ok(r), cached: true });
                }
                None => uncached.push(dir),
            }
        }
        for chunk in uncached.chunks(4) {
            let results: Vec<Result<Resolved, String>> = std::thread::scope(|s| {
                let handles: Vec<_> = chunk.iter().map(|d| s.spawn(move || cargo_target::resolve(d))).collect();
                handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("the cargo metadata resolver panicked".into()))).collect()
            });
            for (dir, result) in chunk.iter().cloned().zip(results) {
                match &result {
                    Ok(r) => {
                        roots.push(r.workspace_root.clone());
                        let stamp = stamp_files(&dir, &r.workspace_root).into_iter().map(|f| (f.clone(), mtime_nanos(&f))).collect();
                        snapshot.metadata.insert(dir.clone(), CachedMetadata { stamp, resolved: r.clone() });
                    }
                    Err(_) => {
                        snapshot.metadata.remove(&dir);
                    }
                }
                out.push(Resolution { dir, result, cached: false });
            }
        }
    }
    out
}

/// The known checkout that most deeply contains `workspace_root`.
fn owner_checkout(workspace_root: &Path, checkouts: &[PathBuf]) -> PathBuf {
    checkouts.iter().filter(|c| workspace_root.starts_with(c)).max_by_key(|c| c.components().count()).cloned().unwrap_or_else(|| workspace_root.to_path_buf())
}

pub struct TargetScan {
    pub targets: Vec<TargetDir>,
    pub metadata_calls: u64,
    pub metadata_cache_hits: u64,
    pub ignored_vars: Vec<String>,
    pub cleared_profiles: BTreeSet<PathBuf>,
}

/// Every Cargo target dir of the live `checkouts` (3 levels) and the cache roots (2 levels),
/// plus every resolved build directory wherever it lies, with owner and fingerprint reports.
pub fn targets(checkouts: &[Checkout], cfg: &Config, all_repos: bool, snapshot: &mut Snapshot, cancel: impl Fn() -> bool + Sync) -> TargetScan {
    let live: Vec<PathBuf> = checkouts.iter().filter(|c| !c.missing).map(|c| c.path.clone()).collect();
    let mut walk_faults = Vec::new();
    let resolutions = resolve_workspaces(&live, snapshot, &mut walk_faults, &cancel);
    let resolved: Vec<&Resolved> = resolutions.iter().filter_map(|r| r.result.as_ref().ok()).collect();
    let failure = resolutions.iter().find_map(|r| r.result.as_ref().err().map(|e| format!("ownership unknown: cargo metadata failed in {}: {e}", r.dir.display())));
    let overrides = cargo_target::caller_overrides();

    let mut found: Vec<(Found, bool)> = Vec::new();
    for checkout in &live {
        let mut v = Vec::new();
        find_targets(checkout, 3, &mut v);
        found.extend(v.into_iter().map(|f| (f, false)));
    }
    for root in &cfg.cache_roots {
        let mut v = Vec::new();
        find_targets(root, 2, &mut v);
        found.extend(v.into_iter().map(|f| (f, true)));
    }
    for r in &resolved {
        if let Some(identified) = cargo_target::identify(&r.build_directory) {
            found.push(((r.build_directory.clone(), identified), false));
        }
    }

    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();
    for ((path, identified), from_cache_root) in found {
        let real = path.canonicalize().unwrap_or(path);
        if seen.insert(real.clone()) {
            entries.push((real, identified, from_cache_root));
        }
    }
    // Profiles are read one scoped thread per target: klynt's checkouts hold thousands of
    // units, and the 2 s status budget includes reading them.
    let reports: Vec<Vec<fingerprint::ProfileReport>> = std::thread::scope(|s| {
        let handles: Vec<_> = entries
            .iter()
            .map(|(_, identified, _)| {
                s.spawn(move || match identified {
                    Ok(dirs) => dirs.iter().map(|(p, _)| fingerprint::read_profile(p)).collect(),
                    Err(_) => Vec::new(),
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
    });
    let mut targets = Vec::new();
    for ((real, identified, from_cache_root), profiles) in entries.into_iter().zip(reports) {
        let owner = match resolved.iter().find(|r| r.target_directory == real || r.build_directory == real) {
            Some(r) => Owner::Owned { checkout: owner_checkout(&r.workspace_root, &live) },
            None => match (&failure, from_cache_root && !all_repos) {
                (Some(why), _) => Owner::Unresolved { reason: why.clone() },
                (None, true) => Owner::Unresolved { reason: "a cache-root target is judged against every registered repository; run `cargo hive status` without arguments".into() },
                (None, false) => Owner::OrphanCandidate { owned_under_caller_env: overrides.iter().find(|(_, p)| *p == real).map(|(name, _)| name.clone()) },
            },
        };
        for p in &profiles {
            match &p.format {
                Format::Unsupported(why) => {
                    snapshot.unsupported_profiles.insert(p.path.clone(), why.clone());
                }
                Format::Supported => {
                    snapshot.unsupported_profiles.remove(&p.path);
                }
            }
        }
        targets.push(TargetDir { path: real, owner, profiles, size: None, unreadable: identified.err() });
    }
    TargetScan {
        targets,
        metadata_calls: resolutions.iter().filter(|r| !r.cached).count() as u64,
        metadata_cache_hits: resolutions.iter().filter(|r| r.cached).count() as u64,
        ignored_vars: overrides.into_iter().map(|(name, _)| name).collect(),
        cleared_profiles: BTreeSet::new(),
    }
}
```

- [x] **Step 4: Replace `status` in `src/cli.rs`**, and add `report_targets`

Add `ReceiptBuilder` to the `crate::receipt` import. Then:

```rust
fn status(state: &State, args: &[std::path::PathBuf], out: OutputArgs) -> i32 {
    use crate::classify::Class;
    use crate::discover::{self, Snapshot, SNAPSHOT_FILE};
    use crate::doctor::{self, Capabilities, Capability};
    let loaded = (|| -> Result<_, crate::state::StateError> {
        Ok((state.read::<Registry>(registry::FILE)?, state.config()?, state.read::<Capabilities>(doctor::CAPABILITIES_FILE)?, state.read::<Snapshot>(SNAPSHOT_FILE)?))
    })();
    let (reg, cfg, caps, mut snapshot) = match loaded {
        Ok(v) => v,
        Err(e) => {
            eprintln!("cargo-hive: {e}");
            return 1;
        }
    };
    let mut b = Receipt::new("status", Scope::default());
    let list = |v: &[std::path::PathBuf]| v.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ");
    b.note(format!("secondary_roots: {}", list(&cfg.secondary_roots)));
    b.note(format!("cache_roots: {}", list(&cfg.cache_roots)));
    let release = doctor::resolve_toolchain(state.root()).ok().map(|t| t.release);
    b.toolchain(release.clone());
    if let Err(refusal) = doctor::require(&caps, Capability::FingerprintReading, release.as_deref()) {
        b.finding("refusal", None, refusal.to_string());
        return emit(state, out, b.finish(), &format!("refused: {refusal}\n"), 1);
    }
    let mut code = 0;
    let mut selected = Vec::new();
    if args.is_empty() {
        selected = reg.repos.clone();
    }
    for arg in args {
        let real = arg.canonicalize().unwrap_or(arg.clone());
        match reg.repos.iter().find(|r| r.main == real) {
            Some(r) => selected.push(r.clone()),
            None => {
                b.finding("refusal", Some(arg.clone()), "not a registered repository; run `cargo hive repo add` first");
                code = 1;
            }
        }
    }
    let mains: Vec<_> = reg.repos.iter().map(|r| r.main.clone()).collect();
    let (mut total, mut protected, mut secondary, mut missing) = (0u64, 0u64, 0u64, 0u64);
    let mut human = String::new();
    let mut all_checkouts = Vec::new();
    for repo in &selected {
        b.scope_mut().repos.push(repo.main.clone());
        human.push_str(&format!("{}\n", repo.main.display()));
        for c in discover::checkouts(repo, &mains, &cfg) {
            total += 1;
            let label = if c.missing {
                missing += 1;
                "missing"
            } else if c.class == Class::Secondary {
                secondary += 1;
                "secondary"
            } else {
                protected += 1;
                "protected"
            };
            human.push_str(&format!("  {label:<9} {}  ({})\n", c.path.display(), c.reason));
            b.finding(&format!("checkout.{label}"), Some(c.path.clone()), c.reason.clone());
            b.scope_mut().checkouts.push(c.path.clone());
            all_checkouts.push(c);
        }
    }
    b.count("checkouts", total).count("protected", protected).count("secondary", secondary).count("missing", missing);
    let all_repos = selected.len() == reg.repos.len();
    let cancelled = || doctor::CANCEL.load(std::sync::atomic::Ordering::SeqCst);
    let scan = discover::targets(&all_checkouts, &cfg, all_repos, &mut snapshot, cancelled);
    report_targets(&mut b, &mut human, &scan);
    if let Err(e) = state.locked(|tx| tx.write(SNAPSHOT_FILE, &snapshot)) {
        eprintln!("cargo-hive: {e}");
        return 1;
    }
    emit(state, out, b.finish(), &human, code)
}

fn report_targets(b: &mut ReceiptBuilder, human: &mut String, scan: &crate::discover::TargetScan) {
    use crate::cargo_target::Owner;
    use crate::fingerprint::Format;
    let (mut owned, mut orphans, mut unresolved, mut unreadable, mut unsupported) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut units, mut resolved, mut dangling, mut ambiguous) = (0u64, 0u64, 0u64, 0u64);
    human.push_str("targets\n");
    for t in &scan.targets {
        let (kind, detail) = match &t.owner {
            Owner::Owned { checkout } => {
                owned += 1;
                ("target.owned", format!("owned by {}", checkout.display()))
            }
            Owner::OrphanCandidate { owned_under_caller_env } => {
                orphans += 1;
                let base = "no known checkout resolves to this dir".to_string();
                ("target.orphan_candidate", match owned_under_caller_env {
                    Some(var) => format!("{base}; it would be owned under the caller's {var}"),
                    None => base,
                })
            }
            Owner::Unresolved { reason } => {
                unresolved += 1;
                ("target.unresolved", reason.clone())
            }
        };
        let size = t.size.map(|n| format!("  {n} bytes")).unwrap_or_default();
        human.push_str(&format!("  {}{size}  {detail}\n", t.path.display()));
        b.finding(kind, Some(t.path.clone()), detail);
        if let Some(why) = &t.unreadable {
            unreadable += 1;
            human.push_str(&format!("    unreadable: {why}\n"));
            b.finding("target.unreadable", Some(t.path.clone()), why.clone());
        }
        for p in &t.profiles {
            let key = p.path.display().to_string();
            b.count(&format!("units:{key}"), p.units as u64);
            units += p.units as u64;
            match (&p.format, &p.edges) {
                (Format::Supported, Some(e)) => {
                    b.count(&format!("edges_resolved:{key}"), e.resolved).count(&format!("edges_unresolved:{key}"), e.unresolved).count(&format!("edges_ambiguous:{key}"), e.ambiguous);
                    resolved += e.resolved;
                    dangling += e.unresolved;
                    ambiguous += e.ambiguous;
                    human.push_str(&format!("    {key}  {} units, edges {} resolved / {} unresolved / {} ambiguous\n", p.units, e.resolved, e.unresolved, e.ambiguous));
                }
                (Format::Unsupported(why), _) => {
                    unsupported += 1;
                    human.push_str(&format!("    {key}  unsupported: {why}\n"));
                    b.finding("profile.unsupported", Some(p.path.clone()), why.clone());
                }
                (Format::Supported, None) => {}
            }
        }
    }
    b.count("targets", scan.targets.len() as u64)
        .count("owned", owned)
        .count("orphan_candidates", orphans)
        .count("unresolved_owners", unresolved)
        .count("unreadable", unreadable)
        .count("unsupported_profiles", unsupported)
        .count("units", units)
        .count("edges_resolved", resolved)
        .count("edges_unresolved", dangling)
        .count("edges_ambiguous", ambiguous)
        .count("metadata_calls", scan.metadata_calls)
        .count("metadata_cache_hits", scan.metadata_cache_hits);
    for var in &scan.ignored_vars {
        b.note(format!("{var} is set in the caller's environment and was ignored: ownership comes from each checkout's own Cargo configuration"));
    }
}
```

`src/lib.rs` gains `pub mod cargo_target;`.

Run: `cargo test --lib -- cargo_target state` and `cargo test --test status_targets` — expect: all pass (2 + 6 unit tests, 9 integration tests). Then run `cargo test` and `cargo clippy --all-targets -- -D warnings`.

- [x] **Step 5: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(status): Cargo target dirs with owners, orphan candidates and fingerprint reports"`

_Requirements: CORE-4.2, CORE-4.3, CORE-4.5, CORE-4.7, CORE-4.8, CORE-4.10, CORE-4.12, CORE-4.13, CORE-4.14, CORE-4.15, CORE-6.2_

---

### Task 15: Allocated size per target, and `--no-sizes`

**Files:**
- Create: `src/size.rs`
- Modify: `src/cli.rs` (`status --no-sizes`; fill `TargetDir.size` and the `bytes:<path>` counts)
- Modify: `src/lib.rs` (add `pub mod size;`)
- Test: `src/size.rs` (`#[cfg(test)]`), `tests/status_targets.rs`

**Reuse:** 3 — `std::os::unix::fs::MetadataExt` (`blocks`, `ino`, `dev`)

**Interfaces:**
- Consumes: `discover::TargetScan`, `cargo_target::TargetDir` (Task 14)
- Produces:
  - `size::{MAX_DEPTH, SizeError::{DepthLimit(PathBuf), Io(PathBuf, io::Error)}, allocated_bytes(&Path, max_depth: usize) -> Result<u64, SizeError>}`
  - Receipt counts `bytes` (total) and `bytes:<target>`
  - `unmeasured` entries `sizes` (reason mentions `--no-sizes`) and `bytes:<target>` (unreadable target or size error)

**Depends-on:** Task 14

- [x] **Step 1: Write the failing tests**

At the bottom of the new `src/size.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(label: &str) -> PathBuf {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("hive-size-{label}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn each_inode_is_counted_once_and_symlinks_are_not_followed() {
        let root = scratch("inodes");
        let tree = root.join("tree");
        fs::create_dir_all(tree.join("sub")).unwrap();
        fs::write(tree.join("f"), vec![1u8; 100_000]).unwrap();
        fs::hard_link(tree.join("f"), tree.join("sub/g")).unwrap();
        fs::write(tree.join("sub/h"), vec![2u8; 10_000]).unwrap();
        let outside = root.join("big");
        fs::write(&outside, vec![3u8; 1 << 20]).unwrap();
        std::os::unix::fs::symlink(&outside, tree.join("link")).unwrap();
        let blocks = |rel: &str| fs::metadata(tree.join(rel)).unwrap().blocks() * 512;
        assert_eq!(allocated_bytes(&tree, MAX_DEPTH).unwrap(), blocks("f") + blocks("sub/h"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_depth_limit_is_explicit() {
        let root = scratch("depth");
        fs::create_dir_all(root.join("a/b/c")).unwrap();
        assert!(matches!(allocated_bytes(&root, 2), Err(SizeError::DepthLimit(p)) if p.ends_with("a/b/c")));
        assert!(allocated_bytes(&root, 3).is_ok());
        fs::remove_dir_all(root).unwrap();
    }
}
```

Add to `tests/status_targets.rs`:

```rust
#[test]
fn each_target_reports_its_allocated_bytes_unless_sizes_are_skipped() {
    let hive = Hive::ready();
    let s = Scratch::new("sizes");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    register(&hive, &repo);
    let target = repo.join("target");
    let r = status(&hive, &[repo.to_str().unwrap()]);
    let expected = cargo_hive::size::allocated_bytes(&target, cargo_hive::size::MAX_DEPTH).unwrap();
    assert!(expected > 0);
    assert_eq!(count(&r, &format!("bytes:{}", target.display())), Some(expected));
    let skipped = status(&hive, &[repo.to_str().unwrap(), "--no-sizes"]);
    assert!(skipped["counts"].as_object().unwrap().keys().all(|k| !k.starts_with("bytes")), "{skipped}");
    assert!(skipped["unmeasured"].as_array().unwrap().iter().any(|u| u["field"] == "sizes" && u["reason"].as_str().unwrap().contains("--no-sizes")));
}
```

In `an_unreadable_target_is_reported_and_the_others_are_still_listed`, add before the final assertion:

```rust
    let field = format!("bytes:{}", repo.join("target").display());
    assert!(r["unmeasured"].as_array().unwrap().iter().any(|u| u["field"] == serde_json::json!(field)), "{r}");
    assert!(r["counts"].get(&field).is_none(), "an unmeasured size is omitted, not reported as a number");
```

Run: `cargo test --lib size` and `cargo test --test status_targets` — expect: compile errors (`size` missing) and failures (`--no-sizes` unknown).

- [x] **Step 2: Implement `src/size.rs`** (above its tests)

```rust
//! Allocated bytes of a tree: Σ st_blocks × 512 over regular files, each (dev, inode)
//! counted once, never following symlinks, with an explicit depth limit.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Deeper than any Cargo target layout (`OUT_DIR` trees included); hitting it is reported, not skipped.
pub const MAX_DEPTH: usize = 64;

#[derive(Debug)]
pub enum SizeError {
    DepthLimit(PathBuf),
    Io(PathBuf, io::Error),
}

impl fmt::Display for SizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SizeError::DepthLimit(p) => write!(f, "{} is deeper than the size walk's limit", p.display()),
            SizeError::Io(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

pub fn allocated_bytes(root: &Path, max_depth: usize) -> Result<u64, SizeError> {
    let mut seen = HashSet::new();
    let mut total = 0u64;
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| SizeError::Io(dir.clone(), e))?;
        for entry in entries {
            let path = entry.map_err(|e| SizeError::Io(dir.clone(), e))?.path();
            let meta = fs::symlink_metadata(&path).map_err(|e| SizeError::Io(path.clone(), e))?;
            if meta.is_dir() {
                if depth + 1 > max_depth {
                    return Err(SizeError::DepthLimit(path));
                }
                stack.push((path, depth + 1));
            } else if meta.is_file() && seen.insert((meta.dev(), meta.ino())) {
                total += meta.blocks() * 512;
            }
        }
    }
    Ok(total)
}
```

- [x] **Step 3: Wire sizes into `status`**

In `src/cli.rs`, add to the `Status` variant:

```rust
        /// Skip size accounting (sizes are then reported as not measured)
        #[arg(long)]
        no_sizes: bool,
```

Change the dispatch to `Command::Status { repos, no_sizes, out } => status(&state, &repos, no_sizes, out),` and the signature to `fn status(state: &State, args: &[std::path::PathBuf], no_sizes: bool, out: OutputArgs) -> i32`. Make `scan` mutable, and insert between `discover::targets(…)` and `report_targets(…)`:

```rust
    if no_sizes {
        b.unmeasured("sizes", "--no-sizes was given");
    } else {
        let mut bytes = 0u64;
        for t in &mut scan.targets {
            let key = format!("bytes:{}", t.path.display());
            if let Some(why) = &t.unreadable {
                b.unmeasured(&key, format!("the target dir is unreadable: {why}"));
                continue;
            }
            match crate::size::allocated_bytes(&t.path, crate::size::MAX_DEPTH) {
                Ok(n) => {
                    t.size = Some(n);
                    bytes += n;
                    b.count(&key, n);
                }
                Err(e) => {
                    b.unmeasured(&key, e.to_string());
                }
            }
        }
        b.count("bytes", bytes);
    }
```

`src/lib.rs` gains `pub mod size;`.

Run: `cargo test --lib size` and `cargo test --test status_targets` — expect: 2 and 10 passed. Then run `cargo test` and `cargo clippy --all-targets -- -D warnings`.

- [x] **Step 4: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(status): inode-deduplicated allocated bytes per target, and --no-sizes"`

_Requirements: CORE-4.6, CORE-4.11, CORE-5.5_

---

### Task 16: Guards, crash safety, the performance check, and the README

**Files:**
- Create: `tests/guards.rs`
- Create: `README.md`

**Reuse:** existing — `tests/common::{Hive, Scratch, git, init_repo}` and `Hive::{ready, build_crate}`

**Interfaces:**
- Consumes: every command (Tasks 2–15)
- Produces: guard tests; the ignored CORE-6.1 timing test; `README.md`

**Depends-on:** Task 12, Task 15

- [x] **Step 1: Write the tests** — `tests/guards.rs`:

```rust
// The snapshot helpers serve only the macOS guard below.
#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
mod common;
use common::{git, init_repo, Hive, Scratch};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

type Entry = (&'static str, u64, SystemTime, u64);

/// Every entry under `root`: kind, length, mtime, and a hash of the content or link target.
fn tree(root: &Path) -> BTreeMap<PathBuf, Entry> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let mut h = DefaultHasher::new();
            let kind = if meta.is_dir() {
                stack.push(path.clone());
                "dir"
            } else if meta.is_symlink() {
                fs::read_link(&path).unwrap().hash(&mut h);
                "symlink"
            } else if meta.is_file() {
                fs::read(&path).unwrap().hash(&mut h);
                "file"
            } else {
                "special"
            };
            out.insert(path, (kind, meta.len(), meta.modified().unwrap(), h.finish()));
        }
    }
    out
}

fn changed(before: &BTreeMap<PathBuf, Entry>, after: &BTreeMap<PathBuf, Entry>) -> Vec<String> {
    let keys: BTreeSet<&PathBuf> = before.keys().chain(after.keys()).collect();
    keys.into_iter().filter(|k| before.get(*k) != after.get(*k)).map(|k| k.display().to_string()).collect()
}

fn cargo_home() -> PathBuf {
    std::env::var_os("CARGO_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".cargo"))
}

/// What hive could plausibly add to ~/.cargo: config, env files, or a wrapper in bin/.
/// Narrower than a full snapshot on purpose: other cargo processes on this machine may
/// legitimately touch the registry and git caches while the test runs.
fn cargo_home_marks() -> BTreeMap<String, Option<SystemTime>> {
    let home = cargo_home();
    let mut marks = BTreeMap::new();
    for name in ["config", "config.toml", "env", "env.fish"] {
        marks.insert(name.to_string(), fs::metadata(home.join(name)).and_then(|m| m.modified()).ok());
    }
    if let Ok(entries) = fs::read_dir(home.join("bin")) {
        for e in entries.flatten() {
            marks.insert(format!("bin/{}", e.file_name().to_string_lossy()), None);
        }
    }
    marks
}

#[cfg(target_os = "macos")]
#[test]
fn repo_add_status_and_doctor_leave_checkouts_targets_and_cargo_home_unchanged() {
    let hive = Hive::ready();
    let s = Scratch::new("guard");
    let built = init_repo(&s.path.join("built"), "");
    hive.build_crate(&built, None);
    git(&built, &["worktree", "add", "-q", built.join(".worktrees/lane").to_str().unwrap()]);
    let never_built = init_repo(&s.path.join("never-built"), "");
    let real_cargo_home = cargo_home();
    let (before, marks) = (tree(&s.path), cargo_home_marks());
    let runs: Vec<Vec<&str>> = vec![
        vec!["repo", "add", built.to_str().unwrap()],
        vec!["repo", "add", never_built.to_str().unwrap()],
        vec!["status"],
        vec!["status", "--no-sizes"],
        vec!["doctor"],
    ];
    for args in runs {
        let out = hive.command(&args).env("CARGO_HOME", &real_cargo_home).stdin(Stdio::null()).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    assert_eq!(changed(&before, &tree(&s.path)), Vec::<String>::new(), "hive changed files in checkouts or targets");
    assert_eq!(cargo_home_marks(), marks, "hive changed ~/.cargo");
}

#[test]
fn cargo_builds_a_registered_checkout_exactly_as_without_hive() {
    let hive = Hive::ready();
    let s = Scratch::new("as-before");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    assert!(hive.run(&["repo", "add", repo.to_str().unwrap()]).status.success());
    assert!(hive.run(&["status"]).status.success());
    let out = Command::new("cargo")
        .args(["build", "--offline"])
        .current_dir(&repo)
        .env("CARGO_HOME", &hive.cargo_home.path)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_BUILD_BUILD_DIR")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success() && !stderr.contains("Compiling"), "the build was not fresh after hive ran:\n{stderr}");
    assert!(!repo.join(".cargo").exists());
    let allowed: BTreeSet<&str> = [".lock", "capabilities.json", "config.json", "receipts", "registry.json", "snapshot.json"].into();
    for entry in fs::read_dir(&hive.home.path).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().to_string();
        assert!(allowed.contains(name.as_str()), "unexpected file in hive's state: {name}");
    }
}

#[test]
fn killing_hive_mid_write_never_leaves_a_partial_state_file() {
    let hive = Hive::ready();
    let s = Scratch::new("kill");
    let repos: Vec<String> = (0..6).map(|i| init_repo(&s.path.join(format!("r{i}")), "").display().to_string()).collect();
    for i in 0..40u64 {
        let repo = &repos[(i % 6) as usize];
        let args: Vec<&str> = if i % 3 == 0 { vec!["status", "--no-sizes", "--json"] } else { vec!["repo", "add", repo.as_str(), "--json"] };
        let mut child = hive.command(&args).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        std::thread::sleep(Duration::from_micros((i * 997) % 25_000));
        let _ = child.kill();
        let _ = child.wait();
    }
    for name in ["registry.json", "snapshot.json", "capabilities.json", "config.json"] {
        let path = hive.home.path.join(name);
        if path.exists() {
            serde_json::from_slice::<serde_json::Value>(&fs::read(&path).unwrap()).unwrap_or_else(|e| panic!("{name} is not complete JSON: {e}"));
        }
    }
    for entry in fs::read_dir(hive.home.path.join("receipts")).unwrap() {
        for line in fs::read_to_string(entry.unwrap().path()).unwrap().lines() {
            serde_json::from_str::<serde_json::Value>(line).unwrap_or_else(|e| panic!("a partial receipt line: {e}"));
        }
    }
    let out = hive.run(&["repo", "list"]);
    assert!(out.status.success(), "hive can no longer read its state: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn concurrent_registrations_are_all_kept() {
    let hive = Hive::ready();
    let s = Scratch::new("concurrent");
    let repos: Vec<PathBuf> = (0..8).map(|i| init_repo(&s.path.join(format!("r{i}")), "")).collect();
    let children: Vec<_> = repos.iter().map(|r| hive.command(&["repo", "add", r.to_str().unwrap()]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap()).collect();
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    let reg: serde_json::Value = serde_json::from_slice(&fs::read(hive.home.path.join("registry.json")).unwrap()).unwrap();
    assert_eq!(reg["repos"].as_array().unwrap().len(), 8);
}

#[test]
#[ignore = "manual performance check on the proof machine; needs ~/Developer/klynt/klynt"]
fn status_on_klynt_finishes_within_two_seconds_cold_and_warm() {
    let klynt = PathBuf::from(std::env::var_os("HOME").unwrap()).join("Developer/klynt/klynt");
    assert!(klynt.join("Cargo.toml").is_file(), "{} is not a checkout", klynt.display());
    let klynt = klynt.to_str().unwrap();
    let hive = Hive::ready();
    fs::remove_file(hive.home.path.join("config.json")).unwrap(); // the real defaults, as a user has them
    let real = cargo_home();
    assert!(hive.command(&["repo", "add", klynt]).env("CARGO_HOME", &real).status().unwrap().success());
    let run = || {
        let started = Instant::now();
        let out = hive.command(&["status", klynt, "--no-sizes", "--json"]).env("CARGO_HOME", &real).output().unwrap();
        let took = started.elapsed();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        took
    };
    let _ = fs::remove_file(hive.home.path.join("snapshot.json"));
    let cold = run();
    let mut warm: Vec<Duration> = (0..5).map(|_| run()).collect();
    warm.sort();
    eprintln!("cold {cold:?}; warm {warm:?}");
    assert!(cold < Duration::from_secs(2), "cold run took {cold:?}");
    assert!(warm[2] < Duration::from_secs(2), "warm median took {:?}", warm[2]);
}
```

Run: `cargo test --test guards`. Expect 4 passed and 1 ignored. These tests guard behavior earlier tasks built, so they should pass on first run. A failure is a real defect in an earlier task: stop and report it, and do not weaken the test. Then run `cargo test --test guards -- --ignored` on the proof machine and record the printed cold and warm timings in the task report.

- [x] **Step 2: Write `README.md`**

````markdown
# cargo-hive

Cargo build artifacts across agent worktrees on macOS, with measured receipts.

This release (foundation) reads only. It registers repositories, lists their checkouts
and Cargo target dirs, and says how hive may treat each one. It deletes, compresses
and seeds nothing yet.

## Install from scratch

Requirements: macOS, a Rust toolchain (rustup), and the Xcode Command Line Tools
(`xcode-select --install`), which also build the C parts of the embedded Applesauce
library.

```sh
cargo install --path .
cargo hive doctor          # checks what works on this machine; run it again after a toolchain change
cargo hive repo add ~/code/my-repo
cargo hive status          # every registered repo; add --json for one machine-readable receipt
```

## Commands

| Command | What it does |
|---|---|
| `cargo hive doctor` | Runs checks (a)–(f) in a temporary directory and records which capabilities are enabled for the active toolchain |
| `cargo hive repo add <path>` | Registers the main checkout containing `<path>` |
| `cargo hive repo list` | Lists registered repositories and their root commits |
| `cargo hive status [<repo>…] [--no-sizes]` | Lists checkouts (protected, secondary, missing) and Cargo target dirs (owner or orphan candidate, size, units and edges) |

Every command accepts `--json`. It then prints exactly one receipt on stdout, and every
receipt is also appended to `~/.hive/receipts/<date>.jsonl`. Receipts carry counts and
measured values only, never durations or estimated time saved.

## What hive never does

- It installs no Cargo configuration, environment variable, rustc wrapper, cargo shim or
  hook. `cargo` and rust-analyzer build exactly as without hive.
- It writes only under `~/.hive/` and inside doctor's own temporary directory.
- It reads Cargo's internal `.fingerprint/` files only when their exact format is known
  (docs/adr/0001). Anything else is reported as unsupported.

## Configuration

None is required. `~/.hive/config.json` may set `secondary_roots`, which defaults to
`/private/tmp` and `$TMPDIR`, and `cache_roots`, which defaults to
`~/.cache/cargo-target`. A key you set replaces its default list.

`HIVE_HOME` relocates the state directory. It exists for tests only.

## Uninstall

```sh
cargo uninstall cargo-hive && rm -rf ~/.hive
```
````

- [x] **Step 3: Check that the non-macOS build compiles**

Run `rustup target list --installed`. If `x86_64-unknown-linux-gnu` is listed, run `cargo check --target x86_64-unknown-linux-gnu`; expect success. If it is not listed, do not install it: that would write under `~/.rustup`. Record "non-macOS compile check not run: target not installed" in the task report.

- [x] **Step 4: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "test: guards for untouched checkouts and cargo, crash-safe state, and the README"`

_Requirements: CORE-6.1, CORE-6.3, CORE-6.5, CORE-7.1, CORE-7.2_

### Task 17: A toolchain selected by the caller's directory stays out of hive's children

Added after execution by `root-cause` (2026-10-28). The rustup proxy hands every cargo subcommand `RUSTUP_TOOLCHAIN`, which beats directory resolution, so doctor and the `status` gate resolved the caller's directory toolchain instead of the state root's. Landed as `451222a`.

**Files:**
- Modify: `src/doctor.rs` (`drop_directory_toolchain`, used by `rustup_which`)
- Modify: `src/cargo_target.rs` (`resolve`)
- Test: `tests/capability_gate.rs`
- Test: `tests/status_targets.rs`

**Reuse:** existing — `tests/common::{Hive, Scratch, init_repo}` and `Hive::{ready, command, build_crate}`

**Interfaces:**
- Consumes: `doctor::resolve_toolchain` (Task 9), `cargo_target::resolve` (Task 14)
- Produces: `doctor::drop_directory_toolchain(&mut Command)`, crate-visible

**Depends-on:** Task 14

- [x] **Step 1: Write the failing tests** — in `tests/capability_gate.rs`, `status` launched with `RUSTUP_TOOLCHAIN` naming a toolchain that is not installed passes the gate when `RUSTUP_TOOLCHAIN_SOURCE` is `toolchain-file` or `path-override`, or absent, and is still refused when the source is `cli` or `env`. In `tests/status_targets.rs`, the same `toolchain-file` selection leaves a built checkout's target `target.owned`. Run `cargo test --test capability_gate --test status_targets`; the three directory cases fail with "the active Rust toolchain could not be determined" and `target.unresolved`.

- [x] **Step 2: Implement** — `drop_directory_toolchain` removes `RUSTUP_TOOLCHAIN` and `RUSTUP_TOOLCHAIN_SOURCE` from a child unless the source is `cli`, `env` or `default`; `rustup_which` and `cargo_target::resolve` call it. Run the same tests; expect all to pass, and the guard to fail if the function always removes the variables.

- [x] **Step 3: Verify and commit** — run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and `cargo check --target x86_64-unknown-linux-gnu`; all exit 0 (117 passed, 1 ignored). Commit `fix: keep a directory-selected toolchain out of hive's rustup and cargo children`.

_Requirements: CORE-1.11, CORE-1.12_

### Task 18: A cancelled command stops waiting for hive's state lock

Added after execution by `root-cause` (2026-10-28), from the third whole-branch re-review. Once `status` installs its signal handlers, SIGINT only sets the cancel flag, and `State::locked` waited in a blocking `flock(LOCK_EX)` that never read it. Landed as `d432461`, which also routes the fingerprint read-cap test through `read_profile`.

**Files:**
- Modify: `src/state.rs` (`StateError::Cancelled`, `locked_or_cancelled`; `locked` delegates to it)
- Modify: `src/cli.rs` (`emit` and the `status` snapshot write wait with the cancel flag)
- Test: `tests/status_targets.rs`
- Test: `src/fingerprint.rs` (`#[cfg(test)]`)

**Reuse:** existing — `doctor::CANCEL`, the `__hold-lock flock` subcommand, `tests/status_targets.rs::wait_within`

**Interfaces:**
- Consumes: `State::locked` (Task 2), `emit` (Task 3), `status` (Task 14)
- Produces: `State::locked_or_cancelled(impl FnMut() -> bool, body) -> Result<T, StateError>` and `StateError::Cancelled`

**Depends-on:** Task 14

- [x] **Step 1: Write the failing tests** — in `tests/status_targets.rs`, `status` runs while another process holds `$HIVE_HOME/.lock` through `cargo-hive __hold-lock flock`, a `Drop` guard kills the holder, and SIGINT is sent once `lsof` shows `status` holding the lock file open; expect exit 130 within 10 s, one JSON receipt saying the status was cancelled, and no `snapshot.json`. In `src/fingerprint.rs`, `read_profile` reads a `lib-fifo.json` FIFO fed 12 MiB and must report the 4 MiB error. Run them; the lock test fails with "status did not finish within 10s".

- [x] **Step 2: Implement** — the state lock is tried with `LOCK_EX | LOCK_NB` and retried every 10 ms; `locked_or_cancelled` returns `StateError::Cancelled` when `cancel` fires while the lock is held elsewhere, and takes a free lock even after cancellation. `status` passes its cancel flag to the snapshot write; `emit` passes it to the receipt log and, when cancelled, prints the receipt under `--json`, says on stderr that it was not logged, and exits 130. Run the tests; expect them to pass, the lock test to fail with the fix reverted, and the FIFO test to fail when `read_unit` reads without the cap.

- [x] **Step 3: Verify and commit** — run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and `cargo check --target x86_64-unknown-linux-gnu`; all exit 0 (119 passed, 1 ignored). Commit `fix: stop waiting for hive's state lock once a command is cancelled`.

_Requirements: CORE-6.6, CORE-6.7_

### Task 19: A unit that is not current leaves its profile supported

Added after execution by `root-cause` (2026-10-29). A first compile that fails leaves only `invoked.timestamp` and `output-*`, and Cargo empties a unit's hash file before every rebuild and rewrites it only on success (cargo `fingerprint/mod.rs:146-150`, `536-544`, at the 1.97.1 commit). `read_profile` read both as format deviations, so one failing or rebuilding crate made the whole profile unsupported: klynt's `target/debug` was skipped by `status` and `gc`. Evidence: `.skills/research/2026-10-29-cargo-failed-unit-fingerprint-shape.md`.

**Files:**
- Modify: `src/fingerprint.rs` (`never_compiled`, `read_unit` returns `Ok(None)` for an emptied hash, `ProfileReport.not_current`)
- Modify: `src/gc/run.rs` (test fixture sets `not_current`)
- Test: `src/fingerprint.rs` (`#[cfg(test)]`)

**Reuse:** existing — `check_types`, the `scratch`/`write_unit`/`unit_json` test helpers

**Interfaces:**
- Consumes: `read_profile` (Task 6)
- Produces: `ProfileReport.not_current: usize`

**Depends-on:** Task 6

- [x] **Step 1: Write the failing tests** — a never-compiled unit dir and an emptied hash beside a signature-exact JSON (with and without `output-*`, and with a dependent unit whose edge to it becomes unresolved) each leave the profile supported with `not_current` 1. Both fail with the old reader's reasons: "0 fingerprint JSON files" and "hash file ... is not 16 lowercase hex characters".

- [x] **Step 2: Implement** — then guard the boundary: a stamp-only dir, cached messages beside an unknown file, an emptied hash beside a JSON off the signature, and a JSON with no hash file stay unsupported. Each guard fails under a matching mutation of the new code.

- [x] **Step 3: Verify** — `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (184 passed) and `cargo check --target x86_64-unknown-linux-gnu` (only the existing `entry_kind` warning). `status` on klynt reports `target/debug` supported with 2,423 units and 17,258 resolved edges.

_Requirements: CORE-4.16, CORE-4.17_
