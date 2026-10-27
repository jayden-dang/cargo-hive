# Design review: CORE foundation (design.md vs requirements.md)

Reviewer stance: independent, no stake in the author's framing. Inputs: design.md, requirements.md, scan.md, req-review.md, close-package.md, ADR-0001, CONTEXT.md.
Machine: Darwin 27.0.1 (`sw_vers -productVersion` = 27.0.1, `/usr/bin/sw_vers`), rustup 1.29.2, cargo/rustc 1.97.1, git 2.54. `cargo`, `rustup` on PATH are **mise shims** (`~/.local/share/mise/shims/`).
Local experiments ran only in `/private/tmp` (removed afterwards), plus stat-only snapshots of `~/.rustup` and `~/.cargo` (top levels). Nothing was written under `~/.cargo`, `~/.rustup` or `~/Developer`.

Severity: blocker = design cannot satisfy a live requirement or breaks a lock; major = a requirement or lock is violated or overclaimed in a realistic case; minor = gap with a cheap fix; nit = wording.

## Part 1: Code-facing claims (external systems)

| Claim | Evidence | Status |
|---|---|---|
| libc 0.2.191 `clonefile`, `getattrlist`, `flock`, `mkdtemp` exist on aarch64-apple-darwin | Read `~/.cargo/registry/src/*/libc-0.2.191`: `clonefile` at `src/unix/bsd/apple/mod.rs:5192`, `getattrlist` at 5371, `flock` at `src/unix/mod.rs:2273`, `mkdtemp` at 2161; `FSOPT_ATTR_CMN_EXTENDED=0x20` (4315), `ATTR_CMNEXT_PRIVATESIZE`/`CLONEID`/`EXT_FLAGS` (4390-4396). `EF_*` grep: absent, so Decision 8's "define locally" is right. scan §3c compiled and ran them. | VERIFIED |
| `applesauce::FileCompressor::recursive_compress(paths, Kind::Lzfse, ratio, level, &progress, verify)` and `SkipReason::HardLink` via progress callback, in 0.8.10 | Crate source is not in the local registry. Relies on scan §3a (source read plus a compiled, run spike). Two inexactnesses in the design text: it is a `&mut self` method (`FileCompressor::new().recursive_compress(..)`, not an associated function), and `paths` is `impl IntoIterator<Item=&Path>`. | VERIFIED via scan only; nit (see F18) |
| `objc2-local-authentication` 0.3.2 `LAContext::new()` and `canEvaluatePolicy_error` | Not in local registry (only objc2 0.6.4, objc2-foundation 0.3.2). scan §3b compiled and ran it: `canEvaluate=true`, no dialog. objc2 0.6.5 is not in the local registry but scan resolved it. | VERIFIED via scan only |
| No-GUI-session behaviour of `canEvaluatePolicy` (CORE-6.4 verification) | scan Unknowns and req-review §8: unverified. | UNVERIFIED |
| `cargo metadata` fields `target_directory` / `build_directory` / `workspace_root` | I re-ran with cargo 1.97.1 and a scrubbed env: default gives `target_directory == build_directory`. `CARGO_BUILD_BUILD_DIR` changes only `build_directory`. `CARGO_BUILD_TARGET_DIR` and `CARGO_TARGET_DIR` change both. So Decision 4's three-variable scrub list is correct and complete for those keys. `build_directory` is a recent field. An older active cargo will omit it, and the design does not say to fall back to `target_directory`. | VERIFIED (1.97.1); fallback missing (F13) |
| `git rev-parse --show-superproject-working-tree`, `--path-format=absolute`, `worktree list --porcelain -z` | scan §2 (throwaway repos, git 2.54). Not re-run. | VERIFIED via scan |
| `sw_vers -productVersion` | Ran: `27.0.1`. | VERIFIED |
| `rustup which cargo` | Ran: prints the toolchain's real `cargo` path. | VERIFIED |
| `rustup which` does not write under `~/.rustup` | Stat snapshot of `~/.rustup` (depth 3, 42 entries) before and after two `rustup which` runs: no difference. Also in a dir with `rust-toolchain.toml` naming an uninstalled channel and `RUSTUP_AUTO_INSTALL=0`: exits 1 with "toolchain not installed" and no writes. Without `RUSTUP_AUTO_INSTALL=0` I did not test it, because it could install a toolchain. `RUSTUP_AUTO_INSTALL` exists in the rustup 1.29.2 binary and `rustup set auto-install` exists. | VERIFIED for default toolchain; override case UNVERIFIED (F4) |
| Check (f): real toolchain cargo, temp `CARGO_HOME`, `CARGO_TARGET_DIR`, `RUSTC`, `--offline` avoids all writes to `~/.cargo` and `~/.rustup` | Ran `cargo test --no-run --offline` on a lib+bin+build.rs+test crate with `env -i` and the toolchain's own `cargo`/`rustc`. Snapshot of `~/.cargo` (depth 3, excluding registry and cargo-target) and `~/.rustup`: no diff. Cargo created `.global-cache`, `.package-cache` and `.package-cache-mutate` inside the temp `CARGO_HOME` only. So the mechanism works **with a clean env**. The design does not say the env is cleaned (F5). | VERIFIED (clean env); design incomplete |
| `cargo metadata --no-deps` writes nothing to `CARGO_HOME` (global cache tracker) | Run with a fresh empty `CARGO_HOME`, `--offline`: `CARGO_HOME` stayed empty (no `.global-cache`, no `.package-cache`). Matches scan §5 and req-review §10 (real `~/.cargo` snapshot unchanged, including `.global-cache`). I did not run it against the real `CARGO_HOME` (forbidden by my safety rules). | VERIFIED (fresh home); real-home run is req-review evidence only |
| `cargo metadata --no-deps` never writes `Cargo.lock` into a registered repo | Fresh crate, no lock, `--offline`: no `Cargo.lock` created. scan §5 same. Stale or malformed lock, `[patch]`, `rust-toolchain.toml` auto-install: not tested by anyone. | UNVERIFIED for those states (F4) |
| Fingerprint hash-file kinds in Decision 6 (`lib, bin, test, example, build-script, run-build-script`) are complete | **Refuted.** `cargo doc` (1.97.1) writes `.fingerprint/<unit>/doc-lib-c` plus `doc-lib-c.json` with the usual JSON shape. `cargo build --benches` yields `test-bench-b` (covered by the `test` prefix). Corpus under `~/.cache/cargo-target` has none, so the scan missed it. | REFUTED (F6) |

