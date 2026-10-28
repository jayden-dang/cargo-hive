# Design: Foundation (registry, discovery, classification, target identification, fingerprint reader, receipts, doctor)

Feature code: CORE
Status: Approved
Date: 2026-10-25
Requirements: ./requirements.md

## Context

Nothing exists yet. This is a greenfield Rust crate. The surface it touches is external: Cargo's on-disk target layout and fingerprint files, git's worktree metadata, and four macOS facilities (APFS clones and their sharing attributes, decmpfs compression through Applesauce, `flock`/`fcntl` locks, LocalAuthentication). These shapes are pinned down in three places:
- the scan digest `.skills/CORE/scan.md`;
- the premise review `.skills/CORE/req-review.md`;
- the independent design review `.skills/CORE/design-review.md`, whose findings F1 to F18 are folded in below.

Examples of what they established:
- the fingerprint JSON has 11 keys and is identical from 1.96.1 through nightly;
- dependency edges resolve through little-endian hex, 24,855 of 24,855;
- Cargo takes `.cargo-build-lock` exclusively with `flock(2)`, while rustc takes session `.lock` files with `fcntl`;
- `cargo metadata --no-deps` is read-only and costs 127 to 148 ms per workspace on klynt (5 runs, 2026-10-25).

**Binding constraint:** the confirmed close package. ROAD-1 is read-only on every repository. Hive's own code writes only under its state root and doctor's temporary dir (CORE-6.3). Child processes hive starts run with constrained environments and flags, so they do not write into checkouts or `~/.cargo` (CORE-7.1). Hive adds no wrapper, shim, hook, Cargo config or environment variable (A1, CORE-7.2). Every later reclaim step relies on what this layer reports, so correctness of identification and classification outranks completeness: anything unrecognised is reported unsupported, unresolved or protected, never guessed.

**Rejected alternatives:**
- **libgit2 bindings for git, or parsing Cargo's config layers ourselves for target ownership.** Both are new, large surfaces that would drift from what the `git` and `cargo` binaries actually do.
- **Computing rustc identity with `rustc-stable-hash`.** ROAD-1 judges format support by signature (Decision 6). It ties capabilities to the toolchain *release string* from `rustc -vV` (Decision 14), and later steps can compare the fingerprint JSON `rustc` u64 values directly.

Fresh retrieval (`load-subgraph`, seeded with CODE `CORE` and the requirement terms) found no other features in the catalog. There is no `docs/architecture/` spine and no Approved system docs, so the consult was a no-op.

## Decisions

1. **One crate, `cargo-hive`, with a library and a thin binary.** The binary is `cargo-hive`, invoked as `cargo hive`. Dependencies were approved by the user on 2026-10-25: `serde` 1.0.229, `serde_json` 1.0.154, `libc` 0.2.191, `clap` 4.6.7, `applesauce` 0.8.10, `objc2` 0.6.5, `objc2-local-authentication` 0.3.2, `block2` 0.6.2, `objc2-foundation` 0.3.2. `applesauce` builds two C libraries (`lzfse-sys`, `libdeflate-sys`). They need the C compiler that macOS linking already requires (Xcode Command Line Tools), so I1's "only a Rust toolchain" holds in practice; the README states it.
2. **macOS-specific dependencies are target-gated** under `[target.'cfg(target_os = "macos")'.dependencies]`. On any other OS the binary still builds, and every command exits non-zero with the macOS-only message (CORE-1.6).
3. **State root and one lock.**
   - The state root is `~/.hive/`. `HIVE_HOME` overrides it; this is documented as test-only and is the only environment variable hive reads to locate itself (F17.3).
   - Every read-modify-write of a state file runs under `flock(LOCK_EX)` on `$root/.lock`, a dedicated file that is never renamed over. The lock is blocking, so concurrent `repo add` runs serialize instead of losing an update (F2).
   - State files are JSON: `registry.json`, `config.json` (optional, user-edited), `capabilities.json`, `snapshot.json`, and `receipts/<YYYY-MM-DD>.jsonl`.
   - Every write, including a receipt append, rewrites the whole file: temp file in the same dir, `fsync`, then `rename`. A kill at any instant leaves the previous or the new complete file (CORE-6.5, F3).
   - Receipt files are daily, so a rewrite stays small.
   - A malformed registry, config, capabilities or snapshot file fails closed: the command refuses and names the file. It is never treated as empty and overwritten (F17.1).
