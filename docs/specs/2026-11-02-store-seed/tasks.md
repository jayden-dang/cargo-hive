# Tasks: Store and seed (a compressed store of registry units, cloned into a new secondary checkout)

> **For agentic workers:** after plan approval, pick one execute skill:
> - `build-in-waves`: a continuous, dependency-aware scheduler;
> - `build-by-story`: human-gated story review units;
> - `build-inline`: the controller implements, with no implementer subagents.
>
> The chosen skill writes `Execution-mode:`. Steps use checkbox (`- [ ]`) syntax for tracking.

Feature code: SEED
Status: In-progress
Date: 2026-11-02
Execution-mode: continuous
Max-concurrency: auto
Requirements: ./requirements.md
Design: ./design.md

**Goal:** Add a compressed, hive-owned store of registry units and `cargo hive seed <checkout>`, which fills the store from live targets and clones into a new secondary checkout whose first build then recompiles none of the cloned units, with per-target counts in `status` and a plan that no longer claims shared bytes as freed.

**Architecture:**
- **`src/seed/`** (new, a sibling of `src/plan_tier/`) holds source reading, the store and its `store.db`, the fill under Cargo's locks, the pure choice of units, the placing of clones, and the seed record. Its public surface is `run` and `record::counts`.
- **`src/cli.rs`** loads, gates, builds the receipt and emits, as it does for plans; `status` gains four per-target counts only for a target with a seed record.
- **Small extensions:** `Unit` and `Toolchain` each keep one more fact, `Fs` gains `create_dir`, `CompressOutcome` gains `Incompressible`, and one condition changes in the ledger.

**Tech Stack:** Rust 2021, rustc/cargo 1.97.1 on macOS 27 (APFS). Existing dependencies only: `serde`, `serde_json`, `clap`, `libc`, `applesauce`. No new dependency. Integration tests run the real binary (`env!("CARGO_BIN_EXE_cargo-hive")`) on scratch fixtures under `/private/tmp/hive-it-*`.

## Global Constraints

The canonical sources (`docs/standards/` is the SSOT). Read them; do not restate them. Hashes were taken 2026-11-02; a mismatch at execute time means the source changed after planning.