## Part 2 to 5: Findings

### F1. blocker: no rule for which directory `cargo metadata` runs in
**Claim.** cargo_target `resolve_owner(workspace_dir)` and Decision 4 ("runs in the workspace dir") produce CORE-4.13 and CORE-4.15 ownership.
**Evidence.** CORE-2.1 and 2.3 explicitly allow the `Cargo.toml` to sit up to 2 levels below the checkout root. On the proof repo klynt, `Cargo.toml` is in `backend/`, and req-review §10 ran metadata from `backend`, not from the checkout root. `cargo metadata` searches parent directories, not children, so at the checkout root it fails. Nothing in the design says who finds the manifest dirs, how many `cargo metadata` calls run per checkout (a checkout may hold several independent workspaces within 2 levels), or what happens when a checkout has none (a worktree at an old commit).
**Impact.** For the one repo the NFR is measured on, no checkout is found to own its target, so every target is an orphan candidate.
**Fix.** Add to `discover` or `cargo_target`: "workspace dirs of a checkout = dirs with a `Cargo.toml` at depth 0 to 2, not inside a dir that is itself a member (dedupe by `workspace_root` returned from metadata)". State that `resolve_owner` runs once per distinct manifest dir and merges. Add this to CORE-6.1 budgeting (F12).

### F2. major: registry read-modify-write has no lock, so concurrent `repo add` loses updates
**Claim.** Decision 3: whole-file writes are temp+fsync+rename; `state` is "the single writer".
**Evidence.** "Single writer" is a module-level statement; the processes are many (two agents can run `cargo hive repo add` at once). Process A reads registry.json, process B reads it, both write a temp file and rename; the later rename wins and the earlier registration is silently gone. Each file is complete, so CORE-6.5 holds, but CORE-2.1 ("record ... in hive's registry") and CORE-2.4 (already registered must leave the registry unchanged) are violated in effect. The same applies to `capabilities.json` (doctor in parallel with a gated command). Rename does not provide mutual exclusion.
**Fix.** Add a lock to `state`: `flock(LOCK_EX)` on `$HIVE_HOME/.lock` (a separate file, never renamed over) for the whole read-modify-write of registry and capabilities and for receipt appends. Interface stays small, e.g. `state.update_registry(|r| ...)`. Add a concurrent `repo add` test (two processes, different repos, both must be present afterwards).