4. **Target ownership through `cargo metadata`, with the caller's overrides scrubbed.**
   - **Which directory it runs in (F1):** each *workspace dir* of a checkout, meaning every dir at depth 0 to 2 that holds a `Cargo.toml`, deduplicated by the `workspace_root` that metadata returns. A dir inside an already-found workspace root is not queried again.
   - **Invocation (F4):** `cargo metadata --no-deps --format-version 1 --locked --offline`, with `RUSTUP_AUTO_INSTALL=0`, and with `CARGO_TARGET_DIR`, `CARGO_BUILD_TARGET_DIR`, `CARGO_BUILD_BUILD_DIR` and all `GIT_*` removed. Every other variable is inherited.
   - **Result:** `build_directory` falls back to `target_directory` when the key is absent (older cargo, F13).
   - **Failure:** any failure (stale lock, missing toolchain, malformed manifest) gives that workspace `Owner::Unresolved(reason)`. It does not abort the walk (F7).
   - **Ignored caller variables:** if the caller's environment set any of the three target variables, the receipt notes them. Each orphan candidate also records whether it *would* be owned under the caller's environment, so status can say so and ROAD-6 can treat it carefully (F8).
   - **Cost and cache (F12):** results are cached in `snapshot.json`, keyed by workspace dir plus the mtimes of its `Cargo.toml`, `Cargo.lock`, `rust-toolchain(.toml)`, and every `.cargo/config(.toml)` from the workspace dir up to `/` and in `$CARGO_HOME`. Uncached calls run up to 4 at a time. At 127 to 148 ms each, this keeps CORE-6.1 under 2 s for klynt.
   - **Lock, F9 (owner decision 2026-10-25):** GC never compiles and never replays a build. Ownership is the one exception: it may run exactly `cargo metadata --no-deps --format-version 1 --locked --offline` with the environment above, and no other cargo subcommand. **Condition for ROAD-6:** at apply time, ownership is resolved fresh and never read from the `snapshot.json` cache. Otherwise a `build.build-dir` added after the last `status` could let an orphan plan delete a target that has just gained an owner. The cache serves only `status` and plan previews.
5. **Git through the `git` binary with a scrubbed environment.** `GIT_*` is removed for every child `git`. Calls always use `--path-format=absolute`, and worktrees come from `worktree list --porcelain -z`. Submodules are detected with `--show-superproject-working-tree`. An unborn HEAD (exit 128) means no commits; `--is-shallow-repository` means shallow (scan §2).
6. **Fingerprint format signature** (ADR-0001). A unit dir is a directory under `.fingerprint/` holding exactly one `<H>.json` and a sibling file `<H>` whose content is exactly 16 lowercase hex characters. `H`'s prefix up to the target name is the unit kind; `doc-lib`, `test-`, `run-build-script-` and the like are all accepted by this rule (F6). The unit is supported when:
   - its JSON has exactly the 11 keys `rustc`, `features`, `declared_features`, `target`, `profile`, `path`, `deps`, `local`, `rustflags`, `config`, `compile_kind`, with the scanned types;
   - every `deps` entry is `[u64, string, bool, u64]`, parsed as `u64`.

   A directory with no files is ignored and is not evidence of format. A profile dir with no `.fingerprint/`, which includes the newer `build/<pkg>/<hash>/fingerprint/` layout, is unsupported as a whole (CORE-4.14).
7. **Doctor's format check (f) builds in isolation (F5, F17.8).**
   - It builds a throwaway crate in the doctor temp dir: lib, bin, `build.rs` and a test, with no dependencies.
   - The toolchain's real `cargo` and `rustc` come from `rustup which cargo|rustc`, run with cwd set to the temp dir so no `rust-toolchain.toml` elsewhere can change the answer. If rustup is absent, the `cargo` on `PATH` is used.
   - The child environment is cleared and then given an allowlist only: `PATH`, `HOME`, `TMPDIR`, `RUSTUP_HOME` (if set), `RUSTUP_TOOLCHAIN` (pinned to the resolved toolchain), `RUSTUP_AUTO_INSTALL=0`, `CARGO_HOME` and `CARGO_TARGET_DIR` (both inside the temp dir), and `RUSTC`.
   - It runs with `--offline`, and its cwd is the temp dir.
