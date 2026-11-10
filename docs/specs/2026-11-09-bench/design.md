# Design: Bench (measured cold-versus-seeded first builds)

Feature code: BENCH
Status: Approved
Approved: 2026-11-09 14:58 by the operator (Jayden), "approve" on the spec sheet for #6 at feat/bench 0be0657, relayed by the conductor (/Users/jayden/Developer/experiments/cargo-zip/.skills/_pending-bench/auto/operator-reply.md). The reply approves the sheet as a whole; the Open Questions in requirements.md stay open as listed, and the owner-reserved sampling pick (S1) stays labelled as such.
Date: 2026-11-09
Requirements: ./requirements.md

Sources: the scan digest `/Users/jayden/Developer/experiments/cargo-zip/.skills/_pending-bench/auto/design-scan.md`
(the "digest" below), the independent review `.../auto/design-review.md` (findings R1–R11, all applied), the
close package `.../auto/close-package.md` r2, and the ledger `.../auto/ledger.md`. `[Weigh Wn]` and
`[Proposed DFn]` mean what they mean in requirements.md ("How labels read").

## Context

hive already has every piece a seeded sample needs, but they are welded together. `seed_cmd`
(src/cli.rs:1403-1551) inlines the whole locate-and-gate sequence: load state, check `gc_window_days`,
install signal handlers, canonicalize, match the checkout to a registered repo through
`discover::checkouts`, refuse a protected checkout, resolve the toolchain, require Seeding,
StoreCompression and Deletion, build a `Ctx`, call `seed::run`, then emit. `seed::run`
(src/seed/mod.rs:179-261) runs the owned-target walk and fill (:186-199), then the tag check, store read,
choice, clones and record (:200-248), as one function with no timing seam. The four seed counts come only
from `record::counts` (src/seed/record.rs:86), which `status` calls per target (src/cli.rs:739-757). The
child runner `cargo_target::output_with_deadline` (src/cargo_target.rs:259-337) pipes both streams into
unbounded buffers and polls every 20 ms. Git calls (src/git.rs:52-82) carry no deadline and no
`--no-optional-locks`. Receipts are integer-only (src/receipt.rs:14-19).

The binding constraint is that **hive itself writes no repo file and creates or removes no checkout or
target** (ARCH-2, ARCH-18, BENCH-10.1), while **every reported second comes from bench** (ARCH-10).
Together these rule out the obvious alternative, a bench that clones or adds worktrees itself and cleans up
after, as the hyperfine reference does with a prepare and a cleanup command (close package D2,
approaches.md). They also rule out timing through `output_with_deadline`: its buffers are unbounded, which
`docs/standards/code.md:14` forbids for a 60-minute build, and its end time is only seen at the next 20 ms
poll.

So bench is a new orchestration module that **reuses** the seed steps (split, not copied), the status
counter, the receipt builder and the target-variable list. It adds only what nothing has: a bounded child
runner, two read-only git readers, a load-average reading, sample order and integer statistics.

## Decisions

1. **A new `src/bench/` module owns the run; `cli.rs` only adapts.** `bench_cmd` checks `gc_window_days`,
   installs the signal handlers, and emits, in the same order `seed_cmd` does (src/cli.rs:1420-1431), so a
   bad config refuses before anything runs (R3).
2. **Seed is split, not copied.** `seed::run_fill` returns `Fill::Stopped(Outcome)` exactly where `run`
   returns early on cancel today (src/seed/mod.rs:191, :195-199). Otherwise it returns `Fill::Continue(Filled)`,
   and `run_clone` carries on from :200. `seed::run` is `run_fill` then `run_clone`, with its signature and
   `Outcome` unchanged (R4).
3. **The per-checkout locate-and-gate block of `seed_cmd` is extracted** into `cli::locate_checkout`. It
   covers canonicalize, the repo match, the protected refusal, the toolchain and the capabilities
   (src/cli.rs:1433-1506). It returns a `Refusal { kind, path, detail, code }` that keeps seed's two
   finding paths: the argument as given on a canonicalize failure (:1436-1442), the real path elsewhere.
   Seed's findings, order and exit codes stay byte-identical (BENCH-9.1, R2).
4. **Bench's own children use a new bounded runner** in `cargo_target`. It captures stdout and stderr each
   to a capped tail (`Capture`), gives the child its own process group, and stamps `exited_after` in the
   waiter thread right after `wait()` returns, measured from an `Instant` taken just before spawn. Reader
   threads get a 1 s grace after exit and are then detached, so a grandchild holding a pipe open cannot
   delay the result (R1).