### F3. major: the receipt-append mechanism does not give CORE-6.5 under SIGKILL, and `ftruncate` is unsafe under concurrency
**Claim.** "One `write(2)` with `O_APPEND`, then `fsync`. On a short write `ftruncate` back to its previous length (CORE-6.5)."
**Evidence by failure mode.**
- *ENOSPC / short write while the process survives*: ftruncate runs and restores the file. OK.
- *SIGKILL*: the truncate code never runs. A write(2) of a short line to a local APFS file is in practice not torn by SIGKILL, but nothing guarantees it, and any partial tail (after a short write followed by kill, or a crash) is left permanently. The next append then glues its line onto the fragment, which corrupts a second record. The requirement's verification is "parses every file afterwards", so a torn last line fails the test. The design's "short-write truncate" cannot cover the very failure (kill) it is cited for.
- *Concurrent appenders without a lock*: A short-writes, B appends its full line after it, A then truncates to "previous length" and **deletes B's complete line**. The recorded previous length is also stale if another process appended between A's `fstat` and A's write.
- `fsync` on macOS does not flush to media (`F_FULLFSYNC` does). Irrelevant to process kill, relevant to power loss, which CORE-6.5 does not require. Nit.
**Fix.** (a) Take the F2 lock around append. (b) Make readers and the next append tolerant: on open for append, if the file does not end with `\n`, either truncate the unterminated tail (under the lock) or write a leading `\n`; readers skip an unterminated last line. (c) Reword the design: "complete form" for the log means every newline-terminated line parses. If a literal "parses every byte" reading is wanted, requirements need to say so. (d) Use `F_FULLFSYNC` or drop the fsync claim; do not describe it as crash safety beyond process kill.

### F4. major: `cargo metadata` can still write under `~/.rustup` and into registered repos (CORE-6.3, CORE-7.1)
**Claim.** Decision 4: `cargo metadata --no-deps --format-version 1`, "never compiles", scrubs three vars; "read-only" is leaned on for CORE-7.1.
**Evidence.**
- The design omits `--locked` and `--offline`, though scan §5 recommends them ("Run it with `--locked --offline` and a scrubbed env").
- Without `--locked`, cargo may write `Cargo.lock` into a registered checkout when the lock is missing or stale. Scan §5 and req-review §10 tested only a populated lock or a no-dependency/new crate; the stale or malformed lock case is listed as untested. I confirmed only that a fresh crate gets no lock offline.
- `cargo` on PATH is a rustup proxy (here via a mise shim). If the checkout has a `rust-toolchain.toml` naming an uninstalled toolchain, rustup 1.28+ auto-install is the default and would install into `~/.rustup`. I confirmed that `RUSTUP_AUTO_INSTALL=0` turns this into a clean error with no writes. The design does not set it. CORE-7.1 lists `~/.cargo` and toolchains as at-risk.
- `cargo metadata --no-deps` does not write to `CARGO_HOME` (verified above), so the global-cache-tracker concern in the brief does not materialise for this command.
**Fix.** Child env for every cargo/rustup call: `RUSTUP_AUTO_INSTALL=0`, args `--locked --offline`. If it fails, report the checkout's ownership as `Unresolved(reason)`, not an error that aborts the walk. Add a test with a fixture whose `rust-toolchain.toml` names an absent channel and one with a stale `Cargo.lock`; assert the fixture and `~/.rustup` snapshots are unchanged. State what remains unverified (stale-lock behaviour with `--locked` should simply fail, which is safe).