8. **Sharing attributes through `getattrlist`.** Calls use `FSOPT_ATTR_CMN_EXTENDED` with `ATTR_CMNEXT_PRIVATESIZE | ATTR_CMNEXT_CLONEID | ATTR_CMNEXT_EXT_FLAGS`. `EF_MAY_SHARE_BLOCKS = 0x1` and `EF_SHARES_ALL_BLOCKS = 0x40` are defined locally because `libc` lacks them. Clones use `libc::clonefile`.
9. **Doctor's compression check uses the library.**
   - It calls `FileCompressor::new()` and then `compressor.recursive_compress(paths, Kind::Lzfse, 0.95, level, &progress, true)` (F18).
   - The fixture is a compressible 4 MiB single-link file plus a hardlinked pair.
   - Outcomes come from the progress callbacks. `SkipReason::HardLink` maps to `SkippedHardLink`. Every other skip reason, and "not compressible enough", maps to `Failed(reason)` (F17.4).
10. **Owner-authentication availability only.** Doctor calls `LAContext::new()` then `canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthentication)`, and never `evaluatePolicy` (CORE-6.4).
11. **Lock check (d) proves the exclusive lock both ways (F16; amended by AUTOGC-1.1/1.2).**
   - A hidden subcommand `cargo-hive __hold-lock <flock|fcntl|fcntl-read> <path>` holds the lock until it is killed.
   - Doctor spawns it for each of flock, fcntl and fcntl-read, and probes the way production will: `flock(LOCK_EX | LOCK_NB)` on a read-only fd. The `fcntl` argument is the write lock; its measured key is `fcntl_write`.
   - It also measures the reverse: while hive holds that exclusive flock, another process's non-blocking fcntl read and write tries on the same file are refused.
   - The check passes only if every forward refusal and success, and both reverse refusals, are observed without waiting.
12. **Receipt schema v1.** Fields: `schema`, `hive_version`, `macos_version`, `toolchain` (the `rustc -vV` release line, or unmeasured), `command`, `ts` (when the command ran), `scope` (repos and checkouts, from arguments), `counts` (named integers only), `checks` (doctor), `findings` (unsupported profile dirs, unresolved owners, missing worktrees), `unmeasured` (list of `{field, reason}`), `notes`. There is no field for durations or time saved (CORE-5.4).
13. **Discovery is bounded explicitly.** There is no glob recursion. Walks are depth-counted `read_dir` loops: up to 3 levels in checkouts, up to 2 in cache roots, up to 2 in secondary roots. They never enter `node_modules`, `.git`, or a dir already identified as a Cargo target (CORE-6.2). Path containment is compared component-wise on realpaths, so `/private/tmpfoo` is not under `/private/tmp` (F15.6).
14. **Doctor results are tied to a toolchain release (F17.2).** `capabilities.json` records the `rustc -vV` `release:` line of the toolchain doctor ran with. `require` refuses with "run `cargo hive doctor`" when the record is missing or the active release differs (CORE-1.10).
15. **Doctor temp-dir cleanup (F10).**
   - The temp dir is a guard. It is removed on return, on check failure and on panic (`panic = "unwind"` is required in `Cargo.toml`).
   - A `sigaction` handler for SIGINT and SIGTERM sets a flag. Doctor polls it between steps and returns through the guard (CORE-1.8).
   - Temp dirs are named `hive-doctor-<pid>-<nonce>`. At start, doctor removes any whose pid is no longer alive (CORE-1.9).

ADRs: `docs/adr/0001-read-cargo-internal-fingerprints.md` (Decision 6). Decision 4 has no ADR: it is surprising and a real trade-off, but cheap to reverse (one function in `cargo_target`), so it fails the ADR gate.

## Architecture

Vocabulary for every section below: **module**, **interface**, **implementation**,
**seam** (public surface where behavior is observable and substitutable). Prefer
deep modules — an interface much simpler than what it hides.

```
cli ──► registry ──► git (infra)
 │
 ├──► discover ──► git, classify, cargo_target ──► fingerprint
 │        └──► size  (skipped under --no-sizes)
 ├──► doctor ──► platform (Fs, Approve), fingerprint
 └──► receipt
state ◄── written only via cli (registry, capabilities, snapshot, receipts)
```

### state