5. **`resolve` is untouched** (BENCH-9.6). Bench's env map reads the existing `TARGET_VARS` list
   (src/cargo_target.rs:15-19) and applies its own `GIT_` prefix filter. There is no scrub extraction (R5).
6. **One env map per run is built once and given with `env_clear()` + `envs()` to every timed build**, so the
   child sees exactly the map (BENCH-5.1, 5.2). The prime and seed keep their own scrubs.
7. **[Weigh W18] The toolchain is pinned and a caller-set `RUSTC` or `CARGO_BUILD_RUSTC` is overridden, not
   refused.** The map removes both and always sets `RUSTC=<resolved rustc>`, plus `RUSTUP_TOOLCHAIN=<name>`
   when rustup resolved one, and `RUSTUP_AUTO_INSTALL=0`. That is the pair doctor's probe sets
   (src/doctor.rs:699-719). Without rustup, `resolve_toolchain` falls back to bare `cargo` and `rustc`
   (src/doctor.rs:459-460), so the pin is the same bare `rustc` on `PATH` that seed resolved, not an absolute
   path. BENCH-5.4's finding still names the caller's
   value. Why: seed clones only units for the toolchain `resolve_toolchain` found (SEED decision 6,
   MAIN .skills/SEED/decisions.md:11), so a different `RUSTC` would make every seeded unit recompile and
   measure a mismatch. This is the wrapper decision's shape (W6): override and report. It answers the Open
   Question, and it amends BENCH-5.2 and 5.4 (see "Upstream sync").
8. **Git reads for bench go through a new read-only builder**: `git --no-optional-locks -c core.fsmonitor=false
   -C <dir>`, GIT_* removed, run through the bounded runner with a 60 s deadline and a 64 KiB stdout cap.
   `Git::command` and `Git::run` are not touched (BENCH-9.5). The review confirmed in a scratch repo that
   `--no-optional-locks status` leaves the index bytes unchanged and plain `status` rewrites them.
9. **Load average is an `Fs` method with a default body** (`Err(Unsupported)`), overridden on `Apfs` with
   `libc::getloadavg` under `// SAFETY:`, and forwarded by the test wrapper `Recording` so in-process tests
   see a real value (R9). A failed reading becomes an `unmeasured` entry, never a refusal (BENCH-1.6).
10. **The timed-build deadline is a parameter of `bench::run`** (`Limits`). `bench_cmd` passes the fixed
    60 minutes [Weigh W15]; tests pass a short one (BENCH-10.2). No flag.
11. **Seeded timings grow with position, and that is reported, not corrected** (R10). `run_fill` walks every
    checkout and runs `cargo metadata` per checkout, and `place` runs it again, so `fill_ms` and `seed_ms`
    rise through the run as finished samples add targets. ABBA spreads it across both arms, and
    `fill_targets_read` per sample shows it (card 3, W4).
12. **No new dependency** (`libc` is already in Cargo.toml) and **no ADR.** The D3 reading of ARCH-2 is
    surprising and a trade-off, but it is not hard to reverse, because it binds only `src/bench/`. So it
    stays in the close package for the operator to confirm. Every new module opens with a `//!` comment
    (`docs/standards/code.md`).

Spine invariants this feature relies on: ARCH-1, ARCH-2 (under D3 [Weigh W3]), ARCH-4, ARCH-5, ARCH-6,
ARCH-7, ARCH-8, ARCH-10, ARCH-13, ARCH-14, ARCH-15, ARCH-16, ARCH-17, ARCH-18, ARCH-19.

## Architecture

Vocabulary for every section below: **module**, **interface**, **implementation**,
**seam** (public surface where behavior is observable and substitutable). Prefer
deep modules — an interface much simpler than what it hides.

### 1. `bench::run`: the run orchestrator (src/bench/mod.rs, new)