### F5. major: doctor check (f) does not scrub the inherited environment, so it can write outside its temp dir
**Claim.** Decision 7: point `CARGO_HOME` and `CARGO_TARGET_DIR` into the temp dir; this keeps `~/.cargo` untouched (CORE-7.1) and writes only in the temp dir (CORE-6.3).
**Evidence.** Verified that the mechanism writes nothing under `~/.cargo`/`~/.rustup` when run under `env -i`. But the design only overrides three variables and inherits the rest. Inherited `CARGO_BUILD_BUILD_DIR` sends fingerprints and intermediates to the user's directory (the design itself scrubs exactly this variable in Decision 4 but not here). `RUSTC_WRAPPER` / `RUSTC_WORKSPACE_WRAPPER` (sccache writes its own cache), `RUSTFLAGS`, `CARGO_BUILD_RUSTFLAGS`, `CARGO_BUILD_TARGET`, `CARGO_INCREMENTAL`, `CARGO_PROFILE_*`, `RUSTUP_TOOLCHAIN`, `CARGO_TARGET_<triple>_LINKER` change either the write location or the fingerprint content that (f) then judges.
**Fix.** `env_clear()` plus an allowlist (`PATH`, `HOME`, `TMPDIR`, `RUSTUP_HOME` if set, `CARGO_HOME`/`CARGO_TARGET_DIR`/`RUSTC` as set by hive, `RUSTUP_AUTO_INSTALL=0`). Also run `cargo` with cwd inside the temp dir. Record the allowlist in design.

### F6. major: Decision 6's kind list omits `doc-`, so profile dirs touched by `cargo doc` are wrongly marked unsupported
**Claim.** The hash file is `<kind>-<target>`, kind one of six listed values (ADR-0001 depends on it).
**Evidence.** Experiment: `cargo doc` writes `doc-lib-c` and `doc-lib-c.json`; `cargo check`, `cargo build --benches` give `lib-c`, `test-bench-b`. scan §1.5 also asked for `bench-` (not observed in 1.97.1: benches appear as `test-bench-*`). Under the design, one `doc-lib-*` unit makes the whole profile dir `Unsupported`, which later disables deletion and seeding for any target that ever ran `cargo doc`. It fails safe but is a false negative and a silent loss of function.
**Fix.** Define the name grammar as `(doc-|test-|run-)?(lib|bin|example|build-script|test|bench|proc-macro...)-...` verified against Cargo, or accept any `[a-z-]+` hash file whose content is exactly 16 hex and whose sibling `<name>.json` exists, and keep the 11-key/type check as the real gate. Add a throwaway-crate fixture for `doc-lib` to the (f) crate or the unit tests. Also note unverified: `proc-macro` units (their kind may be `lib`).

### F7. major: a failed ownership resolution turns a live target into an "orphan candidate"
**Claim.** cargo_target: "Ownership is a realpath match ...; everything else is an orphan candidate (CORE-4.5)".
**Evidence.** `resolve_owner` returns `Result`. Failure modes (toolchain not installed with auto-install off, broken manifest, a worktree at an old commit with a manifest cargo rejects, older cargo) have no path into `TargetDir.owner`; the only non-owned value is `OrphanCandidate`. The close package says unrecognised is reported unsupported or protected, never guessed. Later steps treat orphans as plan-tier whole-dir deletion candidates.
**Fix.** Add `Owner::Unresolved{checkout, reason}` and make "orphan" require that every known checkout resolved successfully. Count unresolved in the receipt `counts`.