Satisfies: CORE-2.6, CORE-3.5, CORE-3.6, CORE-4.3, CORE-5.2, CORE-6.3, CORE-6.5, CORE-6.6, CORE-6.7, CORE-7.1
Reuse: 3 — `std::fs` and `std::os::unix` for files, `fsync` and `rename`; 5 — `libc::flock` for the lock, `serde_json` for the shapes
Interface: `State::open() -> Result<State, StateError>` (creates `$HIVE_HOME` or `~/.hive/` on first use) · `state.locked(|tx| …)`, which runs a closure holding the state lock where `tx` reads and writes `registry`, `capabilities`, `snapshot` and `append_receipt` · `state.config() -> Result<Config>` (read-only; defaults `secondary_roots = [/private/tmp, realpath($TMPDIR)]`, `cache_roots = [~/.cache/cargo-target]`; a set key replaces its default list) · `StateError::Malformed{file}`
Depth: deletion test — callers would still need only "registry, config with defaults, capabilities, a discovery snapshot and a receipt log, changed only inside `locked`". The lock file, the temp, `fsync` and `rename` protocol, the daily receipt files and fail-closed parsing stay hidden.
Locality: new `src/state.rs`. All of hive's own file writes outside doctor's temp dir go through it (CORE-6.3). For CORE-7.1, `state` guarantees that hive's own code never writes into checkouts, targets or `~/.cargo`. Child processes are constrained by `cargo_target` (Decision 4) and `doctor` (Decision 7).
Landed after execution (2026-10-28, CORE-6.6/6.7): `locked_or_cancelled(cancel, body)` tries the lock with `LOCK_NB`, retries every 10 ms, and returns `StateError::Cancelled` only while another process holds the lock and `cancel` fires; `locked` is the same wait with a cancel that never fires.

### git (infrastructure)

Satisfies: none — infrastructure used by `registry` and `discover`
Reuse: 4 — the `git` binary (2.54 on the proof machine); 3 — `std::process::Command`
Interface: `Git::at(path)` · `.toplevel()` · `.git_dir()` · `.common_dir()` (absolute paths) · `.superproject() -> Option<PathBuf>` · `.root_commits() -> Result<BTreeSet<Oid>, NoCommits>` · `.is_shallow() -> bool` · `.worktrees() -> Vec<WorktreeRecord{path, head, branch, locked, prunable, bare}>`
Depth: deletion test — callers would still need only these answers. The `-z` porcelain parsing, the `GIT_*` scrub, the rule that exit 128 means unborn, and absolute-path normalisation stay hidden.
Locality: new `src/git.rs`; no neighbour impact.

### registry

Satisfies: CORE-2.1, CORE-2.2, CORE-2.3, CORE-2.4, CORE-2.5, CORE-2.7, CORE-2.8, CORE-2.9
Reuse: none — new code (rung 7). No library models "a registered repository".
Interface: `register(path, &mut Tx) -> Result<Registered | AlreadyRegistered, RegisterRefusal>`, where `RegisterRefusal` is one of `NotGit`, `Submodule{superproject}`, `LinkedWorktree{main}`, `NoCommits`, `Shallow` or `NoCargoToml` · `list(&State) -> Vec<Repo{main, root_commits}>`
Depth: deletion test — callers would still need to know "a repo is a main-checkout realpath plus its root-commit set, and these six refusals exist". The probe order and the depth-2 `Cargo.toml` search stay hidden.
Locality: new `src/registry.rs`; reads through `git`, writes through a `state` transaction.

Probe order: not git → submodule → linked (git-dir ≠ common-dir) → no commits → shallow → no `Cargo.toml` within 2 levels → already registered → record.

### classify

Satisfies: CORE-3.2, CORE-3.4
Reuse: none — new code (rung 7). A pure function.
Interface: `classify(facts: &CheckoutFacts, registered_mains: &[PathBuf], secondary_roots: &[PathBuf]) -> (Class::{Protected, Secondary}, Reason)`, where `CheckoutFacts{realpath: Result<PathBuf, Reason>, kind: Main | Linked{main} | Clone, git: Result<(), Reason>}`
Depth: deletion test — callers would still need only "C1 rules (1) to (4) in order, and unclassifiable means protected". It is deliberately tiny and pure, because it is the safety core and every branch is unit-tested.
Locality: new `src/classify.rs`; consumed by `discover`. ROAD-2, 5, 6 and 9 call it unchanged.

### discover