Satisfies: BENCH-1.1, BENCH-1.3, BENCH-1.4, BENCH-1.5, BENCH-1.6, BENCH-1.7, BENCH-1.8, BENCH-1.10, BENCH-1.12, BENCH-4.1, BENCH-4.2, BENCH-4.3, BENCH-6.1, BENCH-6.2, BENCH-6.3, BENCH-6.4, BENCH-7.1, BENCH-7.2, BENCH-7.3, BENCH-9.7, BENCH-10.1, BENCH-10.2
Reuse: rung 2 — `seed::run_fill` / `seed::run_clone` (section 6), `record::read` + `record::counts` with
`gc::DEPS_CAP` (src/seed/record.rs:74,86; src/gc/run.rs:22), `seed::store::read` (src/seed/store.rs:130), `discover::targets` with an in-memory
`Snapshot::default()` (src/discover.rs:523, as src/seed/source.rs:86 does), the bounded runner (section 7),
`ReceiptBuilder` (src/receipt.rs:140-215). The sequencing itself is new code (rung 7), because nothing
sequences builds and seeds.
Respects: ARCH-2, ARCH-5, ARCH-6, ARCH-10, ARCH-16, ARCH-17, ARCH-18, ARCH-19
Interface:
```rust
pub struct Plan { pub repo: Repo, pub cold: Vec<Checkout>, pub seeded: Vec<Checkout>,
                  pub toolchain: Toolchain, pub build: BuildCmd, pub env: EnvMap }
pub struct Limits { pub build_deadline: Duration }          // BUILD_DEADLINE = 60 min in bench_cmd
pub fn run(ctx: &Ctx<'_>, plan: &Plan, limits: Limits,
           cancel: &(dyn Fn() -> bool + Sync)) -> Report
pub enum End { Finished, Failed, Cancelled }
pub struct Report { pub end: End, pub counts: BTreeMap<String, u64>,
                    pub findings: Vec<receipt::Finding>, pub unmeasured: Vec<(String, String)> }
impl Report { pub fn write_into(&self, b: &mut ReceiptBuilder);
              pub fn exit_code(&self) -> i32 }  // Finished 0, Failed 1, Cancelled 130
```
Depth: if this module vanished, callers would still need only `Plan → Report`, that is, "time these
checkouts in this order and give me rows and an end". The prime, the order, the clock reads, the seed split,
the post-build counts, and the failure and cancel rules all live behind it.
Locality: lands in the new `src/bench/`; `src/seed/mod.rs` is `extract` (section 6); `status` stays as it
is (its counter is called, not changed).

Flow of one run:
```
prime:   t0 → seed::run_fill(ctx, repo)
           Stopped(o) → Cancelled · Err(e) → Failed (bench.failed "prime: …")
           Continue(f) → t1; counts prime_ms, prime_entries_added, prime_targets_read; f.findings copied
         seed::store::read: no Row whose `repos` holds repo.main → bench.nothing_to_seed, Failed   (7.3)
for (arm, i) in order::abba(n):                                        (section 2)
  if cancel() → Cancelled
  load_before ← fs.load_average_hundredths()  (Err → unmeasured sample.<pos>.load_before)
  seeded: t0 → run_fill → t1 → run_clone(f) → t2
     Stopped(o) / o.cancelled → Cancelled
     Err(DiskFull{kept}) → kept findings copied, Failed · Err(other) → Failed
     fill_ms=ms(t1-t0), clone_ms=ms(t2-t1), seed_ms=fill_ms+clone_ms; seed findings copied (kind kept, path = checkout)
  run_bounded(build cmd, limits.build_deadline) → Ran{exited_after, status, stderr_tail}
     Cancelled → Cancelled · TimedOut → Failed (names the deadline) · status≠0 → Failed (exit code, tail)
     build_ms = exited_after
  load_after ← fs.load_average_hundredths()
  seeded: record::read + discover::targets([row]) + record::counts → four counts, or unmeasured (4.3)
  finding bench.sample {arm, position, build_ms, load_before_x100, load_after_x100,
                        [seed_ms, fill_ms, clone_ms, fill_targets_read, seeded_fresh,
                         seeded_unneeded, seeded_recompiled, seeded_missing]}
Cancelled → unmeasured sample.<pos> "cancelled" for each sample not finished      (6.2)
Failed    → unmeasured sample.<pos> "not run: an earlier step failed"              (7.1)
Finished  → counts <series>_{median,p90,min,max,spread} for cold_build_ms,
            seeded_build_ms, seeded_seed_ms, seeded_total_ms                         (1.8; none otherwise, 6.3)
```
Seed and fill times are `Instant` pairs. Build time is the runner's `exited_after`. Both are converted with
`u64::try_from(d.as_millis())` (BENCH-1.10). Bench writes no state file of its own. Its only state writes
are the ones `run_fill`/`run_clone` already make (the store and `seeds.json`) and the receipt line. The
review checked every write path in this flow and found no other: `discover::targets` with
`Snapshot::default()`, `record::read` and `record::counts` write nothing (BENCH-9.7). Bench removes nothing
(BENCH-6.4, 10.1). Cancel works because `CANCEL` is set by the handlers `bench_cmd` installs, the seed halves
take `cancel`, and the runner kills the group (BENCH-6.1). The deadline comes from `limits` (BENCH-7.2,
10.2).