### F8. minor: the env scrub makes the proof machine's own target an orphan
**Claim.** Decision 4: the answer "depends only on the checkout's own config files, not on who calls hive".
**Evidence.** req-review §10: klynt's live cache dir `~/.cache/cargo-target/klynt` is not what cargo resolves in a clean shell (`backend/target`), so it would be reported as an orphan candidate unless the user's builds set a variable or config differently. The design records ignored variables in `notes` but still classes the dir as orphan, with no link between the two facts.
**Fix.** Keep the scrub (it is the right trade for determinism) but add `Orphan{matches_caller_env: Option<Checkout>}` so status prints "would be owned if CARGO_TARGET_DIR=... applied". Do not let ROAD-6 treat that as plain orphan without the human. Costs one extra metadata run only when ignored vars exist.

### F9. major: "GC never invokes cargo" is satisfied by redefinition, not by design
**Claim.** Decision 4: "GC liveness stays disk-only (close package D3), so 'GC never invokes cargo' holds."
**Evidence.** Close package, Data/GC liveness: "GC never invokes cargo and never replays commands." Orphan determination (plan tier: "whole orphan target dirs") needs `resolve_owner`, which runs cargo. Liveness (roots/closure) is disk-only, but the plan that decides which dirs are orphans is GC input. The design reads the sentence narrowly (liveness only). That may be what the owner meant, but the design should not claim the lock is trivially intact. Note CORE-7.1 itself is not violated by `cargo metadata` (verified above).
**Fix.** Either (a) persist ownership in state at `status` time and let GC read the snapshot, with an age check, or (b) flag it in the design as a deliberate widening, "ownership may invoke `cargo metadata --no-deps --locked --offline`; liveness never does", and get the owner's confirmation before ROAD-6. Do not write "holds" without the qualifier.

### F10. major: CORE-1.5 is overclaimed ("every exit path including panic via drop"); SIGINT is realistic
**Claim.** Doctor: temp dir "removed on every exit path, including a panic, via drop".
**Evidence.** Drop runs on normal return and on unwinding panic. It does not run on SIGKILL, SIGSEGV, `panic = "abort"`, `std::process::exit` called while the guard is live, or on SIGINT/SIGTERM whose default action ends the process without unwinding. Doctor takes seconds (it builds a crate), so Ctrl-C is the likely interruption. Orphans left: the temp dir and possibly the lock-holder child. Requirement text: "WHEN doctor exits, whether its checks passed or failed" - an exit is a return or panic; SIGKILL cannot be required. SIGINT/SIGTERM are borderline; honest reading: not required, but the design's "every exit path" is false as written.
**Fix.** (a) Reword to "normal return, check failure, or panic". (b) Install SIGINT/SIGTERM handling via `libc::sigaction` that sets a flag the long steps poll, then return through the guard. (c) Doctor start sweeps stale `hive-doctor-*` dirs it created earlier (named with a recognizable prefix and a marker file) and kills/reaps its child holders. (d) cli must not call `process::exit` before the guard is dropped; keep `panic = "unwind"` in the release profile (assert in a test).

### F11. minor: CORE-1.1 is never exercised against the real platform in the seams table
**Claim.** Seams: CORE-1.1 appears only in the unit row, where `doctor::run` uses fake `Fs`/`Approve`.
**Evidence.** CORE-1.1 is about measured real clone/compress/lock/LA behaviour. A fake `Fs` proves wiring only. The integration row lists 1.2 to 1.7 but not 1.1.
**Fix.** Add CORE-1.1 to the integration row (real `cargo-hive doctor` on the APFS proof machine, assert each check measured value) and say it is machine-gated. Also note CORE-1.6 cannot be run on this Mac without a cross-compile or an env override; add a `cargo check --target x86_64-unknown-linux-gnu` style gate or a test hook, otherwise 1.6 is untested.