Satisfies: CORE-3.1, CORE-3.3, CORE-3.7, CORE-4.2, CORE-4.15, CORE-6.1, CORE-6.2
Reuse: 3 — `std::fs::read_dir` in depth-counted loops, and `std::fs::canonicalize` for realpaths
Interface: `discover(repo: &Repo, &Config, &mut Tx /* snapshot cache */) -> RepoView{checkouts: Vec<Checkout{path, class, reason, missing}>, targets: Vec<TargetDir>}`
Depth: deletion test — callers would still need to know "a repo's checkouts with classes, plus its Cargo target dirs". These stay hidden: the three checkout sources, realpath resolution, the root-commit intersection, the bounded walks, the missing-dir `stat`, the workspace-dir rule, the metadata cache with bounded parallelism, and the merge of out-of-root build dirs.
Locality: new `src/discover.rs`. It calls `git`, `classify` and `cargo_target`, and is the only producer of `CheckoutFacts`.

Checkouts are main, plus `git.worktrees()`, plus primary checkouts at most 2 levels under each secondary root whose `root_commits` intersect the repo's. Shallow clones never match, so they fail safe.

Every worktree path is `stat`-ed, and a missing dir becomes `missing` whatever `prunable` says (CORE-3.7). Every path is canonicalized before it reaches `classify` (CORE-3.3).

Targets are found by walking each known checkout up to 3 levels and each cache root up to 2. Then every `build_directory` that `cargo_target` resolved is added (CORE-4.15).

### cargo_target

Satisfies: CORE-4.5, CORE-4.10, CORE-4.12, CORE-4.13
Reuse: 4 — `cargo metadata` (Decision 4); 3 — `std::fs` for tag and layout probes
Interface: `is_cargo_target(dir) -> Option<Layout::{Fingerprint, NewBuildDir}>` · `resolve(workspace_dir, cancel: impl FnMut() -> bool) -> Resolved{target_directory, build_directory, workspace_root} | Unresolved(reason)` · `TargetDir{path, owner: Owned(checkout) | Unresolved(reason) | OrphanCandidate{owned_under_caller_env: Option<checkout>}, profiles: Vec<fingerprint::ProfileReport}, size: Option<u64>, readable: Result<(), Reason>}`
Depth: deletion test — callers would still need to know "is this a Cargo target, and who owns it (or that it's unknown)". These stay hidden: the exact tag line, the two layout probes, the child flags and environment, JSON extraction, the `build_directory` fallback, and the caller-environment comparison.
Locality: new `src/cargo_target.rs`; calls `fingerprint` for each profile dir.

A dir is a target iff the tag line matches **and** some child dir has `.fingerprint/` or `build/<pkg>/<16hex>/fingerprint/` (CORE-4.12).

Ownership is a realpath match against `target_directory` or `build_directory` (CORE-4.13). A target is an orphan candidate only when no checkout owns it **and** every known checkout's workspaces resolved. ROAD-6 must re-resolve ownership fresh at apply time, never from the cache (Decision 4, F9). Otherwise it is `Unresolved` (CORE-4.5, F7). IO and permission errors become `readable: Err(reason)` and the walk continues (CORE-4.10).

### fingerprint

Satisfies: CORE-4.7, CORE-4.8, CORE-4.14
Reuse: 5 — `serde_json` (`Value`, read as `u64`); the graph logic is new (rung 7)
Interface: `read_profile(dir) -> ProfileReport{path, format: Supported | Unsupported(reason), units: usize, edges: Option<EdgeCounts{resolved, unresolved, ambiguous}>, graph: Option<UnitGraph>}` · `UnitGraph{units: Vec<Unit{dir_name, kind, hash_hex, deps}>, edges: Vec<(UnitIdx, UnitIdx)>}`
Depth: deletion test — callers would still need to know "units, their edges, and whether the format is trusted". These stay hidden: the unit-dir rule, the 11-key and type signature, `[u64, str, bool, u64]` validation, and the little-endian hex index.
Locality: new `src/fingerprint.rs`. `UnitGraph` is the type ROAD-5 (closure liveness) consumes unchanged. Unsupported profiles are written to `snapshot.json` by `cli` as "hive's state" (CORE-4.14, F14).

Edges map each `deps[i][3]` to `hex(u64.to_le_bytes())`, looked up among unit hash files. One hit is resolved, none is unresolved, several are ambiguous. A single failing unit marks the whole profile dir `Unsupported`, and then no edges are reported (CORE-4.14).

### size