### 2. `bench::order` and `bench::stats`: pure sample order and integer statistics (src/bench/order.rs, src/bench/stats.rs, new)

Satisfies: BENCH-1.2, BENCH-1.9
Reuse: none — new code (rung 7). The repo has no statistics helpers (digest §12), and std has no median.
Interface:
```rust
pub enum Arm { Cold, Seeded }
pub fn abba(n: usize) -> Vec<(Arm, usize)>   // cold_0, seeded_0, seeded_1, cold_1, cold_2, seeded_2, …
pub struct Summary { pub median: u64, pub p90: u64, pub min: u64, pub max: u64, pub spread: u64 }
pub fn summarize(values: &[u64]) -> Option<Summary>   // None on empty
```
Depth: callers know "an order for n" and "five integers for a series". The rank arithmetic (ceil(9n/10) in
integers, the floor of the mean at even n) stays inside.
Locality: new files only; no neighbor impact.

### 3. `bench::check`: refusals before anything builds (src/bench/check.rs, new; uses `cli::locate_checkout`)

Satisfies: BENCH-2.1, BENCH-2.2, BENCH-2.3, BENCH-2.4, BENCH-2.5, BENCH-2.6, BENCH-2.7, BENCH-2.8, BENCH-2.9, BENCH-2.11, BENCH-2.12
Reuse: rung 2 — `cli::locate_checkout` (section 6) for 2.1, 2.7 and the toolchain half of 2.8;
`cargo_target::resolve` (src/cargo_target.rs:339) for the target and build directories in 2.5 and 2.11;
the git readers (section 8) for 2.3 and 2.4. The cross-checkout rules (same repo, same HEAD, same toolchain
text, arm sizes, duplicates, a shared target) are new code (rung 7).
Respects: ARCH-4, ARCH-5, ARCH-7, ARCH-8, ARCH-14
Interface:
```rust
pub struct Unfair { pub path: Option<PathBuf>, pub reason: String }
pub fn check(reg: &Registry, cfg: &Config, caps: &Capabilities, cold: &[PathBuf], seeded: &[PathBuf],
             cancel: &(dyn Fn() -> bool + Sync)) -> Result<Checked /* repo, cold, seeded, toolchain */, Vec<Unfair>>
```
Depth: callers know "paths in, a fair plan or a list of reasons out". The order of checks, the repo match
and the HEAD, dirty and target probes are hidden.
Locality: new file; `cli.rs` is `extract` (section 6).

Every checkout is checked, and every failing one gets its own refusal (BENCH-2.9). Arm sizes and duplicates
(2.6) come first, from the arguments alone. Then each path is located (2.1, 2.7). Then come the
cross-checks: one repo `main` (2.2), one HEAD (2.3), no tracked changes (2.4), and one toolchain by its
whole `version_text`, which is what seed's `choose` matches on (src/seed/mod.rs:221) (2.8). Last, `resolve`
runs per checkout. Its target directory must not exist and must equal its build directory, else
`target.split`, as seed refuses at src/seed/place.rs:206 (2.5). No two checkouts may resolve the same target
directory (2.11). The workspace root `resolve` returns must hold a `Cargo.lock` (2.12).

### 4. `bench::command`: the timed build and its environment (src/bench/command.rs, new)

Satisfies: BENCH-3.1, BENCH-3.2, BENCH-3.4, BENCH-5.1, BENCH-5.2, BENCH-5.3, BENCH-5.4
Reuse: rung 2 — `cargo_target::TARGET_VARS` (src/cargo_target.rs:15-19) for the ARCH-13 floor, `Toolchain`
(src/doctor.rs:133) for the pin, and doctor's `RUSTC` + `RUSTUP_TOOLCHAIN` pin pattern
(src/doctor.rs:699-719). The build shapes and wrapper rules are new (rung 7).
Respects: ARCH-1, ARCH-13
Interface:
```rust
pub enum BuildCmd { Build, CheckAll, ClippyAll, TestNoRun, CheckLib(String) }  // clap value parser
impl BuildCmd { pub fn args(&self) -> Vec<String> }   // always ends "--offline", "--locked"
pub struct EnvMap { vars: BTreeMap<OsString, OsString>, overridden: Vec<(&'static str, String, String)> }
pub fn env_map(caller: impl IntoIterator<Item = (OsString, OsString)>, tc: &Toolchain) -> EnvMap
impl EnvMap { pub fn vars(&self) -> &BTreeMap<OsString, OsString>;
              pub fn command(&self, tc: &Toolchain, dir: &Path, b: &BuildCmd) -> Command;
              pub fn findings(&self) -> Vec<(&'static str, String)> }
```
Depth: callers know "a build shape plus the caller's env gives a Command and a list of what was
overridden". The variable list, the empty-string overrides and the toolchain pin are hidden.
Locality: new file; `cargo_target.rs` stays as it is, because `TARGET_VARS` is already `pub`
(src/cargo_target.rs:15).