### F12. minor: CORE-6.1 budget omits the cost the design adds
**Claim.** Seams note: CORE-6.1 is a timed manual run against klynt.
**Evidence.** The 39 ms + 53 ms in SPIKES §13 predate the design's per-checkout `cargo metadata` calls (one process spawn per manifest dir per checkout, F1), `root_commits` per candidate clone under secondary roots, and the `git worktree list`. I could not measure klynt without risking writes, so this is unmeasured. Dozens of worktrees times a cargo spawn is plausibly well over 2 s.
**Fix.** Run metadata calls in parallel (bounded), or cache by `(manifest path, mtime of Cargo.toml and .cargo/config.toml)` in state; add the measurement as an acceptance step before approving the design.

### F13. minor: `build_directory` absent on older cargo
`build_directory` is a recent `cargo metadata` field. Define `build_directory = target_directory` when the key is missing (older active cargo), and test it with a stubbed JSON. (Verified present in 1.97.1 only.)

### F14. minor: CORE-4.14 "in hive's state" has no home in state
**Claim.** fingerprint/cargo_target mark profile dirs unsupported; state lists registry/config/capabilities/receipts only.
**Evidence.** CORE-4.14 says "mark that profile directory as unsupported in status and in hive's state". No state file stores it. The only persisted artefact is the receipt. That is a reinterpretation.
**Fix.** Either add `unsupported.json` (profile path, reason, rustc, mtime) to `state`, or amend CORE-4.14 to "in status and in the receipt". Prefer the first: ROAD-2/5 need it to refuse deletion without re-reading.

### F15. minor: names and flows that do not line up
1. `Refusal` means two things: registry's six-variant enum and doctor's `Refusal{check}`. Rename one (`RegisterRefusal` / `CapabilityRefusal`).
2. `size`: the diagram shows `discover -> size`, but `RepoView`/`TargetDir` have no size field, and size's Locality says `cli` skips it under `--no-sizes`. So the data flow is `cli -> size`, not `discover -> size`. Fix the diagram or add `size` to `TargetDir`.
3. `cargo_target` Interface names `ProfileDir` but `fingerprint` returns `ProfileReport`; define the relationship (one type).
4. The diagram has no `cli -> doctor` edge and no `cli -> size` edge.
5. state says it is "the only code that creates or changes files outside doctor's temp dir, which is how CORE-6.3 and CORE-7.1 are enforced structurally." False as written: `cargo metadata` (child process) can touch the repo (F4), the doctor throwaway build writes in its own dir, `platform` creates the temp dir. Structural enforcement of CORE-7.1 lives in cargo_target and doctor child-env policy (F4, F5), so 7.1 belongs in their Satisfies lines or the claim should be softened to "hive's own code".
6. CORE-3.3 (resolve every path to realpath) sits under `classify`, but `classify` only consumes `realpath: Result<PathBuf>`; the resolver is `discover`. Also the `secondary_roots` handed to `classify` (from user config) must themselves be realpath-resolved and compared component-wise (`/private/tmpfoo` is not under `/private/tmp`); say so.