Satisfies: CORE-4.6, CORE-4.11
Reuse: 3 — `std::os::unix::fs::MetadataExt` (`blocks`, `ino`, `dev`)
Interface: `allocated_bytes(dir) -> u64`, the inode-deduplicated sum of `st_blocks × 512`
Depth: deletion test — callers would still need only "the allocated bytes of a tree, each inode counted once". The `(dev, ino)` set and the walk that never follows symlinks stay hidden.
Locality: new `src/size.rs`. `discover` fills `TargetDir.size`. Under `--no-sizes` it leaves `None`, and `cli` records `unmeasured: sizes — --no-sizes` (CORE-4.11).

### platform

Satisfies: CORE-1.6, CORE-6.4
Reuse: 5 — `libc` (`clonefile`, `getattrlist`, `flock`, `fcntl`, `mkdtemp`, `sigaction`), `applesauce`, `objc2-local-authentication`; the trait shape is new (rung 7)
Interface: `trait Fs { temp_dir(prefix) -> TempDir; write_file; read; hardlink; set_mtime; clone_file(src, dst); sharing(path) -> Sharing{private_bytes, clone_id, may_share, shares_all}; compress(path) -> CompressOutcome::{Compressed, SkippedHardLink, Failed(reason)}; hold_lock(kind, path) -> LockHolder; try_lock_exclusive(path) -> TryLock; try_lock_elsewhere(kind, path) -> LockState::{Free, Held} }` · `trait Approve { owner_auth_available() -> Availability }` · `fn current() -> Result<Platform, NotMacOs>`
Depth: deletion test — callers would still need to know these filesystem operations and one auth question. These stay hidden: the FFI structs, the local EF constants, the Applesauce progress adapter, the hidden `__hold-lock` holder process, and the objc message sends.
Locality: new `src/platform/{mod.rs, apfs.rs, local_auth.rs, unsupported.rs}`. `unsupported.rs` builds on non-macOS, and `current()` returns `NotMacOs` there (CORE-1.6). ROAD-2, 7, 8 and 9 extend these traits rather than adding new ones. `Approve` gains `evaluate` only in ROAD-7 (CORE-6.4).

### doctor

Satisfies: CORE-1.1, CORE-1.2, CORE-1.3, CORE-1.4, CORE-1.7, CORE-1.8, CORE-1.9, CORE-1.10, CORE-1.11, CORE-1.12
Reuse: none — new code (rung 7), composing `platform`, `fingerprint` and `state`
Interface: `run(&dyn Fs, &dyn Approve, &Toolchain) -> DoctorReport{macos, below_floor, toolchain_release, checks: [Check{id: a..f, verdict: Pass | Fail | Unavailable, measured: Value}]}` · `capabilities_from(&DoctorReport) -> Capabilities` · `require(&Capabilities, Capability, active_release) -> Result<(), CapabilityRefusal{check | RunDoctor}>`
Depth: deletion test — callers would still need to know "run six checks and get verdicts with measured values, a capability map tied to a toolchain release, and a gate". These stay hidden: the fixtures, the throwaway crate with its isolated environment, the CORE-1.3 check → capability map, the signal flag and the stale-dir sweep.
Locality: new `src/doctor.rs`. `require` is the gate every capability-dependent command calls from ROAD-2 onward (CORE-1.4, CORE-1.10).

The macOS version comes from `sw_vers -productVersion`. Below 27 sets `below_floor` and every check still runs (CORE-1.7). Cleanup follows Decision 15 (CORE-1.8, CORE-1.9).
Landed after execution (2026-10-28, CORE-1.11/1.12): `drop_directory_toolchain` removes `RUSTUP_TOOLCHAIN` and `RUSTUP_TOOLCHAIN_SOURCE` from `rustup which` and from `cargo_target::resolve`'s `cargo metadata` unless the source is `cli`, `env` or `default`.

### receipt

Satisfies: CORE-5.3, CORE-5.4, CORE-5.5, CORE-5.6
Reuse: 5 — `serde` derive on a closed struct
Interface: `Receipt::new(command, scope) -> ReceiptBuilder` · `.count(name, u64)` · `.check(Check)` · `.finding(Finding)` · `.unmeasured(field, reason)` · `.note(text)` · `.finish() -> Receipt`
Depth: deletion test — callers would still need to know "counts, checks, findings, unmeasured fields with reasons, notes, and fixed metadata". Schema versioning stays hidden, and so does the fact that the type has no float or duration slots, which makes CORE-5.4 impossible to violate.
Locality: new `src/receipt.rs`. Later ROADs add counts and finding kinds by name; they never add numeric field types.