`env_map` takes the caller's variables and removes `TARGET_VARS`, every `GIT_*`, `CARGO_PKG_*`,
`CARGO_MANIFEST_DIR`, `OUT_DIR`, `CARGO_BUILD_RUSTC_WRAPPER`, `CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER`, and
[Weigh W18] `RUSTC`, `CARGO_BUILD_RUSTC`, `RUSTUP_TOOLCHAIN` and `RUSTUP_TOOLCHAIN_SOURCE`. It sets
`RUSTC_WRAPPER=""`, `RUSTC_WORKSPACE_WRAPPER=""`, `RUSTUP_AUTO_INSTALL=0`, `RUSTC=tc.rustc`, and
`RUSTUP_TOOLCHAIN=tc.name` when `Some` (decision 7). Each wrapper variable the caller set non-empty becomes
an `env.wrapper.overridden` finding (BENCH-5.3), and a caller `RUSTC` or `CARGO_BUILD_RUSTC` becomes an
`env.rustc.overridden` finding (BENCH-5.4). `command` runs `tc.cargo` with `env_clear()` and `envs(vars)`,
`current_dir(checkout)`, and the shape's args. The exact line is reported as a `bench.command` finding with
`measured.command` text (BENCH-3.4). The default is `Build` (BENCH-3.1). `CheckLib`'s package must match
`[A-Za-z0-9_-]+`, so a value starting with `-` cannot become a Cargo flag. Anything else fails clap
parsing (BENCH-3.3, section 5).

### 5. `cli::bench_cmd`: the subcommand adapter (src/cli.rs)

Satisfies: BENCH-1.11, BENCH-3.3
Reuse: rung 2 — the `seed_cmd` shape: the `window_days` check and `install_signal_handlers`
(src/cli.rs:1420-1431), `platform::current`, `Ctx`, `Receipt::new` and `emit` (src/cli.rs:222); clap derive
for the arguments.
Respects: ARCH-8, ARCH-10
Surface:
- `Command` enum (src/cli.rs:23-94) → `tests/skill.rs` `parse()` exhaustive match (:424-452): **replace**,
  adding `Parsed::Bench` and its arm in this change.
- `main_with` dispatch (src/cli.rs:143-176): **replace**, with a new arm.
- Persisted receipts in `~/.hive/receipts/*.jsonl`, read by people and agents: **frozen**. The design builds
  around the contract: bench writes a schema-2 line with existing fields only, and no existing line changes.
Interface: `cargo hive bench --cold <checkout>... --seeded <checkout>... [--build-cmd <shape>] [--json]`.
Exit codes come from `Report::exit_code` (section 1), 1 for a refusal, and 2 for a clap usage error,
including an unknown `--build-cmd` value (BENCH-3.3).
Depth: n/a — extends `cli` with one adapter of the existing pattern.
Locality: `extend` `cli.rs` with one variant, one arm and one function.

`bench_cmd` loads state, checks `window_days`, installs signal handlers, then calls `bench::check`. On
refusal it emits the refusal findings with exit 1. Otherwise it builds the `Plan`, the `Ctx` (with
`repos = [repo]`) and `Limits { build_deadline: BUILD_DEADLINE }`, calls `bench::run`, writes the report
into `Receipt::new("bench", scope)` (schema 2, BENCH-1.11) with the env findings and the command line, and
emits. The human line is one sentence: the cold and seeded-total medians, and n. Every key bench adds ends
in `_ms`, `_x100` or is a plain count name, so none contains `seconds`, `duration`, `elapsed`, `saved` or
`estimate` (BENCH-1.11).

### 6. Seed split and `locate_checkout` extraction (src/seed/mod.rs, src/cli.rs)

Satisfies: BENCH-9.1, BENCH-9.2
Reuse: rung 2 — extract from the existing `seed::run` (src/seed/mod.rs:179-261) and `seed_cmd`
(src/cli.rs:1433-1506).
Respects: ARCH-5, ARCH-19
Surface:
- `seed::run` callers: `seed_cmd` (src/cli.rs:1523) and `tests/seed.rs:1933` (in process): **frozen**. The
  design builds around the contract: `run` keeps its signature and `Outcome`, composing `run_fill` then
  `run_clone` with the same early returns.