### F16. minor: doctor check (d) does not test the case that matters
**Claim.** Decision 11: child holds `flock(LOCK_EX)`, parent probes with `LOCK_EX|LOCK_NB`.
**Evidence.** scan §4: Cargo uses `flock`, rustc uses `fcntl` locks, and on this macOS the two conflict. The deletion gate (CORE-1.3: lock probe disables every deletion) exists to detect rustc's `fcntl` locks via a `flock` probe. A `flock`-holds/`flock`-probes test passes even where cross-syscall conflict is broken. scan §4 itself recommends a child holding an `fcntl` lock. Also unspecified: how the child is spawned (hidden subcommand of the binary? `sh`?), and `Fs::try_lock_exclusive` as a trait method cannot express the holder. Production probe is `LOCK_SH` on a read-only fd, which the doctor does not mimic.
**Fix.** Run both holders (flock and fcntl) and probe with `flock(LOCK_SH|LOCK_NB)` on a read-only fd; define the holder as a hidden `cargo-hive __hold-lock <kind> <path>` subcommand killed and reaped on guard drop; add `Fs::hold_lock(kind)` to the trait.

### F17. minor: smaller correctness and fit issues
1. Hand-edited registry (Out of Scope says editing the registry by hand is the v1 unregister path): the design does not say what a malformed or hand-edited file does. It must fail closed (refuse, name the file), never "treat as empty and overwrite". Add it to `state`.
2. `capabilities.json` is not tied to a toolchain identity; after a rustup upgrade the (f) verdict is stale (D4 says "per toolchain"). Record the `rustc -vV` release in `Capabilities` and have `require` treat a mismatch as "run doctor".
3. `HIVE_HOME` relocates the state root while CORE-2.6, 6.3, 5.2 name `~/.hive/`. Fine as a test hook; say in design that it is documented as test-only, and that it is the only environment variable hive reads for scope (CORE-5.7/A6 remain about cwd).
4. Doctor's `CompressOutcome` has three values, but Applesauce has ten `SkipReason`s plus `not_compressible_enough` and `Stats`; map the others to `Failed(reason)` and make the fixture compressible (scan used 3.6 MB, ratio 0.95).
5. `Fs` has no way to create fixture content, make a hardlink, set mtime or compare bytes, which doctor (a)(b)(c) need; doctor will use `std::fs` directly, so the fake `Fs` cannot drive those checks. Add `write_file`, `hardlink`, `set_mtime`, `read` or drop the claim that `doctor::run` is unit-testable with fakes.
6. Decision 2 says `cargo install` "fails nowhere": `applesauce` pulls two C libraries (`lzfse-sys`, `libdeflate-sys`) needing a C compiler (scan §3a); I1 says "no dependency beyond a Rust toolchain". On macOS the linker already needs Xcode CLT, so benign, but state it.
7. The active `cargo` here is a mise shim; `rustup which cargo` through the shim worked. Whether the mise shim writes its own state (outside `~/.cargo`/`~/.rustup`) during `cargo metadata` was not checked. CORE-6.3 says "only under `~/.hive/`": that holds for hive's code, not for third-party shims. UNVERIFIED.
8. Doctor's `rustup which cargo` is run in the process cwd; a `rust-toolchain.toml` in cwd changes "the active toolchain" and makes doctor cwd-dependent (the spirit of CORE-5.7). Run it with cwd set to the temp dir.

### F18. nit
- Decision 9 calls `FileCompressor::recursive_compress(...)` as an associated function; it needs `FileCompressor::new()` and `&mut self`.
- The close-package item I4 check list matches CORE-1.1; no contradiction found.
- Receipt schema (Decision 12) has no slot for `unsupported` profile findings; they must go in `counts` or `notes` (F14).

## Structure checks (Part 3)

**Coverage (re-derived from requirements.md).** Live IDs: 1.1-1.7; 2.1-2.9; 3.1-3.8; 4.2, 4.3, 4.5-4.8, 4.10-4.15 (4.1, 4.4, 4.9 struck); 5.1-5.7; 6.1-6.5; 7.1, 7.2. Each appears in exactly one `Satisfies:` line; the coverage table matches; no stray or duplicate IDs. Seams table: every live ID is in at least one row (but see F11 for 1.1). Struck IDs are correctly excluded.