### cli

Satisfies: CORE-3.8, CORE-5.1, CORE-5.7, CORE-7.2
Reuse: 5 — `clap` derive
Interface: `cargo hive doctor | repo add <path> | repo list | status [<repo>…] [--no-sizes]`, each with `--json`; plus the hidden `__hold-lock`
Depth: n/a — a thin adapter. It maps arguments to library calls, writes results through one `state.locked` transaction, and prints.
Locality: new `src/main.rs` and `src/cli.rs`.

Scope always comes from the arguments. `status` with no repo means every registered repo, and the cwd is never read (CORE-3.8, CORE-5.7). With `--json`, stdout carries exactly the receipt and all prose goes to stderr (CORE-5.1). Nothing hive does at install or run time writes Cargo config, environment variables, wrappers or hooks (CORE-7.2).

## Seams for testing

| Seam | Kind | Covers |
|---|---|---|
| The `cargo-hive` binary run as a subprocess with `HIVE_HOME=<tmp>`. Fixtures are built under `/private/tmp`: git repos (main, linked worktree, submodule, unborn, shallow), tiny crates built for real target dirs, and copied fingerprint fixtures (newer layout, corrupted JSON, a `doc-lib` unit). Doctor runs against the real APFS proof machine and is machine-gated | integration | CORE-1.1, CORE-1.2, CORE-1.4, CORE-1.7, CORE-1.8, CORE-1.9, CORE-1.10, CORE-2.1–2.9, CORE-3.1, CORE-3.3, CORE-3.5–3.8, CORE-4.2, CORE-4.3, CORE-4.5, CORE-4.6, CORE-4.10–4.15, CORE-5.1, CORE-5.2, CORE-5.5–5.7, CORE-6.1, CORE-6.2, CORE-6.3, CORE-6.4, CORE-6.5, CORE-7.1, CORE-7.2 |
| The `cargo_hive` library: `classify`; `fingerprint::read_profile`; `receipt`; `doctor::run` and `require` with fake `Fs` and `Approve`; `state` locking and atomic writes; and `platform::current` on a non-macOS cfg path through a `cfg(test)` OS override | unit | CORE-1.1, CORE-1.3, CORE-1.6, CORE-3.2, CORE-3.4, CORE-4.7, CORE-4.8, CORE-4.14, CORE-5.3, CORE-5.4 |

There are two new seams, since no code exists to reuse. NFR checks:
- **CORE-6.1:** timed runs against klynt (manual performance test, not CI).
- **CORE-6.4:** doctor runs with no TTY and must finish without blocking.
- **CORE-6.5:** a kill-during-write integration test, plus a concurrent `repo add` test (F2).
- **CORE-1.6:** additionally, `cargo check --target x86_64-unknown-linux-gnu` in CI to prove the non-macOS build compiles.

## Coverage check

Every live ID in requirements.md appears in exactly one `Satisfies:` line:

| Module | IDs |
|---|---|
| state | 2.6, 3.5, 3.6, 4.3, 5.2, 6.3, 6.5–6.7, 7.1 |
| registry | 2.1–2.5, 2.7–2.9 |
| classify | 3.2, 3.4 |
| discover | 3.1, 3.3, 3.7, 4.2, 4.15, 6.1, 6.2 |
| cargo_target | 4.5, 4.10, 4.12, 4.13 |
| fingerprint | 4.7, 4.8, 4.14 |
| size | 4.6, 4.11 |
| platform | 1.6, 6.4 |
| doctor | 1.1–1.4, 1.7–1.12 |
| receipt | 5.3–5.6 |
| cli | 3.8, 5.1, 5.7, 7.2 |

Deliberately unmapped: ~~CORE-1.5~~, ~~CORE-4.1~~, ~~CORE-4.4~~ and ~~CORE-4.9~~, which are superseded by CORE-1.8 and 1.9, and by 4.12, 4.13 and 4.14.

Unverified, carried from the review:
- applesauce 0.8.10 and objc2-local-authentication 0.3.2 sources were not read locally; those claims rest on the scan's compile-and-run evidence;
- `canEvaluatePolicy` with no GUI session;
- `cargo metadata` with a stale or malformed `Cargo.lock`, which Decision 4 handles as `Unresolved`;
- whether the mise shims for `cargo` and `rustup` write their own state.