| Source | sha256 (first 16) |
|---|---|
| `docs/standards/INDEX.md` | `80cd41328c0f7652` |
| `docs/standards/code.md` (bounds on every walk, read and child process; no new deps; no requirement IDs in code, comments or test names; `//!` module comments) | `afdcc8b89136f573` |
| `docs/standards/errors-logging.md` (typed errors with hand-written `Display`; expected refusals are data; exit codes) | `48f5baac3b52a552` |
| `docs/standards/testing.md` (merge gate; scratch-only fixtures under `/private/tmp/hive-it-*`; `HIVE_HOME` is the only test-only environment variable; no test-only branches in production paths) | `2f60a5b234124d90` |
| `docs/architecture/INDEX.md`: this feature relies on ARCH-1 to 11, 13 to 17, 19 | `36f62957f3f4ca1b` |
| `docs/specs/2026-11-02-store-seed/design.md` (Decisions 1 to 18) | see git |
| `.skills/SEED/decisions.md` (the owner's locks 1 to 13, and every measurement), `.skills/SEED/design-scan.md`, `.skills/SEED/design-review.md` | not in git |

Merge gate (testing standard). Every task must leave all four commands green:
- `cargo fmt --check`, after `cargo fmt`.
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `cargo check --target x86_64-unknown-linux-gnu`

Commits: each commit step is `cargo fmt`, then `git add -A && git commit -m "<subject>"`, on the feature branch the execute skill creates. No `Co-Authored-By` or generated-by line. The uncommitted docs on `main` (`CONTEXT.md`, `docs/architecture/INDEX.md`, `docs/roadmap/INDEX.md`, `docs/specs/catalog/hive.md`, this spec directory) are committed before the branch is cut; ask the owner to do so or to authorize it.

House rules for this plan:
1. Seed never compiles, never replays a build and never runs a cargo subcommand other than the `cargo metadata --no-deps --format-version 1 --locked --offline` that `cargo_target::resolve` already runs (ARCH-13, ARCH-17).
2. No requirement ID appears in source, comments or test names.
3. `gc`, `gc --plan`, `apply`, `repo`, `doctor`, `explain`, and `status` for a target with no seed record must not change in any task, apart from the byte values of Task 2. Their existing tests are the guard.
4. Writes into a Cargo target dir go through `Fs::create_dir` and `Fs::clone_file` only. Writes to `store.db` and `seeds.json` go through the `state` module only, never holding the state lock across a clone or a compression.
5. Adding a `Command` variant breaks `tests/skill.rs` at compile time; Task 12 maps it in `Parsed` and `parse` in the same commit.
6. No test-only branch or environment variable in a production path. Failure and stopping are injected through the `Fs` wrapper `Recording` and the `cancel` closure.
7. Code names the two numbers `toolchain_value` and `fingerprint_hash`. Never `rustc` or `fingerprint` alone (design, Layout).
8. Tests never touch the real `~/.cargo` or `~/.cache`; `Hive` gives a scratch `CARGO_HOME`. The one exception is Task 17, and only once the owner approves it in `docs/standards/testing.md`.
9. `hive.json(&[...])` appends `--json` itself; passing it again exits 2. `cargo_json` redirects stdout to a file, since `wait_within` does not drain a pipe.
10. Words: `fingerprint_hash` is the content of a unit's hash file, used for edges and in the store key. Files in `deps/` are named by the metadata hash, `fingerprint::unit_hash(dir_name)`. They are different numbers.
11. A store key's path segment is `format!("{name_meta}-{fingerprint_hash}")` under `store/<toolchain_value>/`. `filled_at` and `last_live_at` are epoch seconds; stamps are nanoseconds.

**How to read the steps.** A step's test code is complete. Implementation steps give every signature and the order of work, and point at the numbered design decision that holds the reasoning; the full reasoning does not repeat here. A pure or very small body is given in full.

## File Structure

| Path | Change | Responsibility |
|---|---|---|
| `src/seed/mod.rs` | create | `run`, `Outcome`, `Error`, the five steps in order |
| `src/seed/source.rs` | create | Live registry units of a target, the registry rule, a unit's file list |
| `src/seed/store.rs` | create | `store.db` types, `read`, `merge`, `compress_and_publish`, layout |
| `src/seed/fill.rs` | create | The lock window, the clone into `.tmp`, the tag, the toolchain row |
| `src/seed/choose.rs` | create | Pure choice of units |
| `src/seed/place.rs` | create | Create the target, clone tag and units, read sharing, store bytes |
| `src/seed/record.rs` | create | `seeds.json`, `write`, `counts` |
| `src/lib.rs` | modify | `pub mod seed;` |
| `src/cli.rs` | modify | `Command::Seed`, `seed_cmd`, the record argument of `report_targets` |
| `src/state.rs` | modify | `State::read_bounded`, unlocked |
| `src/doctor.rs` | modify | `Toolchain.version_text` |
| `src/fingerprint.rs` | modify | `Unit.toolchain_value` |
| `src/explain.rs` | modify | Test helper literal of `Unit`; `DirListFs::create_dir`; `FakeFs::create_dir` |
| `src/liveness.rs` | modify | `embedded_hashes` to `pub(crate)`; `FakeFs::create_dir` |
| `src/gc/ledger.rs` | modify | The compressed `SHARES_ALL` condition and one table case |
| `src/gc/run.rs` | modify | `Rec::create_dir` |
| `src/plan_tier/build.rs` | modify | `FailStat::create_dir` |
| `src/platform/mod.rs` | modify | `Fs::create_dir`, `CompressOutcome::Incompressible`, its `Display`, the `#[cfg(test)]` double `Hooked` |
| `src/platform/apfs.rs` | modify | `create_dir`, typed `Watch` skip |
| `tests/liveness.rs` | modify | `Volumes::create_dir` forwards |
| `tests/common/recording_fs.rs` | modify | Forward `create_dir`; log every mutating call and each unlock; `fail_clone`, `plant_before_clone`, `on_compress` |
| `tests/common/mod.rs` | modify | `pub mod registry_fixture;`, `Hive::hold_state_lock`, `Hive::write_capabilities` |
| `tests/common/plan_fixture.rs` | modify | `LibCtx::open(hive, fs)`; `Planned::library_ctx_with` delegates to it |
| `tests/common/registry_fixture.rs` | create | A `local-registry` crate with a build script under the scratch `CARGO_HOME` |
| `tests/seed.rs` | create | The binary seam and the real-cargo seam |
| `tests/seed_fill.rs` | create | `seed::fill` and `seed::place` through `Recording` |
| `tests/status_seed.rs` | create | `status` counts on a real seeded target |
| `tests/skill.rs` | modify | `Parsed::Seed`, `parse`, `want`, path-word rule |
| `plugin/skills/hive/SKILL.md` | modify | Teach `seed` |
| `skills/hive/eval.json`, `skills/hive/TESTS.md` | modify | One recorded eval for `seed` |
| `README.md` | modify | One command line and the not-for-daily-use note |
| `docs/specs/2026-11-02-store-seed/klynt-run.md` | create | The dated klynt record |
| `docs/standards/testing.md` | modify | The owner-approved exception for the klynt run (Task 17) |

Retrieval: `blast_radius` on these paths names CORE, AUTOGC, EXPLAIN, LIVE, GCPLAN and SKILL, all `Implemented`, so no other open plan collides on a file. `cluster(SEED)`: SEED has no `tasks.md` yet, so its OWNS is empty and no member is eligible; OWNS coverage 6 of 7.

---

### Task 1: A unit keeps its toolchain value, and a toolchain keeps its `-vV` text

**Files:**
- Modify: `src/fingerprint.rs` (`read_unit`, line 285; `Unit`, line 79)
- Modify: `src/explain.rs` (test helper literal at line 947)
- Modify: `src/doctor.rs` (`Toolchain`, line 133; `resolve_toolchain`, line 387)
- Test: `src/fingerprint.rs`, `src/doctor.rs` (`#[cfg(test)]`)

**Reuse:** rung 2 — extends `Unit` and `Toolchain`

**Interfaces:**
- Consumes: nothing new
- Produces: `Unit.toolchain_value: u64` with `#[serde(skip)]`; `Toolchain.version_text: String` with `#[serde(skip)]`, the whole stdout of `rustc -vV`

**Depends-on:** none

- [x] **Step 1: Write the failing tests.** In `src/fingerprint.rs`, `a_unit_keeps_the_toolchain_value_of_its_json`: write one unit dir whose JSON is the file's `unit_json(&[])` with its fixed `rustc` value replaced by `776363091828236830`, call `read_profile`, and assert `graph.units[0].toolchain_value == 776363091828236830`. Also `a_file_outside_the_known_shape_stays_unsupported`, if the file's existing table does not already pin it, with an extra JSON key. In `src/doctor.rs`, `a_resolved_toolchain_keeps_the_whole_version_text`: `resolve_toolchain(scratch)`, assert `version_text` starts with `rustc ` and contains both `host: ` and `release: {release}`. Run `cargo test --lib fingerprint:: doctor::`; expect a compile failure on the missing fields.
- [x] **Step 2: Implement.** Add the fields. In `read_unit`, set `toolchain_value` from `obj["rustc"].as_u64()`, which `check_types` has already proved is a `u64`; `check_types` keeps the same 11 keys. In `resolve_toolchain`, keep `text.to_string()` before the existing `release` parse. Set `toolchain_value: 0` in the `explain.rs` helper. Run `cargo test`; expect pass, with no receipt change since both fields are skipped.
- [x] **Step 3: Commit.** `feat(fingerprint): keep each unit's toolchain value and the whole rustc version text`

_Requirements: SEED-10.10, SEED-10.11_

---

### Task 2: A compressed file that shares all its blocks is not counted as freed

**Files:**
- Modify: `src/gc/ledger.rs` (the condition at line 66; the table in `freed_bytes_follow_links_and_sharing`, near line 191)
- Test: `src/gc/ledger.rs`

**Reuse:** rung 2 — one condition at `src/gc/ledger.rs:66`

**Interfaces:**
- Consumes: `Sharing`, `EntryStat`
- Produces: unchanged signatures

**Depends-on:** none

- [x] **Step 1: Write the failing test.** Add table rows, using the file's `stat` helper and a new `shares_all_flagged(private)` helper beside `clone_flagged` that sets `may_share: true, shares_all: true, clone_id: 7`:

```rust
(
    "compressed and sharing every block: nothing is freed",
    vec![(stat(7, 1, 8, true), shares_all_flagged(0))],
    Bytes { freed: 0, still_shared: 4096 },
),
(
    "uncompressed and sharing every block: as before",
    vec![(stat(8, 1, 8, false), shares_all_flagged(0))],
    Bytes { freed: 0, still_shared: 4096 },
),
```

Run `cargo test --lib gc::ledger`; expect the first row to fail with `freed: 4096`. The existing row `compressed: private size is not trusted` must keep passing: it sets `shares_all: false`.
- [x] **Step 2: Implement.** Replace the condition at line 66, so that the rule is the flag and not a measured zero:

```rust
if s.shares_all && stat.compressed {
    seen.private = Some(0);
} else if (s.may_share || s.shares_all) && !stat.compressed {
    seen.private = Some(s.private_bytes);
}
```

Run `cargo test`; expect pass.
- [x] **Step 3: Commit.** `fix(gc): do not count a compressed file that shares all its blocks as freed`

_Requirements: SEED-5.1, SEED-10.7_

---

### Task 3: `Fs` can create a directory, and compression reports a file it cannot shrink

**Files:**
- Modify: `src/platform/mod.rs` (`trait Fs`, line 293; `CompressOutcome`, line 129; its `Display`)
- Modify: `src/platform/apfs.rs` (`compress`, line 140; `Watch`, lines 334 and 343 to 374)
- Modify: `src/explain.rs`, `src/liveness.rs`, `src/doctor.rs`, `src/gc/run.rs`, `src/plan_tier/build.rs`, `tests/liveness.rs`, `tests/common/recording_fs.rs` (each `impl Fs`)
- Test: `src/platform/apfs.rs`

**Reuse:** rung 2 — extends `Fs` and `CompressOutcome`; `HeldLock::with_identity` (`src/platform/mod.rs:218`) for the unlock log line, which keeps `locks::acquire`'s identity check true

**Interfaces:**
- Consumes: nothing new
- Produces: `fn create_dir(&self, path: &Path) -> io::Result<()>` as a required `Fs` method, non-recursive; `CompressOutcome::Incompressible`; `#[cfg(test)] pub(crate) struct Hooked` in `src/platform/mod.rs`, which forwards every method to `inner` and overrides `compress`, `volume`, `sharing` and `entry_stat` by optional closure (a tenth `impl Fs`); on `Recording`: `fail_clone(self, dst: impl AsRef<Path>, errno: i32)`, `plant_before_clone(self, dst: impl AsRef<Path>, bytes: &[u8])`, `on_compress(self, f: impl Fn(&Path) + Send + Sync + 'static)`, and log lines `clone <src> -> <dst>`, `create_dir <path>`, `compress <path>`, `write_file <path>`, `hardlink <path>`, `set_mtime <path>`, `temp_dir <path>`, `unlock <path>`

**Depends-on:** none

- [x] **Step 1: Write the failing tests** in the existing test module of `src/platform/apfs.rs`, using its helpers `apfs()` and `tmp(label)`:

```rust
#[test]
fn a_file_too_small_or_empty_is_incompressible_not_failed() {
    let fs = apfs();
    let t = tmp("incompressible");
    let dir = t.path();
    for (name, bytes) in [("empty", &b""[..]), ("hash", &b"ca2ffbab1cb52c6f"[..])] {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(fs.compress(&path), CompressOutcome::Incompressible, "{name}");
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn create_dir_refuses_an_existing_path_and_a_missing_parent() {
    let fs = apfs();
    let t = tmp("create-dir");
    let dir = t.path();
    let made = dir.join("made");
    fs.create_dir(&made).unwrap();
    assert_eq!(fs.create_dir(&made).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        fs.create_dir(&dir.join("absent/child")).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    let link = dir.join("dangling");
    std::os::unix::fs::symlink(dir.join("nowhere"), &link).unwrap();
    assert_eq!(fs.create_dir(&link).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
}
```

Run `cargo test --lib platform::`; expect a compile failure.
- [x] **Step 2: Implement.** `Apfs::create_dir` is `std::fs::create_dir(path)`. In `Watch` and `WatchTask`, add `incompressible: Arc<AtomicBool>`, set by `file_skipped` for `SkipReason::EmptyFile` and by `Task::not_compressible_enough` in place of the failure text. `compress` checks it before the `UF_COMPRESSED` test and returns `Incompressible`. Every other skip stays `Failed`, `SkipReason::AlreadyCompressed` included. `Display` gains a line. In each other `impl Fs`: wrappers (`FailStat`, `Volumes`, `Recording`) forward to `inner`, and `liveness::FakeFs` to `self.real`; `DirListFs` returns `Unsupported`, as its other methods do; `doctor::FakeFs`, `explain::FakeFs` and `gc/run.rs` `Rec` follow their neighbors. Non-macOS builds compile, since `Apfs` is already gated.
- [x] **Step 3: Run.** `cargo test`; expect pass, doctor's check b included: it compares with `==`. `cargo check --target x86_64-unknown-linux-gnu`.
- [x] **Step 4: Commit.** `feat(platform): Fs::create_dir, and a compression outcome for files it cannot shrink`

_Requirements: SEED-2.23_

---

### Task 4: A real registry unit to test against

**Files:**
- Create: `tests/common/registry_fixture.rs`
- Modify: `tests/common/mod.rs` (`pub mod registry_fixture;`, `hold_state_lock`, `write_capabilities`)
- Modify: `tests/common/plan_fixture.rs` (`LibCtx::open`)
- Test: `tests/seed.rs`

**Reuse:** rung 2 — `Hive`, `Scratch`, `git`, `register`, `wait_within` (`tests/common/mod.rs`)

**Interfaces:**
- Consumes: `Hive { home, cargo_home }`, `Hive::command`, `Hive::config`
- Produces: `pub struct RegistryRepo { pub scratch: Scratch, pub main: PathBuf }`; `RegistryRepo::new(hive: &Hive) -> RegistryRepo`; `RegistryRepo::worktree(&self, name: &str) -> PathBuf`; `pub fn cargo_json(hive: &Hive, dir: &Path, args: &[&str]) -> Compiled`; `pub struct Compiled { pub registry_fresh: Vec<String>, pub registry_recompiled: Vec<String>, pub workspace_recompiled: Vec<String> }`; `Hive::hold_state_lock(&self) -> Child`; `Hive::write_capabilities(&self, disabled: serde_json::Value)`, which `write_ready_capabilities` then calls; `LibCtx::open(hive: &Hive, fs: Box<dyn Fs>) -> LibCtx`

**Depends-on:** none

- [x] **Step 1: Write the failing test** in `tests/seed.rs`:

```rust
#![cfg(target_os = "macos")]
mod common;
use common::registry_fixture::{cargo_json, RegistryRepo};
use common::Hive;

#[test]
fn the_fixture_builds_a_registry_crate_with_a_build_script_offline() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    let built = cargo_json(&hive, &repo.main, &["build"]);
    assert!(built.registry_recompiled.iter().any(|n| n == "depcrate"), "{:?}", built.registry_recompiled);
    assert!(built.registry_recompiled.iter().any(|n| n == "leafcrate"));
    let deps = repo.main.join("target/debug/deps");
    let dep_info = std::fs::read_dir(&deps).unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "d")
            && p.file_name().unwrap().to_string_lossy().starts_with("depcrate-"))
        .unwrap();
    let first = std::fs::read_to_string(dep_info).unwrap();
    let cargo_home = hive.cargo_home.path.join("registry/src");
    assert!(first.contains(&*cargo_home.to_string_lossy()), "{first}");
    let again = cargo_json(&hive, &repo.main, &["build"]);
    assert!(again.registry_recompiled.is_empty());
}
```

Run `cargo test --test seed`; expect a compile failure.
- [x] **Step 2: Implement the fixture,** exactly as probed in plan review (`.skills/SEED/plan-review.md`, M-5):
  - `<reg>/leafcrate-1.0.0.crate` and `<reg>/depcrate-1.0.0.crate`, each packed with `COPYFILE_DISABLE=1 tar -czf <reg>/<name>-1.0.0.crate <name>-1.0.0` from the crate's parent directory. `cksum` is the first field of `shasum -a 256 <file>`.
  - `leafcrate`: `pub fn leaf() -> u32 { 1 }`. `depcrate`: `build = "build.rs"`, `[dependencies] leafcrate = "1"`; `build.rs` writes `$OUT_DIR/generated.rs` holding `pub const G: u32 = 7;`; `src/lib.rs` is `include!(concat!(env!("OUT_DIR"), "/generated.rs")); pub fn dep() -> u32 { leafcrate::leaf() + G }`. Both edition 2021.
  - `<reg>/index/le/af/leafcrate`: `{"name":"leafcrate","vers":"1.0.0","deps":[],"cksum":"<sha256>","features":{},"yanked":false}`
  - `<reg>/index/de/pc/depcrate`: `{"name":"depcrate","vers":"1.0.0","deps":[{"name":"leafcrate","req":"^1","features":[],"optional":false,"default_features":true,"target":null,"kind":"normal"}],"cksum":"<sha256>","features":{},"yanked":false}`
  - `<cargo_home>/config.toml`: `[source.crates-io]`, `replace-with = "fixture"`, `[source.fixture]`, `local-registry = "<reg>"`.
  - `main/`: `git init`; `Cargo.toml` with `name = "app"`, `version = "0.1.0"`, edition 2021, `[dependencies] depcrate = "1"`; `src/lib.rs` is `pub fn app() -> u32 { depcrate::dep() }`; `.gitignore` holds `target`; `cargo generate-lockfile --offline`; then a commit, so every worktree has `Cargo.lock` for `--locked`. `hive.config` names the scratch in `secondary_roots`; then `register`. `worktree(name)` is `git worktree add <scratch>/<name> -b <name>`.
  - `cargo_json`: `Command::new("cargo")`, not `Hive::command`, with `.env("CARGO_HOME", &hive.cargo_home.path)` and the same `env_remove` lines as `Hive::build_crate`; `<args> --offline --message-format=json-render-diagnostics`; stdout to a file, then `wait_within`. Parse `compiler-artifact` lines. A unit's name is the text between `#` and `@` in `package_id`; it is registry when `package_id` starts with `registry+`.
  - `write_capabilities` is the body of `write_ready_capabilities` with `"disabled": disabled`, `"lock_check": 1` and `"failed_marker_check": 1`. Disabling looks like `{"seeding": ["a"]}`, `{"store_compression": ["b"]}`, `{"deletion": ["d"]}`.
  - `LibCtx::open` is the body of `Planned::library_ctx_with`, reading `hive.home.path`, and `library_ctx_with` delegates to it.
  - `hold_state_lock` is `self.hold_lock_child("flock", &self.home.path.join(".lock"))`. That file exists once any command has run.
- [x] **Step 3: Run.** `cargo test --test seed`; expect pass.
- [x] **Step 4: Commit.** `test(seed): a local-registry fixture with a build script, and a state-lock holder`

_Requirements: SEED-1.9_

---

### Task 5: Read the live registry units of a target, and the files of one unit

**Files:**
- Create: `src/seed/mod.rs` (module header, `pub mod source;` only, for now)
- Create: `src/seed/source.rs`
- Modify: `src/lib.rs` (`pub mod seed;`)
- Modify: `src/liveness.rs` (line 626: `embedded_hashes` to `pub(crate)`)
- Test: `src/seed/source.rs`

**Reuse:** rung 2 — `plan_tier::checkout_walk`, `discover::targets` with `Snapshot::default()` (`src/plan_tier/build.rs:213`), `liveness::liveness`, `fingerprint::le_hex`, `liveness::embedded_hashes` (made `pub(crate)`), `Fs::read_dir_bounded`, `Fs::entry_stat`, `gc::DEPS_CAP`, `gc::CRATE_DIR_CAP`; the classifier is rung 7, as no code reads dep-info

**Interfaces:**
- Consumes: `Unit.toolchain_value` (Task 1), `plan_tier::Ctx`, `TargetDir`, `Liveness`
- Produces: `pub struct SourceUnit { pub rel_profile: PathBuf, pub dir_name: String, pub stem: String, pub kind: String, pub fingerprint_hash: String, pub toolchain_value: u64, pub registry_deps: Vec<String> }`; `pub enum Skip { Undecided(String), Unsupported(String), Bound(String) }`; `pub fn owned_targets(ctx: &Ctx, cancel: &(dyn Fn() -> bool + Sync)) -> (Vec<TargetDir>, Vec<String>)`, whose strings are one finding for each walk fault and each `Owner::Unresolved` target; `pub fn live_registry_units(ctx: &Ctx, target: &TargetDir, cargo_home: &Path, window: Duration) -> Result<(Vec<SourceUnit>, Vec<String>), Skip>` (the `Vec<String>` names units left out for an unresolved edge); `pub fn cargo_home() -> PathBuf`; `pub fn is_registry_dep_info(first_line: &str, cargo_home: &Path) -> bool`; `pub fn unit_files(fs: &dyn Fs, target: &Path, unit: &SourceUnit, deps_listing: &[(String, EntryKind)], dir_cap: usize) -> Result<Vec<PathBuf>, Refused>`; `pub struct Refused(pub PathBuf, pub String)`

**Depends-on:** Task 1

- [x] **Step 1: Write the failing tests** in `src/seed/source.rs`:

```rust
#[test]
fn the_first_input_decides_whether_a_unit_is_a_registry_unit() {
    let home = Path::new("/Users/x/.cargo");
    let cases = [
        ("/t/deps/a-1.d: /Users/x/.cargo/registry/src/index.crates.io-1/a-1.0.0/src/lib.rs", true),
        ("/t/deps/b-2.d: src/lib.rs src/x.rs", false),
        ("/t/deps/c-3.d: src/lib.rs /Users/x/.cargo/registry/src/index.crates.io-1/a-1.0.0/data.txt", false),
        ("/t/deps/d-4.d: /Users/x/.cargo/git/checkouts/d/src/lib.rs", false),
        ("/t/deps/e-5.d:", false),
        ("/t/deps/f-6.d: /Users/x\\ y/.cargo/registry/src/i/f/src/lib.rs", false),
    ];
    for (line, want) in cases {
        assert_eq!(is_registry_dep_info(line, home), want, "{line}");
    }
    let spaced = Path::new("/Users/x y/.cargo");
    assert!(is_registry_dep_info("/t/deps/f-6.d: /Users/x\\ y/.cargo/registry/src/i/f/src/lib.rs", spaced));
}
```

Add, on hand-built target dirs: `a_unit_with_no_dep_info_needs_a_resolved_edge_and_only_registry_edges` (a run-build-script unit with zero edges is not registry; with one registry edge is; with one workspace edge is not); `a_live_unit_with_an_unresolved_edge_is_left_out_and_named`; `a_not_current_unit_is_not_filled`; `an_undecided_or_unsupported_target_is_a_skip_with_its_reason`; `unit_files_takes_the_whole_unit_dirs_and_hash_named_deps_but_no_rcgu_objects`; `unit_files_walks_a_dsym_directory_within_its_bound`; `a_symlink_in_a_unit_refuses_the_whole_unit`; `a_deps_listing_over_its_cap_is_a_skip`; `a_unit_dir_at_its_cap_lists_and_one_past_it_refuses`, with `dir_cap` 2 and 3; `a_dep_info_file_over_its_read_bound_skips_the_target`. Run `cargo test --lib seed::source`; expect a compile failure.
- [x] **Step 2: Implement.** `is_registry_dep_info`: take the text after the first `: `, read the first input up to the first space not preceded by a backslash, undo `\ `, and test `starts_with(<cargo_home>/registry/src/)`. `cargo_home` is `CARGO_HOME` else `$HOME/.cargo`. `live_registry_units` (design decisions 10 and 11): call `liveness()`; `Undecided` and `Format::Unsupported` map to `Skip`. List each profile's `deps/` with `DEPS_CAP`. For each live unit, find its record in `profiles[].graph.units`; a unit absent from the graph has an empty hash file and is filtered. Classify from its `.d` (`deps/<crate>-<metadata>.d`, `<crate>` the package name and `<metadata>` `fingerprint::unit_hash` of the unit directory, not the fingerprint-file stem, or `build/<dir>/*-<hash>.d`, each read stops at 1 MiB), then iterate to a fixpoint for units with no `.d`, following `le_hex(dep.fingerprint)` to `hash_hex` over all profiles. Record `registry_deps` as the fingerprint hashes of edges that resolved to registry units. An edge that resolves to nothing leaves the unit out. `rel_profile` is the profile path relative to `target.path`. `owned_targets` calls `checkout_walk(ctx)`, whose `ctx.repos` holds the one repo, then `discover::targets(&checkouts, ctx.cfg, false, &mut Snapshot::default(), cancel)`, keeps `Owner::Owned`, and reports the rest. `unit_files` recurses under `.fingerprint/<dir>/` and `build/<dir>/` with `dir_cap` per directory and `size::MAX_DEPTH` as a depth bound, and picks `deps_listing` names for which `embedded_hashes(name)` yields `fingerprint::unit_hash(dir_name)`, leaving out `.rcgu.o`, recursing into a directory such as a `.dSYM`; any entry whose `entry_stat` kind is not `File` or `Dir` is `Refused`; a non-UTF-8 name under `build/<dir>/` is `Refused` too.
- [x] **Step 3: Run.** `cargo test`; expect pass.
- [x] **Step 4: Commit.** `feat(seed): read a target's live registry units and one unit's files`

_Requirements: SEED-2.2, SEED-2.3, SEED-2.4, SEED-2.24, SEED-9.7_

---

### Task 6: The store holds complete, compressed entries

**Files:**
- Create: `src/seed/store.rs`
- Modify: `src/seed/mod.rs` (`pub mod store;`, `pub enum Error`)
- Modify: `src/state.rs` (`State::read_bounded`)
- Test: `src/seed/store.rs`

**Reuse:** rung 2 — `Tx::read_bounded`, `Tx::write_bounded`, `State::locked_or_cancelled`, the read-merge-write of `plan_tier/record.rs`, `Fs::compress`, `Fs::volume`; `std::fs::rename` (rung 3) for publishing

**Interfaces:**
- Consumes: `CompressOutcome::Incompressible` (Task 3), `State`, `Tx`, `Fs`
- Produces: `pub const STORE_FILE: &str = "store.db"`; `pub const MAX_STORE_BYTES: u64 = 32 * 1024 * 1024`; `pub enum Error { State(StateError), DiskFull(String) }` in `src/seed/mod.rs`, with `Debug` and a hand-written `Display` and `Error`; `State::read_bounded<T: DeserializeOwned + Default>(&self, name: &str, max: u64) -> Result<T, StateError>`, taking no lock, safe because writes replace the file by rename; `pub struct StoreFile { pub units: Vec<Row>, pub toolchains: BTreeMap<String, u64> }`; `pub struct Row { pub toolchain_value: u64, pub name_meta: String, pub fingerprint_hash: String, pub kind: String, pub rel_profile: PathBuf, pub size: u64, pub filled_at: u64, pub last_live_at: u64, pub repos: Vec<PathBuf>, pub registry_deps: Vec<String> }`; `pub fn store_dir(state: &State) -> PathBuf`; `pub fn entry_dir(state: &State, row: &Row) -> PathBuf`; `pub fn tag_path(state: &State) -> PathBuf`; `pub fn tmp_dir(state: &State, key: &str) -> PathBuf`; `pub fn sweep_tmp(state: &State, now: SystemTime) -> Result<(), String>`; `pub fn read(state: &State) -> Result<StoreFile, StateError>` (available rows only); `pub enum Published { Added, Held }`; `pub enum PublishFailed { Compress(String), DiskFull(String), Stopped(String) }`; `pub fn compress_and_publish(fs: &dyn Fs, state: &State, tmp: &Path, row: &Row, cancel: &(dyn Fn() -> bool + Sync)) -> Result<Published, PublishFailed>`; `pub fn merge(state: &State, rows: Vec<Row>, table: BTreeMap<String, u64>, cancel: &(dyn Fn() -> bool + Sync)) -> Result<(), StateError>`

**Depends-on:** Task 3

- [x] **Step 1: Write the failing tests** in `src/seed/store.rs`, on a scratch state root, `platform::current()`, and `platform::Hooked` for injected failures. A test-local `three_file_entry(state, fs) -> (PathBuf, Row)` writes into `tmp_dir` one 4 KiB `debug/deps/libd-0123456789abcdef.rlib`, one 16-byte `debug/.fingerprint/d-0123456789abcdef/lib-d` and one empty `debug/.fingerprint/d-0123456789abcdef/invoked.timestamp`.

```rust
#[test]
fn a_stop_at_any_step_leaves_nothing_available_and_a_later_attempt_finishes() {
    // Polls come before each of the three files and before the rename. A free state lock
    // does not poll, so the state "renamed, no row" is its own test below.
    for stop_after in 0..=3 {
        let (state, fs) = scratch_state(&format!("stop-{stop_after}"));
        let (tmp, row) = three_file_entry(&state, &*fs);
        let polls = std::sync::atomic::AtomicUsize::new(0);
        let cancel = || polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= stop_after;
        let published = compress_and_publish(&*fs, &state, &tmp, &row, &cancel);
        assert!(matches!(published, Err(PublishFailed::Stopped)), "stop_after {stop_after}");
        assert!(read(&state).unwrap().units.is_empty());
        let (tmp, row) = three_file_entry(&state, &*fs);
        compress_and_publish(&*fs, &state, &tmp, &row, &|| false).unwrap();
        merge(&state, vec![row.clone()], Default::default(), &|| false).unwrap();
        assert_eq!(read(&state).unwrap().units.len(), 1);
        assert!(fs.entry_stat(&entry_dir(&state, &row).join("debug/deps/libd-0123456789abcdef.rlib")).unwrap().compressed);
    }
}
```

Add: `a_directory_with_no_row_is_adopted_by_merge_without_cloning`, the hand-built "renamed, no row" state; `a_row_whose_directory_is_gone_is_not_read_and_is_pruned_at_the_next_merge`; `a_rename_that_finds_the_entry_present_counts_as_held_and_discards_its_copy`; `merge_adds_a_repo_and_moves_last_live_at_for_a_held_key`; `a_compress_failure_other_than_too_small_leaves_the_entry_unavailable`; `a_failure_with_little_free_space_is_disk_full`, with `Hooked` returning `Failed` and a low `available_bytes`; `a_file_already_compressed_is_finished`; `a_store_over_its_cap_reports_the_failure_and_leaves_the_file_unchanged`, comparing bytes; `read_bounded_refuses_a_file_over_its_bound_and_takes_no_lock`, in `src/state.rs`. The binary half of the state-lock proof needs the command, so it is in Task 12. Run `cargo test --lib seed::store`; expect a compile failure.
- [x] **Step 2: Implement** (design decisions 1, 2, 5, 6, 16, 17). `read` uses `State::read_bounded`, then filters by one `entry_stat` per row. `fill` removes a stale `tmp_dir` for its own key before cloning into it; a stopped publish leaves its tmp for `sweep_tmp`. `compress_and_publish` polls `cancel` before each file and before the rename; for each regular file under `tmp` with `nlink` 1 and not already compressed, calls `Fs::compress`; `Compressed` and `Incompressible` continue; `SkippedHardLink` and `Failed` end the entry, and after `Failed` it reads `Fs::volume(tmp).available_bytes`, returning `DiskFull` when that is below the file's size. Create the parent `store/<toolchain_value>/` with `std::fs::create_dir_all`, then `std::fs::rename(tmp, entry_dir)`. `EEXIST` or `ENOTEMPTY` removes `tmp` and returns `Held`. `merge` runs inside `locked_or_cancelled`: `tx.read_bounded`, drop rows whose directory is gone, upsert by store key, adding `repos` and taking the later `last_live_at`, insert table rows, `write_bounded` with `MAX_STORE_BYTES`. `sweep_tmp` removes `.tmp` entries older than one day, and returns an error naming the path when the listing hits its cap or a stat or removal fails. A missing `.tmp` directory is success. Every error names its path.
- [x] **Step 3: Run.** `cargo test`; expect pass.
- [x] **Step 4: Commit.** `feat(seed): a store of complete, compressed entries with an atomic publish`

_Requirements: SEED-2.9, SEED-2.10, SEED-2.12, SEED-2.13, SEED-2.20, SEED-2.21, SEED-2.22, SEED-9.3, SEED-9.4_

---

### Task 7: Fill clones new units under Cargo's locks and compresses outside them

**Files:**
- Create: `src/seed/fill.rs`
- Modify: `src/seed/mod.rs` (`pub mod fill;`)
- Test: `tests/seed_fill.rs`

**Reuse:** rung 2 — `locks::acquire` and `Locked` unchanged, dropped before compressing (`src/plan_tier/locks.rs:43`), `Fs::clone_file`, `Fs::entry_stat`

**Interfaces:**
- Consumes: `source::{owned_targets, live_registry_units, unit_files, SourceUnit}` (Task 5), `store::{read, merge, compress_and_publish, tmp_dir, tag_path, sweep_tmp, Row}` (Task 6), `locks::{acquire, Acquired, Reason}`, `Recording::fail_clone` (Task 3)
- Produces: `pub struct FillReport { pub entries_added: u64, pub entries_held: u64, pub targets_read: u64, pub targets_skipped: BTreeMap<&'static str, u64>, pub findings: Vec<(String, Option<PathBuf>, String)>, pub cancelled: bool }`; `pub fn fill(ctx: &Ctx, repo: &Repo, targets: &[TargetDir], cancel: &(dyn Fn() -> bool + Sync)) -> Result<FillReport, Error>`

**Depends-on:** Task 4, Task 5, Task 6

- [x] **Step 1: Write the failing tests** in `tests/seed_fill.rs`, each building `RegistryRepo`, and calling `seed::fill::fill` with `LibCtx::open(&hive, recording.fs())`, whose `repos` it narrows to one:
  - `a_new_unit_is_cloned_under_the_locks_and_compressed_after_they_are_dropped`: in the `Recording` log, every `clone` line sits between the `lock` lines and the first `unlock`, and every `compress` line after the last `unlock`; `on_compress` calls `Fs::try_lock_elsewhere(LockKind::Flock, &lock)` and records `LockState::Free`.
  - `a_held_lock_skips_the_target_and_waits_for_nothing`, with `hive.hold_lock_child("flock", &target.join("debug/.cargo-build-lock"))`; the report counts `targets_skipped_held`; the store gains nothing.
  - `a_target_with_nothing_new_takes_no_lock`: the log from its length before the second fill holds no `lock` line.
  - `a_clone_failure_leaves_the_entry_out_with_one_finding`, with `fail_clone(dst, libc::EIO)`.
  - `cross_volume_stops_the_target_with_one_finding`, with `fail_clone(dst, libc::EXDEV)`.
  - `a_symlink_in_a_source_unit_fills_nothing_for_it`.
  - `the_source_keeps_its_bytes_and_modification_times`: snapshot every file's bytes and mtime before and after; the log holds no `write_file`, `set_mtime` or `create_dir` under the source.
  - `the_tag_is_cloned_once_even_when_no_key_is_new`: delete the store's tag, fill again, and assert it returns.
  - `a_seeded_target_no_build_has_touched_counts_as_lock_missing`.
  - `a_cancelled_fill_merges_what_it_published`, with a closure that stops after the first entry.

  `locks.rs` does not change, so the existing cases at `tests/plan_tier.rs:132-148` are the guard for `Skipped(Missing)`. Run `cargo test --test seed_fill`; expect a compile failure.
- [x] **Step 2: Implement** (design decisions 6, 7, 8). `fill`: `sweep_tmp`, and a sweep error is one finding that stops the fill; `read` the store; for each target: `live_registry_units` (a `Skip` is one finding and `targets_skipped`; a non-empty left-out list is one finding naming the target and those units, and the other units are still filled); split units into held and new, where held means a row or a final directory exists; if new is empty and the tag exists, go on. Otherwise `acquire(ctx.fs, &target.path, false)`; a `Skipped(reason)` adds `reason.count_name()`, a `Failed` is one finding. Under the lock: list `deps/` once per profile with `DEPS_CAP`, the listing the clones use, then for each new unit, poll `cancel`, re-read its hash file at `.fingerprint/<dir_name>/<stem>`, bounded at 64 bytes, and skip it if it changed; `unit_files`; `create_dir_all` under `tmp_dir`, removing a stale one first, then `clone_file` each file. `EXDEV` ends this target. Clone the tag when absent. Drop `Locked` explicitly. Then for each cloned unit, `compress_and_publish`. Merge rows per target, also when cancelled, with `size` summed from `entry_stat.blocks * 512`, `filled_at` and `last_live_at` from `ctx.now` in epoch seconds, and `repos: vec![repo.main.clone()]`.
- [x] **Step 3: Run.** `cargo test`; expect pass.
- [x] **Step 4: Commit.** `feat(seed): fill the store under Cargo's locks, compressing after they are dropped`

_Requirements: SEED-2.5, SEED-2.6, SEED-2.7, SEED-2.8, SEED-2.14, SEED-2.19, SEED-9.8, SEED-10.8_

---

### Task 8: Fill learns the toolchain table

**Files:**
- Modify: `src/seed/fill.rs`
- Test: `tests/seed_fill.rs`

**Reuse:** rung 2 — `doctor::resolve_toolchain`, `Toolchain.version_text`, `store::merge`

**Interfaces:**
- Consumes: `Toolchain.version_text` (Task 1), `doctor::resolve_toolchain`, `store::merge` (Task 6)
- Produces: `pub fn rustc_info_version(text: &str) -> Result<String, String>`; `pub fn toolchain_row(target: &TargetDir, source_checkout: &Path) -> Result<(String, u64), String>`

**Depends-on:** Task 7

- [x] **Step 1: Write the failing tests:**
  - In `src/seed/fill.rs`, `rustc_info_version` table: one `outputs` entry whose `stdout` has `release:` gives that stdout; none is an error; two are an error; not JSON is an error; JSON with no `outputs` is an error.
  - In `tests/seed_fill.rs`: `a_clean_target_writes_a_row_matching_its_toolchain`: the table's key equals `resolve_toolchain(main).version_text` and its value equals the units' value; `a_target_with_two_toolchain_values_writes_no_row_and_one_finding`: rewrite one unit JSON's `rustc` in a copy; `a_malformed_rustc_info_writes_no_row_and_one_finding`; `a_missing_rustc_info_writes_no_row_and_one_finding`; `text_that_differs_from_the_source_checkout_writes_no_row`; `two_targets_with_one_toolchain_share_one_row`, where the table has one key after both fill.

  Run; expect a compile failure.
- [x] **Step 2: Implement.** `toolchain_row` reads `<target>/.rustc_info.json`, bounded at 1 MiB, takes `rustc_info_version`, compares it with `resolve_toolchain(source_checkout)?.version_text`, and checks one distinct `toolchain_value` over all units of all profiles. `fill` calls it for each target with `Owner::Owned { checkout }`, using that checkout, whether or not there are new keys, and adds the row to the per-target merge. Any `Err` is one finding naming the target. Rows of a `BTreeMap<String, u64>` give SEED-2.18 by construction.
- [x] **Step 3: Run.** `cargo test`; expect pass.
- [x] **Step 4: Commit.** `feat(seed): learn which toolchain value each rustc version text carries`

_Requirements: SEED-2.15, SEED-2.16, SEED-2.17, SEED-2.18_

---

### Task 9: Choose the units for one repo and one toolchain

**Files:**
- Create: `src/seed/choose.rs`
- Modify: `src/seed/mod.rs` (`pub mod choose;`)
- Test: `src/seed/choose.rs`

**Reuse:** none — new code (rung 7): no closure-drop exists, and this is a pure function of rows.

**Interfaces:**
- Consumes: `store::{StoreFile, Row}` (Task 6)
- Produces: `pub enum Choice { Units { rows: Vec<Row>, dropped: u64 }, NoRow { release: String } }`; `pub fn choose(store: &StoreFile, repo: &Path, version_text: &str) -> Choice`

**Depends-on:** Task 6

- [x] **Step 1: Write the failing tests** in `src/seed/choose.rs`:

```rust
fn row(name_meta: &str, fp: &str, value: u64, repo: &str, live: u64, deps: &[&str]) -> Row {
    Row {
        toolchain_value: value,
        name_meta: name_meta.into(),
        fingerprint_hash: fp.into(),
        kind: "lib".into(),
        rel_profile: "debug".into(),
        size: 1,
        filled_at: live,
        last_live_at: live,
        repos: vec![repo.into()],
        registry_deps: deps.iter().map(|d| d.to_string()).collect(),
    }
}

fn store(units: Vec<Row>) -> StoreFile {
    StoreFile { units, toolchains: [("V1".to_string(), 1)].into_iter().collect() }
}

fn names(choice: Choice) -> Vec<String> {
    match choice {
        Choice::Units { rows, .. } => {
            let mut v: Vec<_> = rows.into_iter().map(|r| r.fingerprint_hash).collect();
            v.sort();
            v
        }
        Choice::NoRow { .. } => panic!("no row"),
    }
}

#[test]
fn only_this_toolchain_and_this_repo() {
    let s = store(vec![
        row("a-1", "fa", 1, "/r", 5, &[]),
        row("b-1", "fb", 2, "/r", 5, &[]),
        row("c-1", "fc", 1, "/other", 5, &[]),
    ]);
    assert_eq!(names(choose(&s, Path::new("/r"), "V1")), ["fa"]);
}

#[test]
fn no_row_for_the_text_clones_nothing_and_substitutes_nothing() {
    let s = store(vec![row("a-1", "fa", 1, "/r", 5, &[])]);
    let text = "rustc 9\nrelease: 9.9.9\n";
    assert!(matches!(choose(&s, Path::new("/r"), text), Choice::NoRow { release } if release == "9.9.9"));
}

#[test]
fn newest_per_name_meta_and_ties_take_the_larger_fingerprint() {
    let s = store(vec![
        row("a-1", "f1", 1, "/r", 5, &[]),
        row("a-1", "f2", 1, "/r", 9, &[]),
        row("b-1", "f3", 1, "/r", 7, &[]),
        row("b-1", "f4", 1, "/r", 7, &[]),
    ]);
    assert_eq!(names(choose(&s, Path::new("/r"), "V1")), ["f2", "f4"]);
}

#[test]
fn a_dependent_of_a_fingerprint_not_chosen_is_dropped_and_the_drop_repeats() {
    let s = store(vec![
        row("a-1", "f1", 1, "/r", 5, &[]),
        row("a-1", "f2", 1, "/r", 9, &[]),
        row("b-1", "fb", 1, "/r", 5, &["f1"]),
        row("c-1", "fc", 1, "/r", 5, &["fb"]),
        row("d-1", "fd", 1, "/r", 5, &["f2"]),
    ]);
    match choose(&s, Path::new("/r"), "V1") {
        Choice::Units { rows, dropped } => {
            let mut got: Vec<_> = rows.iter().map(|r| r.fingerprint_hash.as_str()).collect();
            got.sort();
            assert_eq!(got, ["f2", "fd"]);
            assert_eq!(dropped, 2);
        }
        Choice::NoRow { .. } => panic!("no row"),
    }
}
```

A workspace dependency is simply absent from `registry_deps`, which the last case shows by `f2` having none. Run `cargo test --lib seed::choose`; expect a compile failure.
- [x] **Step 2: Implement** the pure function: look up `store.toolchains.get(version_text)`, else `NoRow` with the `release:` line of the text; filter by value and `repos.contains(repo)`; group by `name_meta` and take `max_by_key((last_live_at, fingerprint_hash))`; then loop: a set of chosen fingerprint hashes, retain rows whose every `registry_deps` entry is in the set, until no row leaves; `dropped` is the count removed by this loop.
- [x] **Step 3: Run.** `cargo test`; expect pass.
- [x] **Step 4: Commit.** `feat(seed): choose one consistent set of units for a repo and toolchain`

_Requirements: SEED-3.1, SEED-3.2, SEED-3.3, SEED-3.4, SEED-3.5, SEED-3.6, SEED-3.7_

---

### Task 10: Place units into a target that does not exist yet

**Files:**
- Create: `src/seed/place.rs`
- Modify: `src/seed/mod.rs` (`pub mod place;`)
- Test: `tests/seed_fill.rs`

**Reuse:** rung 2 — `cargo_target::resolve`, `Fs::clone_file`, `Fs::sharing`, `Fs::entry_stat`, `size::allocated_bytes` for the store's size, `liveness::unit_timestamp`, the window from `Config`

**Interfaces:**
- Consumes: `Fs::create_dir`, `Recording::fail_clone` (Task 3), `store::{Row, entry_dir, tag_path}` (Task 6)
- Produces: `pub struct SeededUnit { pub rel_profile: PathBuf, pub dir_name: String, pub stamp: Option<(PathBuf, u64)>, pub has_build: bool, pub deps_files: u32 }` (defined in `record.rs` from Task 13 on; defined here until then, and moved with a `pub use`); `pub struct PlaceReport { pub placed: Vec<SeededUnit>, pub cloned: u64, pub existing: u64, pub failed: u64, pub outside_window: u64, pub bytes_not_shared: Option<u64>, pub store_bytes: Option<u64>, pub target_identity: Option<(u64, u64)>, pub findings: Vec<(String, Option<PathBuf>, String)>, pub unmeasured: Vec<(String, String)>, pub cancelled: bool }`; `pub fn place(ctx: &Ctx, checkout: &Path, units: &[Row], window: Duration, dir_cap: usize, cancel: &(dyn Fn() -> bool + Sync)) -> Result<PlaceReport, Error>`

**Depends-on:** Task 3, Task 4, Task 6, Task 7

- [x] **Step 1: Write the failing tests** in `tests/seed_fill.rs`, each with a filled store from `RegistryRepo` and a new worktree:
  - `an_empty_set_creates_nothing`: no `target/` afterward.
  - `an_absent_target_gets_directories_tag_and_units_and_identify_accepts_it`: `cargo_target::identify(&target)` is `Some` before any build; files keep the store's mtimes.
  - `an_existing_path_gets_nothing_whatever_it_holds`, table: an empty directory, a file, a dangling symlink; contents unchanged; one finding naming the path.
  - `a_missing_parent_creates_nothing`, by a `.cargo/config.toml` with `build.target-dir` under an absent directory.
  - `a_root_with_no_cargo_toml_and_a_split_build_directory_clone_nothing`.
  - `only_create_dir_and_clone_write_into_the_target`: the `Recording` log holds no `write_file`, `hardlink`, `set_mtime`, `compress` or removal under the target.
  - `a_clone_finding_an_existing_file_leaves_it_and_counts_it`, with `plant_before_clone`; its content is unchanged.
  - `a_clone_failure_counts_the_unit_and_continues`, with `fail_clone(dst, libc::EIO)`.
  - `a_full_disk_is_an_error_not_a_finding`, with `fail_clone(dst, libc::ENOSPC)`: `Err(Error::DiskFull(_))`.
  - `a_cancel_stops_before_the_next_unit`, with a closure that stops after the first unit: `cancelled`, and fewer cloned than chosen.
  - `an_entry_at_its_directory_cap_clones_and_one_past_it_fails_that_unit`, with `dir_cap` 2 and 3.
  - `an_entry_path_with_a_parent_component_is_refused`, using a hand-written directory in the store: `..` cannot be planted in a real path, so place's path check is tested through `Row.rel_profile = "../escape"`.
  - `units_outside_the_window_are_counted`: backdate a store stamp.

  Run `cargo test --test seed_fill`; expect a compile failure.
- [x] **Step 2: Implement** (design decisions 8, 9). Return at once for an empty `units`. `cargo_target::resolve(checkout, cancel)`; an `Err` or a differing `build_directory` is one finding. `fs.create_dir(target_directory)`: `AlreadyExists` and `NotFound` are one finding each, and anything else is an `Error`. Clone the tag, then create inner directories per component with a set of made paths, tolerating `AlreadyExists`. For each row, poll `cancel`; walk `entry_dir(row)` with `CRATE_DIR_CAP`, group files by `deps/`, `build/`, `.fingerprint/`, and clone in that order. Each relative path must hold only `Component::Normal`; `rel_profile` too. `AlreadyExists` from `clone_file` adds `existing` and ends that unit; `ENOSPC` is `Error::DiskFull`; any other error adds `failed` with one finding and ends that unit. Record `SeededUnit`: the stamp through `liveness::unit_timestamp(profile, dir_name, kind)` and `Fs::mtime`, as nanoseconds; `has_build`; `deps_files`. `outside_window` counts units with no stamp or a stamp older than `window`; `seed_cmd` resolves it through `state::window_days(cfg.gc_window_days)` and a bad value is a `refusal` before anything runs, as in `plan_cmd`. `target_identity` is the `dev` and `ino` of `entry_stat(target_directory)`.
- [x] **Step 3: Run.** `cargo test`; expect pass.
- [x] **Step 4: Commit.** `feat(seed): place chosen units into a target that does not exist yet`

_Requirements: SEED-1.3, SEED-1.4, SEED-1.5, SEED-1.6, SEED-1.7, SEED-1.8, SEED-1.12, SEED-1.16, SEED-1.18, SEED-1.19, SEED-7.2, SEED-9.2, SEED-9.5, SEED-10.9_

---

### Task 11: Seed measures the bytes it added by what is really shared

**Files:**
- Modify: `src/seed/place.rs`
- Test: `tests/seed_fill.rs`

**Reuse:** rung 2 — `Fs::sharing`, `Fs::entry_stat`, `size::allocated_bytes`

**Interfaces:**
- Consumes: `PlaceReport` (Task 10), `Ctx.account_bytes`, `Recording::fail_sharing`
- Produces: `PlaceReport.bytes_not_shared`, `PlaceReport.store_bytes`

**Depends-on:** Task 10

- [x] **Step 1: Write the failing tests** in `tests/seed_fill.rs`:
  - `a_real_clone_of_a_compressed_entry_adds_no_unshared_bytes`: `bytes_not_shared == Some(0)`, with a real `Apfs`; every cloned file with `blocks > 0` reads `shares_all` and the store file's `clone_id`.
  - `a_file_that_does_not_share_is_counted_by_its_blocks_and_never_by_private_bytes`, overriding `sharing` for one named clone with `shares_all: false, private_bytes: 0`: `bytes_not_shared == Some(entry_stat(that clone).blocks * 512)`.
  - `a_different_clone_id_is_not_shared`, overriding `sharing` for one named clone.
  - `a_failed_sharing_read_is_unmeasured_not_zero`, with `fail_sharing`.
  - `with_accounting_off_the_number_is_unmeasured`, with `account_bytes: false`: `bytes_not_shared == None` and an `unmeasured` row.
  - `store_bytes_counts_allocated_blocks`: it is at least one compressed entry's blocks, above 0 while every store file's `private_bytes` is 0.

  Run; expect failure on the unset fields.
- [x] **Step 2: Implement.** After each cloned file, when `ctx.account_bytes`: `Fs::sharing` for the clone and the store file; shared only with `shares_all` and equal `clone_id`; otherwise add `entry_stat.blocks * 512`. `store_bytes` is `size::allocated_bytes(&store_dir, size::MAX_DEPTH)`; a walk error is an `unmeasured` row. Nothing reads `private_bytes`.
- [x] **Step 3: Run.** `cargo test`; expect pass.
- [x] **Step 4: Commit.** `feat(seed): count the bytes a seed adds by real sharing, and report the store's size`

_Requirements: SEED-4.1, SEED-4.2, SEED-4.3, SEED-4.4_

---

### Task 12: `cargo hive seed` runs end to end, and the first build recompiles nothing it cloned

**Files:**
- Modify: `src/seed/mod.rs` (`run`, `Outcome`, `Error`)
- Modify: `src/cli.rs` (`Command::Seed`, dispatch near line 148, `seed_cmd`)
- Modify: `tests/skill.rs` (`Parsed::Seed`, `parse`, line 423)
- Test: `tests/seed.rs`

**Reuse:** rung 2 — the shape of `plan_cmd` and `apply_cmd` (`src/cli.rs:1045`, `:941`), `plan_tier::Ctx` and `plan_tier::checkout_walk`, `doctor::require`, `ReceiptBuilder`, `emit`, `doctor::CANCEL`

**Interfaces:**
- Consumes: `source::owned_targets` (Task 5), `fill::fill` (Task 7), `store::read`, `store::tag_path` (Task 6), `choose::choose` (Task 9), `place::place` (Tasks 10 and 11), `Toolchain.version_text` (Task 1), `RegistryRepo`, `cargo_json` (Task 4)
- Produces: `Command::Seed { checkout: PathBuf, out: OutputArgs }`; `pub fn run(ctx: &Ctx, repo: &Repo, checkout: &Checkout, toolchain: &Toolchain, cancel: &(dyn Fn() -> bool + Sync)) -> Result<Outcome, Error>`; `pub struct Outcome { pub counts: BTreeMap<String, u64>, pub findings: Vec<(String, Option<PathBuf>, String)>, pub unmeasured: Vec<(String, String)>, pub notes: Vec<String>, pub cancelled: bool, pub placed: Vec<SeededUnit>, pub target: Option<(PathBuf, (u64, u64))> }`; `pub fn exit_code(result: &Result<Outcome, Error>) -> i32`; count names per design decision 13, where `clone_units_chosen` is counted before the drop, so `chosen = cloned + existing + failed + dropped`

**Depends-on:** Task 4, Task 5, Task 6, Task 7, Task 8, Task 9, Task 11

- [ ] **Step 1: Write the failing tests** in `tests/seed.rs`:

```rust
#[test]
fn the_first_build_of_a_seeded_worktree_recompiles_no_unit_it_cloned() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    cargo_json(&hive, &repo.main, &["check", "--all-targets"]);
    cargo_json(&hive, &repo.main, &["clippy", "--all-targets"]);
    cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    for (name, args) in [
        ("b", vec!["build"]),
        ("c", vec!["check", "--all-targets"]),
        ("l", vec!["clippy", "--all-targets"]),
        ("t", vec!["test", "--no-run"]),
        ("p", vec!["check", "-p", "app", "--lib"]),
    ] {
        let lane = repo.worktree(name);
        let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
        assert!(out.status.success(), "{receipt}");
        assert!(receipt["counts"]["clone_units_cloned"].as_u64().unwrap() > 0);
        let built = cargo_json(&hive, &lane, &args);
        assert!(built.registry_recompiled.is_empty(), "{name}: {:?}", built.registry_recompiled);
        assert!(!built.registry_fresh.is_empty(), "{name}");
    }
}
```

Add, all through the binary:
  - `a_second_lane_clones_without_filling_again`: `fill_entries_added` is 0 and `fill_entries_held` above 0.
  - `seed_fills_even_when_the_path_exists`: exit 0, `clone_units_cloned` 0, one finding, the store gains entries.
  - `a_protected_checkout_is_refused_and_nothing_filled`: exit 1 on `repo.main`.
  - `a_checkout_of_no_registered_repo_is_refused`: exit 1 for a clone below a secondary root with another root commit, and for a path that does not exist.
  - `a_disabled_capability_is_refused_and_nothing_filled`, table over `{"seeding": ["a"]}`, `{"store_compression": ["b"]}`, `{"deletion": ["d"]}`, using `hive.write_capabilities`.
  - `a_failing_toolchain_lookup_is_refused`: a `rust-toolchain.toml` naming an absent channel in the lane, with `RUSTUP_AUTO_INSTALL=0`.
  - `a_store_with_no_tag_clones_nothing_with_a_finding`, once the store's tag is deleted and every source skipped by a held lock.
  - `no_toolchain_row_clones_nothing_with_a_finding_and_exit_zero`.
  - `the_receipt_separates_the_fill_from_the_clone_and_holds_no_time`: every name of decision 13 present; no key contains `seconds`, `duration` or `elapsed`; `scope.checkouts` names the lane; `toolchain` is the lane's release; `reused_fresh` is absent.
  - `there_is_no_fill_command`: `cargo-hive fill` exits 2.
  - `compression_finishes_while_another_process_holds_the_state_lock`: `hive.hold_state_lock()`, spawn `seed` without waiting, poll `<home>/store/<toolchain_value>/` until an entry directory holds a compressed rlib while the child has not exited, kill the holder, `wait_within` the seed, assert exit 0 and that `store.db` lists the entry.
  - `a_signal_exits_130`: `hive.hold_state_lock()`, spawn `seed`, wait until `<home>/store` exists, by which time the handler is installed and the fill is blocked at its merge, send `SIGTERM`, `wait_within`, assert exit 130. The stop before the next clone is tested at `place` (Task 10).
  - `the_exit_code_follows_the_result`, a table on `seed::exit_code`: `Ok` is 0, `Ok` with `cancelled` is 130, `Err(DiskFull)` is 1, `Err(State)` is 1.
  - `a_full_disk_reaches_the_caller`: `seed::run` through `Recording` with `fail_clone(dst, libc::ENOSPC)` returns `Err(Error::DiskFull(_))`, and through `Recording` a store with no tag returns one finding and no clone.
  - `seed_runs_no_cargo_subcommand_but_metadata`: put a `cargo` shim first on `PATH` that logs its arguments and execs the real one; every logged line is `metadata --no-deps --format-version 1 --locked --offline`, in the manner of `tests/guards.rs:436`.
  - The other commands' receipts stay guarded by `tests/receipts.rs`, `tests/status_targets.rs`, `tests/gc.rs`, `tests/plan_apply.rs` and `tests/explain.rs`, which this task must leave unedited.

  Run `cargo test --test seed`; expect a compile failure.
- [ ] **Step 2: Implement `seed_cmd`.** Load registry, config and capabilities; resolve the window, a bad value being a `refusal` with exit 1; `doctor::install_signal_handlers`; canonicalize the argument. No path means SEED-1.11's refusal. Loop `reg.repos` and `discover::checkouts`, matching `row.path == real && !row.missing`. No match: `refusal`, exit 1. `Class::Protected`: `refusal`, exit 1. `resolve_toolchain(&real)`: an `Err` is a `refusal`, exit 1. `doctor::require` for `Seeding`, `StoreCompression` and `Deletion` with that release, each failure a `refusal`, exit 1. Build `Ctx` with `repos: std::slice::from_ref(repo)`, `mains: &registered_mains`, `account_bytes` from `EvictionAccounting`. `b.toolchain(Some(release))`; `scope_mut().checkouts.push(real)`. Call `seed::run`. Copy `Outcome` into the builder. The code comes from `seed::exit_code`. Add `Parsed::Seed` and its `parse` arm to `tests/skill.rs` in this commit.
- [ ] **Step 3: Implement `seed::run`:** `owned_targets`, its walk faults becoming findings; `fill`; if cancelled, return; `store::read`; no tag means one finding and no clone; `choose`; `NoRow` means one finding and no clone; `place`; assemble the counts of decision 13.
- [ ] **Step 4: Run.** `cargo test --test seed`, then `cargo test`; expect pass. If `the_first_build_...` fails for a command, stop and report it: this is the slice's core claim, and it reopens the owner's lock on the file set.
- [ ] **Step 5: Commit.** `feat(seed): cargo hive seed fills the store and clones into a new secondary checkout`

_Requirements: SEED-1.1, SEED-1.2, SEED-2.1, SEED-1.9, SEED-1.10, SEED-1.11, SEED-1.14, SEED-1.15, SEED-1.17, SEED-3.8, SEED-7.1, SEED-7.3, SEED-7.4, SEED-7.5, SEED-10.1, SEED-10.3, SEED-10.12_

---

### Task 13: Seed writes a record of what it cloned

**Files:**
- Create: `src/seed/record.rs`
- Modify: `src/seed/mod.rs` (`pub mod record;`; `run` writes the record)
- Modify: `src/seed/place.rs` (move `SeededUnit`)
- Test: `src/seed/record.rs`, `tests/seed.rs`

**Reuse:** rung 2 — `Tx::write_bounded`, `State::read_bounded`, the read-merge-write of `plan_tier/record.rs`

**Interfaces:**
- Consumes: `Outcome.placed`, `Outcome.target` (Task 12), `SeededUnit` (Task 10)
- Produces: `pub const SEEDS_FILE: &str = "seeds.json"`; `pub const MAX_SEEDS_BYTES: u64 = 16 * 1024 * 1024`; `pub struct SeedsFile { pub targets: BTreeMap<PathBuf, SeedRecord> }`; `pub struct SeedRecord { pub identity: (u64, u64), pub written_at: u64, pub units: Vec<SeededUnit> }`; `pub fn write(state: &State, target: &Path, record: SeedRecord, cancel: &(dyn Fn() -> bool + Sync)) -> Result<Option<String>, StateError>`, whose string is a note for the receipt; `pub fn read(state: &State) -> Result<SeedsFile, StateError>`

**Depends-on:** Task 12

- [ ] **Step 1: Write the failing tests:**
  - `src/seed/record.rs`: `a_write_drops_records_whose_target_is_gone`; `a_write_over_the_cap_drops_the_oldest_records_until_it_fits`, with a `write_capped` seam taking the cap; `the_newest_record_survives_the_cap`.
  - `tests/seed.rs`: `seed_writes_a_record_holding_each_cloned_unit_and_its_stamp`: `seeds.json` names the lane's target and a run-build-script unit's stamp under `build/<dir>/`; `seeds_receipt_holds_none_of_the_four_counts`; `a_cancelled_seed_records_what_it_cloned`, at the `seed::run` seam; `a_seed_that_cloned_nothing_writes_no_record`.

  Run; expect a compile failure.
- [ ] **Step 2: Implement** (design decision 4). `write` runs inside `locked_or_cancelled`: read with `read_bounded`, treat unreadable as empty with a note, insert, drop records whose path is gone, sort by `written_at`, and encode; while over the cap, drop the oldest; `write_bounded`. `seed::run` calls it after `place` on every path that placed anything, a cancel and `DiskFull` included. A failure is a finding and leaves the exit code 0, since `status` then reports nothing for the target, as SEED-6.8 allows.
- [ ] **Step 3: Run.** `cargo test`; expect pass.
- [ ] **Step 4: Commit.** `feat(seed): record what a seed cloned`

_Requirements: SEED-6.1, SEED-6.10_

---

### Task 14: `status` reports what became of the seeded units

**Files:**
- Modify: `src/seed/record.rs` (`counts`)
- Modify: `src/cli.rs` (`status`, `report_targets`, lines 499, 627, 654)
- Test: `src/seed/record.rs`, `tests/status_seed.rs`

**Reuse:** rung 2 — `State::read_bounded`, `TargetDir.profiles[].graph` already in memory in `status`, `fingerprint::le_hex`, `liveness::unit_timestamp`, the `bytes:<path>` count precedent (`src/cli.rs:609`); the walk is rung 7 (decision 15)

**Interfaces:**
- Consumes: `record::{read, SeedRecord, SeededUnit}` (Task 13), `TargetDir`, `Unit`, `fingerprint::le_hex`, `liveness::embedded_hashes` (Task 5)
- Produces: `pub struct SeedCounts { pub fresh: u64, pub unneeded: u64, pub recompiled: u64, pub missing: u64 }`; `pub fn counts(target: &TargetDir, record: &SeedRecord, deps_cap: usize) -> Option<SeedCounts>`; status counts `reused_fresh:<path>`, `seeded_unneeded:<path>`, `seeded_recompiled:<path>`, `seeded_missing:<path>`

**Depends-on:** Task 13

- [ ] **Step 1: Write the failing tests** on a hand-built `TargetDir` in `src/seed/record.rs`, table-driven, one row per bucket:
  - seeded, reached from a unit not in the record, same stamp: fresh;
  - seeded, reached only through another seeded unit that is itself reached: fresh;
  - seeded, not reached, same stamp: unneeded;
  - stamp differs: recompiled, and a start of the walk;
  - no stamp at seed, stamp now: recompiled;
  - `.fingerprint/<dir>` gone: missing; `build/<dir>` gone with `has_build`: missing; fewer `deps/` files than recorded: missing; a new `.rcgu.o` does not hide a missing file;
  - a not-current unit, absent from the graph, same stamp: unneeded;
  - every seeded unit lands in exactly one bucket: the four sum to `record.units.len()`;
  - identity differs: `None`.

  In `tests/status_seed.rs`, with `RegistryRepo`:

```rust
#[test]
fn after_the_first_build_status_reports_reused_units_and_no_recompiled_ones() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    cargo_json(&hive, &repo.main, &["check"]);
    let lane = repo.worktree("lane");
    hive.json(&["seed", lane.to_str().unwrap()]);
    cargo_json(&hive, &lane, &["build"]);
    let (_, status) = hive.json(&["status", repo.main.to_str().unwrap()]);
    let target = lane.join("target").canonicalize().unwrap();
    let key = |name: &str| format!("{name}:{}", target.display());
    assert!(status["counts"][key("reused_fresh")].as_u64().unwrap() > 0);
    assert_eq!(status["counts"][key("seeded_recompiled")], 0);
    assert_eq!(status["counts"][key("seeded_missing")], 0);
    assert!(status["counts"][key("seeded_unneeded")].as_u64().unwrap() > 0, "check-mode units");
}
```

Add: `a_deps_listing_over_its_cap_gives_no_counts`, with `deps_cap` 2; `status_for_a_target_with_no_record_is_unchanged`, comparing count names with the same fixture before seeding; `a_rebuilt_target_reports_no_seed_counts`, deleting `target/` and building; `an_unreadable_record_reports_no_counts_and_one_note`; `the_counts_are_as_of_each_status`: run `status` twice around a second build touching one seeded unit. Run; expect a compile failure.
- [ ] **Step 2: Implement** (design decision 15). `counts`: read `identity` from `std::fs::symlink_metadata(target.path)`, and return `None` on a mismatch. Index `hash_hex` over all profiles. Read each recorded stamp's modification time. List each profile's `deps/` once, bounded with `deps_cap`, `DEPS_CAP` in production, counting names per metadata hash, `fingerprint::unit_hash(dir_name)`, through `embedded_hashes`, leaving out `.rcgu.o`. Starts: graph units not in the record, and recorded units whose stamp differs. Walk `deps` over the index with `le_hex`. Then bucket each recorded unit in the order missing, recompiled, fresh or unneeded. In `status`: `record::read` through `read_bounded` once, an `Err` becoming one note and no counts; pass the file to `report_targets`, which adds the four `count`s when `counts` returns `Some`.
- [ ] **Step 3: Run.** `cargo test`; expect pass, `tests/status_targets.rs` and `tests/receipts.rs` unchanged.
- [ ] **Step 4: Commit.** `feat(status): report what became of a target's seeded units`

_Requirements: SEED-6.2, SEED-6.3, SEED-6.4, SEED-6.5, SEED-6.6, SEED-6.8, SEED-6.9, SEED-6.11, SEED-6.12, SEED-10.2_

---

### Task 15: GC still treats a seeded target as it treats any other

**Files:**
- Test: `tests/seed.rs`

**Reuse:** none, test only — `RegistryRepo`, `cargo_json` (Task 4)

**Interfaces:**
- Consumes: `RegistryRepo`, `cargo_json` (Task 4), the `seed` command (Task 12)
- Produces: nothing

**Depends-on:** Task 2, Task 12

- [ ] **Step 1: Write the tests** in `tests/seed.rs`. These guard existing behavior, so they are expected to pass at once; a failure is a real defect, reported, not a test to adjust.
  - `before_the_first_build_a_plan_lists_old_seeds_and_apply_skips_the_target`: seed, backdate every stamp 30 days, `gc --plan` lists paths, `apply` exits 0 with outcome `partial` and reason `all_targets_skipped`, the file count is unchanged, and the first build recompiles no registry unit. Measured once by hand on 2026-11-02 (`.skills/SEED/decisions.md`).
  - `a_plan_over_a_seed_does_not_claim_shared_bytes_as_freed`: the plan's `freed_bytes` is 0 and its `deleted_bytes_still_shared` above 0. Another value is a finding, not a threshold to tune.
  - `after_the_first_build_liveness_keeps_every_unit_that_build_needed`: `gc --plan` lists no unit the build used.
  - `liveness_reports_a_seed_with_fresh_stamps_as_live_before_its_first_build`: `gc --plan` gives no plan.
  - `gc_still_lists_rcgu_objects_of_a_dead_unit`, on a source with a dead unit: the stored plan in `plans.json` lists a `.rcgu.o`, read the way `Planned::plans_json` reads it, since the receipt carries only a path count.
- [ ] **Step 2: Run.** `cargo test --test seed`; expect pass. Any failure goes to `root-cause`.
- [ ] **Step 3: Commit.** `test(seed): gc treats a seeded target as it treats any other`

_Requirements: SEED-10.4, SEED-10.6, SEED-10.15_

---

### Task 16: Teach agents to seed

**Files:**
- Modify: `plugin/skills/hive/SKILL.md`
- Modify: `tests/skill.rs` (`want` list, near line 574; path-word rule)
- Modify: `skills/hive/eval.json`, `skills/hive/TESTS.md`
- Modify: `README.md`

**Reuse:** rung 2 — the seven existing headings and the existing test machinery

**Interfaces:**
- Consumes: `Command::Seed` (Task 12), the status counts (Task 14)
- Produces: one `cargo hive seed /path/to/checkout --json` line in the skill

**Depends-on:** Task 12, Task 14

- [ ] **Step 1: Write the failing test.** Add `Parsed::Seed` to `want` in `skill_documents_every_shipped_command`, and `Parsed::Seed` to the arm that requires a path word. Run `cargo test --test skill`; expect the documented-command test to fail.
- [ ] **Step 2: Edit the skill,** inside its seven headings and under the 150-line budget: a `Which command` row; a `Command lines` line; `seed` added to the `Version first` bullet of commands not to run on a mismatch; `Reading a receipt`: the fill and clone counts, and the four `status` counts, `reused_fresh` meaning seeded, reached and unchanged, and not Cargo's printed fresh count; `Do not`: do not seed a checkout that has a target, since it gets nothing, and seed one lane first when starting several. No `.md` text and no stray `cargo hive` outside a command line, as the tests require. Add a `README.md` command line and the note that the store only grows until store GC ships.
- [ ] **Step 3: Record one eval.** Run the skill once on a real prompt, record it as a `## ` section in `skills/hive/TESTS.md`, then add the matching `eval.json` entry with `derived_from` naming that heading. Update any `eval.json` contract quote whose skill text moved.
- [ ] **Step 4: Run.** `cargo test --test skill`, then `cargo test`; expect pass.
- [ ] **Step 5: Commit.** `docs(skill): teach agents to seed a new lane`

_Requirements: SEED-8.1, SEED-8.2, SEED-8.3, SEED-10.13_

---

### Task 17: The klynt run

**Files:**
- Modify: `tests/seed.rs` (an `#[ignore]` test)
- Create: `docs/specs/2026-11-02-store-seed/klynt-run.md`
- Modify: `docs/standards/testing.md`

**Reuse:** rung 2 — the shape of the ignored tests in `tests/guards.rs:328`

**Interfaces:**
- Consumes: the `seed` and `status` commands
- Produces: a dated record

**Depends-on:** Task 15, Task 16

- [ ] **Step 1: Stop and ask the owner to approve a testing-standard exception.** `docs/standards/testing.md` says tests never write under the real `~/.cargo` or any repo outside their scratch dir, and this run does: `git worktree add` writes `.git/worktrees/` in klynt, and the build runs klynt's build scripts. Proposed terms, in the shape of the two approved exceptions: ignored by default; the checkout named by `HIVE_SEED_CHECKOUT`, with no default; the test creates and removes its own worktree; `CARGO_TARGET_DIR` unset; `HIVE_HOME` a scratch; `--offline --locked`. Without the approval, stop here. With it, write the exception into `docs/standards/testing.md` and update its hash in this plan.
- [ ] **Step 1b: Write the ignored test** `seeding_a_new_worktree_of_a_real_repo_recompiles_no_cloned_unit`: it reads `HIVE_SEED_CHECKOUT`, needs a built source target there, uses a scratch `HIVE_HOME`, makes a new worktree, seeds it, asserts `clone_bytes_not_shared == 0`, runs `cargo build --offline --message-format=json-render-diagnostics` to a file, asserts no unit it cloned recompiled, then removes the worktree and the scratch home.
- [ ] **Step 2: Run it once on klynt, with the owner's agreement, since it takes real time and disk,** and write `klynt-run.md`: date, commit, the seed receipt's counts, Cargo's compiled counts, the four `status` counts, and, beside them, the comparison with spike 7's 466 needed and 31 compiled. Neither that comparison nor Cargo's printed fresh count is an expected value for the four counts. If a lock's reopen trigger fired (`.skills/SEED/decisions.md`), say so and stop for the owner.
- [ ] **Step 3: Check the dependency guard.** `git diff main -- Cargo.toml` shows no new dependency.
- [ ] **Step 4: Commit.** `docs(SEED): record the klynt seed run`

_Requirements: SEED-9.6, SEED-10.14_