**Interface smaller than implementation.** state, git, registry, discover, cargo_target, fingerprint, size, platform, doctor, receipt: yes. classify: interface about equals implementation by the author's own admission ("nothing else to hide"); acceptable for a safety core, but it is not a deep module and the Depth line should say so rather than citing the deletion test. cli: n/a.

**Rung-7 Depth sentences name what callers keep knowing.** All do (registry, classify, doctor, fingerprint, receipt). None vacuous. Weakest: classify (restates the rules).

**Locality.** Matches except F15.5 (state's "only writer" claim) and F15.6 (3.3 sits in classify but is done in discover).

**Data flow that skips a component.** F15.2 (size) and F15.4 (cli to doctor missing). Also: no component calls `Git::is_shallow` except registry; fine.

**Names used two ways.** `Refusal` (F15.1); `Kind` (Applesauce compressor kind vs fingerprint kind; keep module-qualified); `unsupported` (platform `unsupported.rs` vs fingerprint `Unsupported`); "Layout" fine. Glossary terms used consistently: Checkout, Protected/Secondary checkout, Secondary root, Cargo target dir, Profile dir, Build directory, Unit, Fingerprint format signature, Orphan target (design says `OrphanCandidate`; glossary defines "Orphan target" as the dir; "candidate" is requirements' wording, fine).

## Verdict table

| # | Area | Sev | Verdict |
|---|---|---|---|
| F1 | Manifest dir for `cargo metadata` undefined | blocker | Fix before approval |
| F2 | No lock on registry/capabilities read-modify-write | major | Fix |
| F3 | Receipt append not safe under SIGKILL; ftruncate races | major | Fix; reword 6.5 claim |
| F4 | `cargo metadata` lacks `--locked --offline` and `RUSTUP_AUTO_INSTALL=0` | major | Fix (7.1/6.3) |
| F5 | Doctor (f) env not scrubbed | major | Fix |
| F6 | `doc-` hash files missing from signature (refuted by experiment) | major | Fix |
| F7 | Failed resolution becomes orphan | major | Fix |
| F8 | Proof machine's klynt cache target reported plain orphan | minor | Add caller-env hint |
| F9 | "GC never invokes cargo" holds only by narrow reading | major | Owner confirmation or snapshot |
| F10 | CORE-1.5 "every exit path" overclaimed; SIGINT | major | Reword and handle signals |
| F11 | CORE-1.1 and 1.6 not tested on real platform | minor | Add to seams |
| F12 | CORE-6.1 budget ignores metadata cost | minor | Measure/parallelise/cache |
| F13 | Missing `build_directory` fallback | minor | Add |
| F14 | CORE-4.14 "in hive's state" has no store | minor | Add file or amend req |
| F15 | Naming and flow inconsistencies | minor | Fix text |
| F16 | Lock probe test misses fcntl/flock case | minor | Fix |
| F17 | Hand-edited registry, stale capabilities, others | minor | Fix |
| F18 | Wording | nit | |

Overall: **not ready for approval** (1 blocker, 7 majors). Everything the design asserts about libc, git, `cargo metadata` fields and the toolchain env mechanism for check (f) holds; the fixes are mostly additions to child-process policy, locking, and one missing step (finding the manifest dir).

## Unverified and remaining

- `applesauce` 0.8.10, `objc2-local-authentication` 0.3.2, `objc2` 0.6.5 sources: not in the local registry; relied on the scan's compile-and-run evidence.
- `canEvaluatePolicy` with no GUI session (CORE-6.4); LA "no dialog" cannot be observed from an agent shell.
- `cargo metadata --no-deps` with stale or malformed `Cargo.lock`, `[patch]`, or a `rust-toolchain.toml` with auto-install enabled: untested (not run, because it could write).
- Real-`CARGO_HOME` run of `cargo metadata` (only fresh-home and req-review's snapshot evidence).
- mise shim side effects; `rustup which` with an uninstalled override and auto-install enabled.
- klynt timing with per-manifest metadata calls (F12).