- `seed_cmd`'s inline locate-and-gate block (src/cli.rs:1433-1506): **replace**, now calling
  `locate_checkout`. The `window_days` check and signal install (:1420-1431) stay inline before it. The
  refusal findings (kind, path, detail), their order and the exit codes are byte-identical, and
  tests/seed.rs's refusal tests check that.
- `record::counts` and `status`'s count reporting (src/cli.rs:739-757): **frozen**. Bench calls `counts` as
  `status` does, with no change.
Interface:
```rust
pub enum Fill { Continue(Filled), Stopped(Outcome) }   // Stopped = today's early cancel returns
pub struct Filled { report: FillReport, findings: Vec<(String, Option<PathBuf>, String)> }
pub fn run_fill(ctx, repo, cancel) -> Result<Fill, Error>                                  // :186-199
pub fn run_clone(ctx, repo, checkout, toolchain, filled: Filled, cancel) -> Result<Outcome, Error> // :200-260
impl Filled { pub fn report(&self) -> &FillReport; pub fn findings(&self) -> &[…] }
pub(crate) struct Refusal { pub kind: &'static str, pub path: Option<PathBuf>, pub detail: String, pub code: i32 }
pub(crate) fn locate_checkout(reg: &Registry, cfg: &Config, caps: &Capabilities, path: &Path)
    -> Result<(Repo, Checkout, Toolchain), Refusal>     // registered_mains computed inside, as :1445
```
Depth: n/a — extends `seed` and `cli`.
Locality: `extract` within `src/seed/mod.rs` and `src/cli.rs`; no other module changes.

### 7. Bounded child runner (src/cargo_target.rs)

Satisfies: BENCH-9.6
Reuse: rung 2 — `ChildOutputError` (src/cargo_target.rs:214) and the group-kill call inside `kill_group`
(:220), which moves into `kill_pgid(pid)` so the waiter thread can own the `Child`; rung 3 — `std::process`,
`std::sync::mpsc`, `std::time::Instant`. The waiter-thread runner itself is new (rung 7).
Respects: ARCH-13
Surface:
- `resolve` (src/cargo_target.rs:339-397) and its readers (`discover::targets`, src/seed/place.rs:193):
  **frozen**. The design builds around the contract and does not edit `resolve`.
- `kill_group(&Child)` (:220), read by `output_with_deadline` (:259): **replace**. Its body calls the new
  `kill_pgid(child.id())`, and the behavior is identical.
- `output_with_deadline`: **frozen**. Bench does not use it.
Interface:
```rust
pub struct Capture { pub stdout_cap: usize, pub stderr_cap: usize }   // 0 = Stdio::null()
#[derive(Debug)] pub struct Ran { pub status: ExitStatus, pub exited_after: Duration,
                 pub stdout: Vec<u8>, pub stderr_tail: Vec<u8> }        // each ≤ its cap
pub fn run_bounded(cmd: &mut Command, deadline: Duration, capture: Capture,
                   cancel: impl FnMut() -> bool) -> Result<Ran, ChildOutputError>
```
Depth: if `run_bounded` vanished, callers would need "run this to exit within a deadline, killable,
keeping capped output, and say when it exited". The waiter thread, the tail rings, the reader grace and the
group kill are hidden.
Locality: `extend` `cargo_target.rs` with `run_bounded`, `Capture`, `Ran`, `kill_pgid`; the rest stays as is.

The timed build uses `Capture { stdout_cap: 0, stderr_cap: 64 KiB }`. `exited_after` runs from just before
spawn to the moment `wait()` returns in the waiter thread, so neither the 20 ms cancel and deadline slices
nor the reader joins inflate it (R1).

### 8. Read-only git readers (src/git.rs)

Satisfies: BENCH-2.10, BENCH-9.5
Reuse: rung 2 — `Git` (src/git.rs:41), its `GIT_*` removal pattern (:52-64) and `GitError`; the bounded
runner (section 7) with `METADATA_DEADLINE` (60 s) and `Capture { stdout_cap: 64 KiB, stderr_cap: 4 KiB }`.
Respects: ARCH-2, ARCH-13
Surface:
- `Git::command` (:52-64), `Git::run` (:66) and every existing reader, including `discover::checkouts`:
  **frozen**. The design adds a separate private builder and touches none of them (BENCH-9.5).
Interface:
```rust
impl Git { pub fn head(&self) -> Result<String, GitError>;                 // rev-parse --verify HEAD
           pub fn has_tracked_changes(&self) -> Result<bool, GitError> }   // status --porcelain --untracked-files=no
```
Depth: n/a — extends `git` with two methods and a private builder.
Locality: `extend` `src/git.rs`; `discover` stays as it is.

`has_tracked_changes` is true when the capped stdout is non-empty, and stdout that hits the cap is still
non-empty. A timeout or a non-zero exit is a refusal under BENCH-2.4, naming the checkout, never a pass.

### 9. Load average (src/platform/mod.rs, src/platform/apfs.rs, tests/common/recording_fs.rs)

Satisfies: none — infrastructure for the load reading that section 1 records.
Reuse: rung 5 — `libc::getloadavg` (already a dependency; libc-0.2.191 src/unix/bsd/mod.rs:623).
Respects: ARCH-15
Surface:
- The `Fs` trait (src/platform/mod.rs:295-326) and its other impls (digest §7): **frozen** for the ten that
  keep the default `Err(Unsupported)`.
- `Recording` (tests/common/recording_fs.rs:234), which forwards every method: **replace**. It forwards the
  new one too, so in-process bench tests read a real load average (R9).
Interface: `fn load_average_hundredths(&self) -> io::Result<u64>` (the 1-minute value × 100, rounded down).
Depth: n/a — extends `Fs` by one defaulted method.
Locality: `extend` `platform`; `Apfs` overrides it with one FFI call under `// SAFETY:`.

### 10. Docs, skill and receipt comment (src/receipt.rs, plugin/skills/hive/SKILL.md, README.md, skills/hive/eval.json, skills/hive/TESTS.md, tests/skill.rs)

Satisfies: BENCH-8.1, BENCH-8.2, BENCH-8.3, BENCH-9.3, BENCH-9.4
Reuse: rung 2 — the existing seven headings and the per-command test pattern
(`the_skill_teaches_seeding_a_new_lane`, tests/skill.rs:531).
Surface:
- src/receipt.rs:1-2 module doc ("There is no float or duration slot"): **replace**. It is reworded to name
  bench as the one command whose receipt holds integer milliseconds (U12). `SCHEMA`, the types and the
  banned-key tests are unchanged (BENCH-9.3).
- SKILL.md text read by installed agents: **replace**, shipped with the plugin version (Compat obligation
  None). :19's version-mismatch hold list gains `bench`. :63 ("Never estimate bytes or time yourself") keeps
  its rule and adds that times come only from `bench`. :67 ("It carries no duration", about seed) stays true.
- tests/skill.rs `parse()` (:424-452), the wants list (:624-640) and the path-required set (:607-610):
  **replace**, adding `Bench`.
- README.md command table (:44-54): **replace**, adding a row.
- skills/hive/eval.json and the `## ` heading in skills/hive/TESTS.md that its `derived_from` names:
  **replace**, adding one behavior entry and its heading.
- tests/guards.rs's allowed state-file set (:230-237): **frozen**. Bench adds no state file, and the bench
  test compares against the seed-era set (`seeds.json` and `store/` added).
Interface: a "Which command" row, one fenced command line, a "Reading a receipt" paragraph (the prime, the
sample findings, the `_ms` series, that p90 is the max for n ≤ 9, the four seed counts, and that seed and
fill times rise through a run as finished samples add targets to scan, shown by `fill_targets_read`), the setup and
cleanup lines (`git worktree add` 2n times, then `git worktree remove`, an even n), the lockfile and cache
line of BENCH-8.1, and a "Do not" line
(run `bench` only when asked).
Depth: n/a — extends existing docs within the 150-line cap (about 65 body lines today, digest §11).
Locality: `extend` each file.

## Seams for testing

| Seam | Kind | Covers |
|---|---|---|
| `bench::order::abba`, `bench::stats::summarize` | unit | BENCH-1.2, BENCH-1.9 |
| `bench::command::{env_map, BuildCmd::args, EnvMap::command}` | unit | BENCH-3.1, BENCH-3.2, BENCH-3.4, BENCH-5.1, BENCH-5.2, BENCH-5.3, BENCH-5.4 |
| `bench::run` in process (`LibCtx`, tests/common/plan_fixture.rs:218, on `RegistryRepo` worktrees, tests/common/registry_fixture.rs:60,120), with `Limits` (**new seam**) | integration | BENCH-1.1, 1.3, 1.4, 1.5, 1.6, 1.7, 1.8, 1.10, 1.12, 4.1, 4.2, 4.3, 6.2, 6.3, 6.4, 7.1, 7.2, 9.7, 10.1, 10.2 |
| `cargo hive bench` binary (`Hive::json`, tests/common/mod.rs) in a new tests/bench.rs | integration | BENCH-1.11, 2.1–2.9, 2.11, 3.3, 6.1, 10.1 |
| `Git::head`, `Git::has_tracked_changes` (index bytes before and after; a deadline through the runner) | integration | BENCH-2.10 |
| `cargo_target::run_bounded`: deadline kill, cancel kill, `exited_after`, the stdout and stderr caps on a child that prints more than the cap, a grandchild holding stderr open | unit | BENCH-7.2, BENCH-6.1, BENCH-10.2 |
| existing tests/skill.rs | integration | BENCH-8.1, 8.2, 9.4 |
| existing tests/seed.rs, tests/status_targets.rs, src/receipt.rs tests, tests/status_checkouts.rs, src/cargo_target.rs tests, tests/guards.rs | integration / unit | BENCH-9.1, 9.2, 9.3, 9.5, 9.6, 9.7 |
| README row (review check) | manual | BENCH-8.3 |

One new seam: `bench::run` with `Limits`. The rest are pure functions or existing seams.

## Coverage check

Every ID in requirements.md appears in exactly one `Satisfies:` line:
- 1.1, 1.3–1.8, 1.10, 1.12 → §1; 1.2, 1.9 → §2; 1.11 → §5.
- 2.1–2.9, 2.11, 2.12 → §3; 2.10 → §8.
- 3.1, 3.2, 3.4 → §4; 3.3 → §5.
- 4.1–4.3 → §1.
- 5.1–5.4 → §4.
- 6.1–6.4 → §1.
- 7.1–7.3 → §1.
- 8.1–8.3 → §10.
- 9.1, 9.2 → §6; 9.3, 9.4 → §10; 9.5 → §8; 9.6 → §7; 9.7 → §1.
- 10.1, 10.2 → §1.

§9 is infrastructure (the load average for BENCH-1.6). Deliberately unmapped: none.

## Upstream sync

Changed at the conductor's final review (2026-11-09, `.skills/_pending-bench/auto/final-review.md`): BENCH-2.12
(no `Cargo.lock` refuses, §3), BENCH-7.3 (an empty store for the repo after the prime stops the run, §1) and
BENCH-8.1's lockfile, cache and removal line (§10) are new. The 7.3 check reads `store.db` once
(`seed::store::read`, src/seed/store.rs:130) and looks for a `Row` whose `repos` holds the repo's main
(src/seed/store.rs:24-34). It is a check on the repo, not on the toolchain. If the store has rows for the
repo but none for the bench's toolchain, the run passes 7.3, and seeded_1's own fill then learns that
toolchain from cold_1's fresh target, so the first compressing fill for it lands inside seeded_1's seed
time. A check through `choose::choose` with the plan's `version_text` would catch that case; it is left open
(Still-open S2) for the operator.

Changed at plan review (2026-11-09, `.skills/_pending-bench/auto/plan-review.md`): the interfaces of §1
(`Report` fields, `seed_ms` defined as `fill_ms + clone_ms`), §3 (`check` takes the registry, config and
capabilities; `Unfair` replaces `Refusal` in its result so it is not confused with `seed::source::Refused`), §4
(`env_map` takes `IntoIterator`; `EnvMap::vars`; `findings` returns a `Vec`) and §7 (`Ran` derives `Debug`)
now match tasks.md, the version that compiles with the planned tests. No decision changed.

Requirements changed at design (already applied to requirements.md, re-surfaced with this file):
- The scrub citation is now `src/cargo_target.rs:352-359`, not 350-358 or 353-360 (R5). Story 9's file list
  no longer says the scrub is extracted, or that `output_with_deadline` is reused.
- BENCH-2.5 gains the `target.split` refusal (seed's own refusal, src/seed/place.rs:206).
- BENCH-2.8 compares the whole `rustc -vV` text, not the release, because seed's `choose` matches on it (R2f).
- BENCH-2.11 (new): refuse two checkouts that resolve the same target directory (R7).
- BENCH-10.1: the wording no longer says no checkout code runs, because build scripts and proc-macros do,
  and Cargo's own `~/.cargo` writes fall under D3 (R11).
- BENCH-10.2 is narrowed to the children bench's own code spawns. `Git::run`'s missing deadline, which
  discovery already uses for every command, is a new Open Question outside ROAD-11 (R6).

- DF1 was picked as pin and override (Weigh W18; bench-interpret checked it in a temp crate, and clippy-all
  still runs clippy's lints under this env shape). BENCH-5.2 and 5.4 are amended, and the RUSTC Open
  Question is closed. BENCH-2.11 is labeled Weigh W17.
