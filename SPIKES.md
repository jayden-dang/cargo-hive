# cargo-hive: assumption spikes

*2026-10-25 · klynt `backend/` (597 packages, 42 workspace crates) · M5 Pro 18 cores · rustc/cargo 1.97.1*

Before building cargo-hive we tested the assumptions its architecture rests on. Everything ran on clones of a klynt snapshot under `/private/tmp/hive/wt-*`, which stand in for worktrees. Scripts are in [`spikes/`](spikes/).

**On timings.** The machine was shared with live klynt agents (other worktrees running `cargo watch` and integration tests) plus Docker. Load average swung between 3 and 38, so wall-clock times are noisy. The decisive signals are deterministic ones: byte equality, units compiled, rustc's own incremental log, and verifier results.

## Summary

| # | Assumption | Verdict | Key evidence |
|---|---|---|---|
| 1 | Outputs can be made path-independent across worktrees | **PASS**, with 3 conditions | 933/933 registry artifacts and 31/31 workspace rmeta byte-identical; workspace rlibs differ only in per-session object names |
| 2 | Incremental state can be seeded into a new worktree | **PASS**, with 5 conditions | New worktree's first build: 45 s / 31 crates, vs 75–114 s / 466 crates from scratch; later edits run at normal incremental speed; `-Zincremental-verify-ich` audit is clean |
| 3 | One machine-wide jobserver can cap all worktrees | **PASS** | Unpooled: 36 concurrent rustc on 18 cores. Pooled with 16 tokens: max 18, exactly the cap. 0-token boundary test: exactly 1 job per Cargo |
| 4 | Undeclared inputs (proc macros) can be observed | **PASS** | One `persistence` compile read **1,069 files not in dep-info**, including 1,024 `.sqlx/*.json`, `.env`, and the `Cargo.toml`s of a `cargo metadata` that sqlx spawns |
| 5 | Choose an APFS compressor: Applesauce vs afsctool | **Applesauce** | Applesauce: 2.15–2.45×, 0 mismatches, mtimes and hardlinks kept, no steady-state slowdown. **afsctool zeroes every hardlinked file** and crashes on LZFSE/LZVN |
| 6 | A seed helps the commands agents run first | **PARTIAL** | Exact unit match: 3.7–5× faster. Mode mismatch: no help. `-p` feature mismatch: deps only (−29%). Covering 4 shapes grew the seed 5×. Compress-then-clone confirmed (clone-then-compress costs +10.2 GB) |
| 7 | A deps-only seed holds in a foreign checkout | **PASS** | 0 non-workspace units rebuilt; `build` 77.1 → 56.2 s, `nextest -p` 74.2 s vs 112.6 s cold |
| 8 | GC by keep-set reachability is safe and worth it | **PASS, with fixes** | Real 69 GB target: 31.4 GB live, **41.7 GB garbage (57%)**. After GC only bin relinks (liveness must come from fingerprint dirs); a keep command that fails silently would delete live state, so GC must be verify-gated |
| 9 | GC liveness from disk, without replay | **PASS** with Cargo's fingerprint graph | 24,855/24,855 edges resolved (little-endian hash). 6 h window: 43 rebuilt (all window gap) vs 1,752 roots-only. `.d` has no binary deps (my error). Never delete fingerprint dirs on their own (1,086-unit cascade) |
| 10 | Auto-tier preconditions: locks and temp files | **PASS** | Cargo locks and rustc session locks can be probed non-blocking; rustc stages `rmeta*` temp dirs in `deps/` mid-compile, so the lock is required |
| 11 | Measure bytes freed by store eviction | **PASS** via flags and clone id | `PRIVATESIZE` reads 0 for every compressed file (useless for the store). `EXT_FLAGS` `SHARES_ALL` plus `CLONEID` (a rewrite gives the clone a new id) identify shared vs freeable bytes |
| 12 | Seeded build-script paths after the source is deleted; wrapper needed? | **PASS**, no wrapper needed | Y builds with X deleted (0 deps rebuilt); binary has 0 refs to X; Cargo re-roots `output` paths on read. `DEP_*` re-rooted too (addendum) |
| 13 | Discovery cost and target-dir identity | **PASS**, with a rule | `git worktree list` 39 ms; tag lookup 53 ms. `CACHEDIR.TAG` is generic (moon uses it): require "created by cargo" + a profile dir with `.fingerprint/` |
| 14 | Cargo detects a content-swapped artifact | **NO** (by design, confirmed) | Corrupted rlib with preserved mtime stays "Fresh"; a dependent compiles against it silently. Only deletion is self-detecting |
| 16 | Hive can delete sessions and temp entries under the owner's lock, using flock only | **PASS** on macOS 27 | Every holder kind (flock SH/EX, fcntl RD/WR) blocks a non-blocking flock EX, and hive's flock EX blocks rustc-style fcntl RD/WR tries. Opening+closing a second fd drops a held fcntl lock but not a flock. 12 builds with a concurrent GC: 0 failures, 0 warnings, 26 superseded sessions deleted, 739/739 `-working` skipped, then 0 units compiled. `.cargo-build-lock` was free only in a cargo process's first ~30 ms; temp entries while it was free: 0/2,215 samples |

## 1. Path independence

**Method:** build `server` in two worktrees through a `RUSTC_WRAPPER` that adds `--remap-path-prefix`, then compare every artifact byte for byte (`spikes/cmp-targets.py`).

Things that leaked an absolute path, and the fix for each:

| Leak | Fix |
|---|---|
| macOS `/tmp` is a symlink to `/private/tmp`. Cargo passes canonical *source* paths, but `OUT_DIR` exactly as configured | Canonicalize the target dir at the source (the tool sets `CARGO_TARGET_DIR` itself), so there is one spelling |
| Build-script output (`include!(concat!(env!("OUT_DIR"), …))`) | Remap the target dir as well as the worktree root |
| Proc-macro dylibs: ld64 writes the output path as `LC_ID_DYLIB` and absolute `.o` paths in the debug map; `LC_UUID` and the ad-hoc signature hash both | `-Wl,-install_name,@rpath/<lib>` and `-Wl,-oso_prefix,<target>`. ld64 honours only the last `-oso_prefix`, which is one more reason for a single canonical spelling |

**Result:**

- All 449 registry rlibs, 449 rmeta, and 35 proc-macro dylibs are byte-identical.
- All 31 workspace rmeta are byte-identical.
- The 30 workspace rlibs contain 3,790 object files that are all identical. The only differing member is `lib.rmeta-link`, which records per-session object names.
- On a minimal crate with incremental off, all 18 artifacts are identical.

## 2. Seeding incremental state

**Method:** APFS-clone a built target dir into a fresh worktree, then build. Diagnose with `-Zincremental-info` and `-Ztime-passes`.

Each condition below was found by experiment. Breaking any one of them silently discards the cache.

1. **Identical `--remap-path-prefix` lists in every worktree.** rustc tracks these arguments for incremental reuse. The tool therefore owns a fixed pool of worktree slots and always passes remaps for all of them.
2. **Fixed rustc working directory.** Isolation tests (`spikes/isolate.py`) showed cwd is tracked; `--out-dir`, `-C incremental`, `-L`, and `--extern` paths are not. Cargo runs rustc from the workspace root, so the wrapper makes `.rs` inputs absolute and runs rustc from a fixed directory.
3. **Full-scope remapping.** With `--remap-path-scope=object` the session was accepted, but almost every node went red: the unremapped source names still feed span hashes. Remapping all scopes fixed it.
   - Diagnostics stay readable by remapping the workspace root to the *empty* prefix. Errors then show `crates/x/src/lib.rs`, exactly like stock Cargo, and are identical across worktrees.
   - Re-verified on klynt (see "Resolved since first draft").
4. **Consistent environment.** `RUSTC_BOOTSTRAP` is tracked. A single stray diagnostic run poisoned the next normal build: 30 s instead of 9 s.
5. **First build in a worktree pays a one-time ~20 s.** Seeded first build after a `persistence` touch: 25 s. The 2nd and 3rd builds: 5.4 s and 5.0 s, the same as the source worktree (5.0 s). The penalty comes from the first load of the cloned proc-macro dylibs, which macOS validates as new files, and from one codegen pass.

**Results:**

| Scenario | Time | Crates compiled |
|---|---|---|
| New worktree, unseeded, first build | 75–114 s | 466 |
| New worktree, seeded, `git worktree add` (all mtimes new), first build | **45.5 s** | 31 |
| Seeded worktree, edit `domain` afterwards | **7.2 s** | 29 (normal incremental) |
| Seed cost (APFS clone of 6.4 GB target) | **1.9 s** | — |

**Correctness:**

- Isolation: an edit in the seeded worktree was not visible in the source worktree, and Cargo in the source reported 0 units to rebuild.
- Byte equality with a clean build is **not** a valid criterion. A never-seeded incremental build *also* differs from a clean build in 19 crates' rmeta, because rustc's incremental output depends on history.
- The right criterion is semantic. A seeded worktree with new mtimes plus an edit rebuilt 31 crates under `-Zincremental-verify-ich`, which recomputes every cached result and compares fingerprints. There were **0 mismatches**, and the flag was confirmed to reach rustc.

## 3. Global jobserver

Two clean builds ran concurrently in two worktrees (`spikes/jobserver-test.sh`), counting only spike rustc processes.

| Mode | Max rustc | Avg rustc | Wall |
|---|---|---|---|
| No pool (each Cargo uses `-j18`) | **36** | 10.0 | 319 s |
| Shared fifo pool, 16 tokens | **18** (= 16 + 2 implicit) | 8.0 | 360 s |
| Shared fifo pool, 0 tokens | **2** (1 per Cargo) | — | — |

- Cargo 1.97 joins a GNU-make-4.4 `fifo:` jobserver through `CARGO_MAKEFLAGS` and honours it exactly. rustc's own codegen threads draw from the same pool.
- Wall time is inconclusive: load was 16–38 from builds *outside* the pool. That itself is the point — the cap only helps if hive owns every build on the machine.
- An early run counted the user's own rustc processes and reported a fake "max 24". The sampler must filter by owner.
- Next step: memory-aware admission. A `persistence` rustc peaks at ~2.4 GB.

## 4. Observing undeclared inputs

**Method:** a 60-line interposer (`spikes/trace_open.c`) injected with `DYLD_INSERT_LIBRARIES` records every `open`/`openat` made by rustc and the proc-macro dylibs it loads.

- rustc from rustup is ad-hoc signed without hardened runtime, so injection works.
- `DYLD_*` is **stripped when exec'ing SIP-protected binaries** like `/bin/bash`. Inject only at the final exec of rustc.

One `persistence` compile:

- 2,021 files opened for reading; 1,412 in worktree sources.
- dep-info declares only 343.
- **1,069 undeclared worktree inputs:**
  - 1,024 × `backend/.sqlx/query-*.json`
  - `.env`, probed from the crate dir up to `/` (dotenvy)
  - `.cargo/config*`, probed up to `/`
  - every workspace `Cargo.toml`, plus `~/.gitconfig` and `/dev/tty`, because sqlx-macros spawns `cargo metadata`, and the tracer follows it into the child process
  - `/dev/urandom`

**Implications:**

- A cache keyed on declared inputs (sccache's model) would serve stale results for klynt. Keys must include observed reads **and negative probes**: creating `/private/tmp/.env` could change the output.
- There is a latent klynt bug of the same kind. Cargo doesn't rebuild `persistence` when `.env` (e.g. `SQLX_OFFLINE`) or a `.sqlx` file changes.
- Reading `/dev/urandom` should flag a crate as potentially nondeterministic, so the audit can sample it more often.

## 5. Applesauce vs afsctool

**Corpus:** a real klynt target dir with 29,832 files (6.69 GB physical, 9.96 GB logical). **23,537 files are hardlinks** (incremental `.o` ↔ `deps/`). Each variant got a fresh `ditto` copy (which preserves hardlinks), was compressed, then checked byte for byte against the original (`spikes/compress-bench.py`).

| Variant | Exit | Wall | CPU | Peak RSS | Physical size | Byte mismatches | mtimes kept |
|---|---|---|---|---|---|---|---|
| afsctool LZFSE `-J18` | **SIGBUS** | 6 s | 16 s | 566 MB | 6.21 GB (partial) | **7,675** | 19,471 / 29,832 |
| afsctool LZVN `-J18` | **SIGBUS** | 4 s | 7 s | 241 MB | 6.56 GB (partial) | **3,400** | 23,911 / 29,832 |
| afsctool ZLIB-5 `-J18` | 0 | 38 s | 191 s | 2,034 MB | 3.32 GB | **23,505** | **226** / 29,832 |
| Applesauce LZFSE | 0 | 14.5 s | 69 s | 128 MB | **3.11 GB (2.15×)** | **0** | all |
| Applesauce LZVN | 0 | 13.1 s | 98 s | 124 MB | 3.32 GB (2.01×) | **0** | all |
| Applesauce ZLIB-5 | 0 | 9.1 s | 74 s | 136 MB | **3.08 GB (2.17×)** | **0** | all |
| Applesauce LZFSE `--verify` | 0 | 7.8 s | 80 s | 144 MB | 3.11 GB (2.15×) | **0** | all |

Wall times are noisy because load average was 4–26. CPU and RSS are the stable comparison.

**Root cause of the afsctool corruption, reproduced in isolation:**

- On a 522-file directory with 512 hardlinked files, afsctool ZLIB returned **rc=0 and left all 512 hardlinked files at 0 bytes**.
- The same directory without hardlinks came out intact.
- This is silent data loss on exactly the file shape Cargo produces.

**Build impact of Applesauce:**

- The seeded target went 7.45 → **3.04 GB (2.45×)** in 9 s with `--verify`.
- No-op builds stayed at 0 units, because mtimes are preserved and Cargo's fingerprints are undisturbed.
- Steady-state incremental builds were unchanged (`domain` edit 7.5 s vs 7.4 s; `persistence` 6.9 s vs 6.9 s). The first post-compression build paid ~2 s.
- Decompressing read-back runs at ~0.7–0.8 GB/s, vs 2.2 GB/s uncompressed. That is irrelevant at rustc's read volumes, and the page cache absorbs repeats.
- rustc rewrites rebuilt artifacts uncompressed (3.04 → 5.22 GB after 6 rebuilds), so compression must be continuous: re-compress cold files at idle.

**Ratios caveat:** every ratio in this section (2.01–2.45×) was measured on **single-link files only**. The 23,537 hardlinked files were skipped. The estimates built on these ratios (klynt about 3.5 GB compressed, the 40 GB store budget default) must be re-measured on store-shaped (single-link) data in ROAD-8.

**Decision: Applesauce, embedded as a library.** The `applesauce` crate is 0.8.10, published 2026-09-27; the CLI is 0.5.30. The deciding factors:

- It never corrupts hardlinked files, but only because it **skips** every file with link count > 1 and leaves it uncompressed (see the correction below and `req-review.md` §9). It does not compress hardlinked files at all.
- It preserves mtimes.
- It verifies.
- It uses about 1/15 of afsctool's memory.
- It parallelizes at the block level.
- It is Rust.

LZFSE is the default. ZLIB-5 is marginally smaller at similar CPU. afsctool is disqualified; uninstall it (`brew uninstall afsctool`).

## Resolved since first draft

- **Diagnostics under full remap.** Remapping each slot's workspace root to the empty prefix (and its repo root to `../`) makes rustc report `crates/x/src/lib.rs`, the same as stock Cargo. Re-verified on klynt:
  - rmeta stores `crates/shared/domain/src/lib.rs`;
  - seeded first build is 20–23 s for 31 crates;
  - incremental edits run at normal speed.

## Open items for the prototype

- **Debugger source lookup.** With rustc run from a fixed cwd, `DW_AT_comp_dir` is that directory, so lldb needs a per-worktree `target.source-map`. Generate it.
- **Slot pool size is fixed.** Changing the slot list invalidates every incremental session. Pick a generous N up front.
- **Memory-aware admission** for the jobserver.
- **Negative-dependency tracking:** record failed `open` probes as part of the key.
- **Linux equivalents:**
  - `LD_PRELOAD` (or seccomp/fanotify) for the tracer;
  - reflinks (btrfs/XFS) for the clones;
  - btrfs/ZFS transparent compression;
  - fifo jobserver works as-is.

## 6. Does a seed help the commands agents actually run first?

**Question.** Agents usually start a new worktree with `nextest -p`, `check -p --tests`, or `clippy`/`check --workspace` (from the transcripts, `evidence/build-concurrency.py`). Spike 2 proved seeding only for `build --bin server`. Does a seed help these first commands?

**Method.** Each trial starts a fresh slot with new mtimes (as after `git worktree add`), APFS-clones a seed into it, and runs one first command. Same wrapper as spike 2. Two seeds:
- **S1**: only `build --bin server`.
- **S2**: a "typical main" checkout: build server, then `clippy --workspace --all-targets --all-features`, then `test --workspace --all-features --no-run`, then `check --workspace --all-targets`.

| First command | No seed | Seed S1 | Seed S2 |
|---|---|---|---|
| C1 `nextest run -p gateways --test it --no-run` | 112.6 s / 464 units | 80.1 s / 54 | 81.0 s / 54 |
| C2 `check -p assistant_turn_service --tests` | 43.0 s / 373 | 46.5 s / 360 | **34.1 s / 42** |
| C3 `clippy --workspace --all-targets --all-features` | 106.2 s / 518 | 111.0 s / 499 | **28.9 s / 42** |
| C4 `check --workspace --all-targets` | 97.4 s / 510 | 91.0 s / 490 | **19.5 s / 42** |

Seed sizes as logical `du`: S1 5.03 GB, S2 **26.37 GB**. Load average at the end was 13–17.

**Answer.** A seed helps only as much as its units match the command exactly: profile, mode (check, build or clippy) and **resolved feature set** all have to agree.
- **Exact match** (C3, C4 with S2): **3.7–5× faster**, and only the 42 workspace crates re-run.
- **Mode mismatch** (S1 is `build`; C2–C4 are `check`/`clippy`): **no help at all**, because metadata-only units are different units.
- **Feature mismatch** (C1 with either seed): only registry dependencies are reused (464 → 54 units). The workspace crates rebuild from scratch, so the gain is just **29%**. `-p gateways` without `--all-features` resolves a feature set that no workspace-wide command produces. That is feature unification again (RESEARCH §2.4, `-Zfeature-unification`). This confirms the user's guess.
- **Coverage costs disk.** Covering four command shapes made the seed 5.2× bigger (5.03 → 26.37 GB).

**Disk: compress vs clone order (real seed S2, `df` deltas).** The user's rule is confirmed at scale:

| Order | Step | Effect |
|---|---|---|
| Clone then compress | clone | +27 MB |
| | compress the clone | **+10,222 MB**, because compression rewrites the blocks and breaks sharing |
| Compress then clone | compress the seed | **−13,353 MB** |
| | clone | +34 MB |

**Consequences for design.**
- What gets seeded must be **learned from the commands actually run**, not fixed to `build --bin server`.
- Feature-set divergence (`-p` vs `--workspace`, `--all-features` vs default) caps the benefit of seeding until it is normalized. A repo-side fix would be hakari or `resolver.feature-unification`, which belongs to the build-optimization skill direction.
- Disk and seeding pull against each other. The seed should be **compressed once, then shared by clone**, never compressed after cloning.
- Run-spike script deleted after recording. Its log is reproduced above.

## 7. Does a deps-only seed hold in a foreign checkout?

**Question.** v1 locks "foreign mode": no slot pool. Each checkout remaps only to its own root and gets a dependency-only seed. Does such a seed survive a different path?

**Method.**
1. Checkout X builds `build --bin server` and then `nextest run -p gateways --test it --no-run` through a wrapper that remaps only X's own root.
2. Checkout Y sits at a different path, outside any slot list, with fresh mtimes. It gets X's target dir by APFS clone, minus every workspace unit (7,597 entries).
3. Workspace units were identified by **unit hash** from Cargo's JSON messages, not by name. klynt's workspace crate `config` collides with the registry crate `config` 0.15.
4. Rebuilt units are counted from Cargo's own output, with the fingerprint log enabled.

| Run | Wall | Units compiled | Non-workspace rebuilt |
|---|---|---|---|
| X `build --bin server` (cold) | 77.1 s | 466 | 434 |
| X `nextest -p gateways --test it --no-run` | 80.3 s | 51 | 25 (`-p` resolves different features for tokio, hyper, sqlx…) |
| **Y `build --bin server`, foreign, deps-only seed** | **56.2 s** | 31 | **0** |
| **Y `nextest -p gateways --test it --no-run`** | **74.2 s** | 26 | **0** |

Load average was about 2.2–3.

**Answer: yes.** A dependency-only seed is fully reused in a foreign checkout.
- No registry unit rebuilt, including the ones with build scripts.
- `build` was 27% faster than cold (77.1 → 56.2 s).
- `nextest -p` was about 34% faster than spike 6's cold C1 (112.6 → 74.2 s; different run, similar load).
- The remaining time is the workspace crates compiling cold, dominated by the serial `persistence` → `gateways` chain. Crate shape is the lever there (build-optimization skill direction), not hive.
- The "29 units rebuilt after moving a target dir" from RESEARCH.md was pre-wrapper and does not reproduce under this setup.
- Run-spike script deleted after recording.

## 8. GC: how much of the real target is unreachable, and can it go with zero rebuilds?

**Question.** klynt's real `backend/target` was 69 GB. How much of it is unreachable from the commands actually run, and can that part be deleted without causing rebuilds?

**Method.** Everything ran on an APFS clone of the real repo and target; the real checkout was only read.
- The **keep-set** is the lanes plus the most common agent first command:
  - `build --bin server`
  - `clippy --all-targets --all-features`
  - `test --workspace --all-features --no-run` (the unit set nextest builds)
  - `test -p gateways --test it --no-run`
- R1 touched every workspace source and ran the keep-set, which marks the live incremental dirs.
- **Live set** = unit hashes from Cargo's JSON on a fully fresh pass, plus the incremental dirs present after convergence. Everything else is garbage.
- Real-target sizes were counted read-only and inode-deduplicated: `cp -c -R` breaks hardlinks, so the clone reported 91.6 GB for a 69 GB target.

**Real target, live vs garbage:**

| Area | Live | Garbage |
|---|---|---|
| `deps/` | 13.80 GB | 17.47 GB |
| `incremental/` | 17.33 GB | **24.16 GB** |
| `build/` + `.fingerprint/` | 0.23 GB | 0.04 GB |
| **Total** | **31.36 GB** | **41.66 GB (57%)** |

**Safety.**
- A fresh pass before GC rebuilt 0 of 2,766 units.
- After GC, 20 units rebuilt, and **all 20 were bin targets (relink only)**. Cargo's JSON reports bins by their uplifted path (`debug/migrate`), which has no unit hash, so the live set missed `deps/<bin>-<hash>` and `.fingerprint/<bin>-<hash>`. Production GC must derive liveness from the fingerprint dirs, not only from JSON filenames.
- Apart from that, GC caused no rebuilds.
- A first, buggy run is worth recording. `--message-format=json` was placed after `--`, so clippy never ran, and GC deleted every clippy unit (499 rebuilt afterwards). **A keep-set that silently fails to run deletes live state.** Production GC needs a verify pass before and after, and must refuse to delete when any keep command did not run successfully.

**Cross-command convergence.** The first pass after deleting the clippy units rebuilt more (`StaleDepFingerprint`) as commands sharing units settled. The second pass was clean. There is convergence, but no persistent thrash between the lanes.

**Side finding (klynt).** `clippy --workspace --all-targets`, which is what rust-analyzer runs per `.vscode/settings.json`, **always fails** (rc=101) and recompiles 44 units every run. It is the same cause as `cargo build --workspace` failing: `continuity_service` needs `testkit`. Fix it in klynt (add `--all-features` to RA's check args, or fix the feature). It is also a reason RA can't share units with the lint lane.

**Consequences for design.**
- About **57% of the real target is reclaimable**.
- The live floor for these four commands is about 31 GB per checkout. That is before compressing the registry part, and compression applies in the store, not in live checkouts.
- GC is keep-set-driven and verify-gated. Liveness comes from fingerprint dirs, including bins. Any keep command that fails to run aborts the deletion.
- Run-spike script deleted after recording.

## 9. GC liveness from disk, without replay

**Question.** The user's model: GC never invokes cargo. Roots are the units compiled inside a time window. Liveness is the closure from those roots over dependency edges. Does that model hold up?

**Method.** APFS clones of the real repo and target; the real target was only read.
1. Run a baseline keep-set pass (normal activity).
2. GC with the candidate rule.
3. Verify with the spike-8 keep-set. Every command must exit 0 and emit artifacts, or the run aborts (see "self-inflicted" below).

The real target was rebuilt about 18 h before this spike: every `invoked.timestamp` is 0.8–17.8 h old. So windows of 1 day or more see everything as a root, and 6 h and 12 h windows are used to exercise the rule.

**Findings, in order:**

1. **rustc's `.d` dep-info does not list rlib/rmeta/dylib inputs** unless `-Zbinary-dep-depinfo` (nightly) is set. This corrects my claim during the interview. A closure over `.d` files found no edges at all (683 roots → 683 live).
2. **Roots-only liveness fails badly.** Anything not compiled inside the window is deleted, including registry deps that are used fresh every day. With a 6 h window, 1,752 units rebuilt. In steady state with a 7-day window, that means re-compiling the dependencies weekly. **Edges are mandatory.**
3. **Deleting fingerprint dirs on their own cascades.** 62 build-script fingerprint dirs had no timestamp and no edge. Deleting them caused **1,086 rebuilds** while freeing 0.02 GB. **Rule: delete only `deps/` artifacts of dead units, plus the fingerprint dir with the same hash. Never delete a fingerprint dir independently. Never touch `build/` (≈0.2 GB).**
4. **Cargo's fingerprint JSON is a complete on-disk dependency graph.**
   - Each unit's `deps` entries are `[pkg_hash, name, public, dep_fingerprint_u64]`.
   - The dependency's short hash file (`lib-<name>` and siblings) holds that same u64 as **little-endian hex**.
   - **24,855 of 24,855 edges resolved uniquely.**
   - It covers history (units compiled before hive existed) and every caller, including rust-analyzer's own `RUSTC_WRAPPER` pass, because Cargo writes it regardless of wrapper.
   - Cost: this is Cargo-internal and unstable. hive must version-check it per toolchain and refuse GC on an unknown shape.
5. **Fingerprint-graph closure is safe up to the window gap.** With a 6 h window: 648 roots → 2,429 live of 2,840 units; 403 dead units deleted.
   - Verify rebuilt **43 of 2,764 units**.
   - 40 of them were deleted units: keep-set units neither compiled in 6 h nor reachable. That is the user's known gap: "a command fully fresh for the whole window has no root".
   - The other 3 were bins, which JSON reports without a hash, so they are most likely the same gap.
   - **No unexplained cascade.**
6. **Real target live vs garbage by window** (read-only, inode-deduplicated):

   | Window | Roots | Live units (of 2,812) | Live | Garbage |
   |---|---|---|---|---|
   | 6 h | 483 | 2,083 | 39.17 GB | 33.59 GB |
   | 12 h | 635 | 2,235 | 45.39 GB | 27.37 GB |
   | 24 h | 2,730 | 2,812 | 72.76 GB | 0.00 GB |

   Steady-state reclaim at a realistic window (7 d) **cannot be measured on this target**, because it is younger than the window. Spike 8's 41.7 GB "garbage" was all compiled within the last 18 h by commands outside its keep-set.
7. **rust-analyzer** defaults to `cargo.buildScripts.useRustcWrapper = true`, which sets `RUSTC_WRAPPER=rust-analyzer` for its build-script pass and so bypasses a wrapper configured in `.cargo/config.toml` (rust-analyzer docs, `configuration_generated.md`). Disk-based liveness is unaffected. Anything that needs hive's wrapper on every rustc call is affected.

**Self-inflicted (recorded so the tool never repeats it).** Two verify runs silently did nothing.
- First: `--message-format=json` was placed after `--`.
- Second: `cargo $k` under zsh does not word-split, so cargo got one argument and printed "no such command".

Both read as "0 rebuilds". The final harness aborts unless every verify command exits 0 and emits artifacts. hive's verify step needs the same guard.

**Answer.** Use the user's model with Cargo's fingerprint graph as the edge source (not `--extern` logs, not `.d`). Delete artifacts only, never fingerprints independently, and never `build/`. The remaining risk is the window gap: a command whose units were all fresh for the whole window rebuilds once.

## 10. Locks and temp files during a live build (preconditions for the auto GC tier)

**Method.** Run a real `cargo build -p domain` into a fresh target and probe every 0.2 s (64 samples). The probes are non-blocking `flock` and `fcntl`, and any lock acquired is released immediately.

| Item | Observed |
|---|---|
| `.cargo-lock`, `.cargo-build-lock`, `.cargo-artifact-lock` | Held in 62 of 62 samples while building. Reported held by **both** `flock` and `fcntl` probes on macOS |
| Incremental `s-*.lock` (rustc session) | Held while that crate compiles (23 samples). Both probe styles report it held |
| `deps/rmetaXXXXXX` temp dirs | **34 appeared during the build**: rustc stages metadata there and then renames. 0 left after a clean finish |

**Answers:**
- rustc does create temp entries in the output dir mid-compile. Deleting `rustc*`/`rmeta*` without holding Cargo's lock would break a running build, so **taking the lock first is required**. The `rustcXXXX` entries found in the real target are leftovers from killed compiles.
- A non-blocking try-lock detects both owners, so GC can skip a held target or session and retry later without blocking.
- **Linux caveat:** BSD `flock` and POSIX `fcntl` locks don't conflict with each other on Linux. GC must use the same lock type Cargo and rustc use, and should check that against their sources before shipping a Linux port. macOS didn't distinguish the two in this test.
- Run-spike script deleted after recording.

**Superseded incremental sessions across checkouts** (read-only, inode-deduplicated; "unique" means not hardlinked elsewhere):

| Target | Old sessions, unique | Old sessions | All incremental |
|---|---|---|---|
| klynt main `backend/target` | **13.18 GB** | 15.08 GB | 49.17 GB |
| klynt `.worktrees/feat-grnd/backend/target` | **21.71 GB** | 25.77 GB | 66.49 GB |
| `work/CommandOss/mailgate/target` | **15.59 GB** | 19.73 GB | 51.14 GB |
| `work/zroute/target` | 5.37 GB | 7.94 GB | 17.17 GB |
| klynt `target/`, `~/.cache/cargo-target/klynt`, `porch/target` | 1.4–1.5 GB each | | |

That is about **60 GB in total**, or 25–33% of incremental space. Almost every crate dir holds 1 or 2 sessions, because rustc prunes the older one only when that crate next compiles. In 2 cases (both in `feat-grnd`) the newest session was `-working`.

**Auto-tier rule:** mirror rustc.
- Keep the newest *finalized* session.
- Keep any `-working` session whose `.lock` is held.
- Delete finalized sessions older than the newest finalized one.
- Delete `-working` sessions whose lock is free (abandoned).
- Never use "all but the newest by mtime": that deletes the last good session when the newest one is `-working`.

## 11. Measuring the bytes an eviction would free (APFS clones and compression)

**Question.** Store eviction must count only the bytes it would actually free. A store unit still cloned into a seeded worktree frees almost nothing (§6: a 13 GB compressed seed cloned for +34 MB). How can hive tell these cases apart?

**Probes:** `getattrlist`, with the `FSOPT_ATTR_CMN_EXTENDED` common-extended attributes.

| Case | `ATTR_CMNEXT_PRIVATESIZE` | `ATTR_CMNEXT_CLONEID` | `ATTR_CMNEXT_EXT_FLAGS` |
|---|---|---|---|
| Plain 50 MB file | 52,428,800 | own id | none |
| Plain file + clone | **0 / 0** (du counts the clone as 50 MB) | same id on both | `MAY_SHARE`, `SHARES_ALL` |
| Plain clone after a 5 MB write | 5 MB / 5 MB | — | — |
| Applesauce-compressed 20 MB file, never cloned | **0** (allocated 61,440 B, held in the resource fork) | own id | none |
| Compressed file + clone | 0 / 0 | same id on both | `MAY_SHARE`, `SHARES_ALL` |
| Clone rewritten whole (as rustc does) | original 0 · clone 20 MB | **clone gets a new id** | original keeps `MAY_SHARE` only (stale) · `SHARES_ALL` is gone |

**Answers:**
- **Private size works for plain files but is useless for compressed ones.** It reports only the data fork, and decmpfs keeps the compressed bytes in the resource fork.
- **The store is all compressed files**, so freed bytes must come from sharing flags plus clone ids:

  | Store file has | Bytes freed by eviction |
  |---|---|
  | no sharing flags | `st_blocks × 512` (resource fork included) |
  | `SHARES_ALL` | ≈ 0, a clone shares everything |
  | `MAY_SHARE` only | check the clones hive recorded at seed time. If none still carries the store file's clone id, freed = `st_blocks × 512` |

- rustc's whole-file rewrite of a seeded file breaks sharing and changes that file's clone id. That is exactly the signal for "this checkout no longer uses the store copy".

## 12. Seeded build-script paths after the source checkout is deleted; is a rustc wrapper needed?

**Question.** Build scripts write absolute paths into `build/<pkg>-<hash>/output`, for example `cargo:rustc-link-search=native=<X's target>/…/out` (aws-lc-sys, libwebp-sys). After a deps-only seed from X to Y, does Y silently link against X's files? Does it break once X is gone? Separately, does v1 need a rustc wrapper at all?

**Method.**
1. Build X with **no wrapper**.
2. Seed Y from X (deps only, workspace units removed by hash).
3. **Delete X entirely**.
4. Build Y, inspect its binary, and run it.

**Results.**
- Before Y built, **58** build-script `output`/`root-output` files in Y mentioned X's path.
- After X was deleted, Y built with **rc=0**: 31 workspace units compiled and **0 dependencies rebuilt**.
- Y's `server` binary references X **0 times** (`otool -L` and `strings`).
- The binary runs: telemetry starts, then it stops on a missing config key (`assistant.decide_timeout_secs`), not on a loader error.
- 7 `output` files still contain X's path in their stored text: `rustc-link-search` ×6, `include`, `root`, `lib_dir`, `rustc-env=MIME_TYPES_GENERATED_PATH`.
- Linking still succeeded with X gone. So Cargo re-roots the paths of a moved script's out-dir when it **reads** `output` (using `root-output`), and leaves the file itself as it was.

**Answers.**
- Deps-only seeding is correct even after the source checkout is deleted.
- **No rustc wrapper is required for v1.** The seed holds only registry units, which are path-independent for reuse, and GC reads the disk.
  - This removes the rust-analyzer `RUSTC_WRAPPER` bypass problem.
  - It removes the risk of poisoning incremental state through argument drift.
- **Residual risk (owned, untested):** a *dependent* build script that re-runs and reads `DEP_*` metadata, such as `DEP_AWS_LC_INCLUDE`, might receive X's path if Cargo doesn't re-root those values. To test: force a dependent build script to re-run in Y after X is gone.
- Run-spike script deleted after recording.

**§12 addendum: the `DEP_*` residual risk is closed.** In a seeded Y with X deleted, I forced the dependent build script of `aws-lc-rs` to re-run (removed its run-unit fingerprint). The `DEP_AWS_LC_0_44_0_*` values it received were **all re-rooted to Y**: `INCLUDE`, `LIBDIR`, `LIBCRYPTO_PATH` and `ROOT` all pointed at `<Y>/…/build/aws-lc-sys-…/out`. The build returned rc=0, and none of the new output mentions X. Cargo re-roots `DEP_*` metadata as well as link-search paths. Only `cargo:warning` text keeps the old path, which is cosmetic.

## 13. Discovery cost and identifying a Cargo target dir

- `git worktree list --porcelain` on klynt takes **39 ms**. A `CACHEDIR.TAG` lookup over main (depth 3) plus `~/.cache/cargo-target` (depth 3) takes **53 ms**.
- **`CACHEDIR.TAG` is a shared standard, not a Cargo marker.** The lookup also found `.moon/cache/CACHEDIR.TAG`, which carries the same signature (`8a477f597d28d172789f06886806bc55`) and only a different comment ("created by moon"). Treating every tagged dir as a target would have made moon's cache an orphan candidate.
- **Rule:** a dir is a Cargo target only if its tag comment says "created by cargo" **and** it has a profile dir (`debug/`, `release/`, or `<triple>/…`) containing `.fingerprint/`.
- Found along the way: `klynt/.cargo-targets/fix-attr` is another Cargo target, and therefore an orphan candidate.
- None of the user's tools make shallow clones (grep for `--depth`, `--shallow` and `--filter=blob` in build-on-host, isolate-workspace, deliver-issues and klynt scripts), so root-commit matching has no shallow-clone gap today.

## 14. Does Cargo detect a content-swapped artifact? (why hive must never replace files in place)

**Probe.** I built `-p domain` into a scratch target. Then I flipped 64 bytes in the middle of `libitoa-<hash>.rlib` (130,264 B), restored its mtime, and rebuilt.

| Step | Result |
|---|---|
| `cargo build -p domain`, no source change | `Finished` in 0.25 s. **Nothing rebuilt.** Cargo does not check artifact content |
| Touch a `domain` source so rustc must load itoa's rlib | `domain` **compiled successfully**: the corrupted bytes sat in object code, not metadata, so no error |

**Answer.** Cargo freshness rests on fingerprint hashes, mtimes, and whether outputs exist. It never looks at artifact content. A replaced artifact with a preserved mtime looks fresh. Its bytes reach downstream compiles and links silently, so the result is a **wrong build, not a rebuild**.

- **Deletion** is the only write Cargo detects on its own: a missing output triggers a rebuild.
- This backs the lock: on protected checkouts hive only deletes and reads, and never replaces files.
- Seeding is safe because each artifact travels **with its own fingerprint dir** (same unit hash), and the store copy is byte-identical to what produced that fingerprint.
- Any future "replace main's units with store clones" needs proof of byte identity before the swap.

## 15. Agent ancestry for `hive approve`

- **A command run by Claude Code's Bash tool** has this ancestor chain: `zsh -c …` → `claude` (comm `claude`, argv `claude`; the binary is `~/.local/share/claude/versions/2.1.310`) → `-zsh` → `tmux` → launchd. The user's own interactive shells also run under tmux, so ancestry matching must key on the agent CLI and never on tmux.
- **build-on-host's eight CLIs on this machine:**
  - Installed, and all native Mach-O arm64 rather than Node: `grok`, `codex` (0.164.0), `claude` (2.1.310), `kimi`, `opencode` (1.19.4).
  - Not installed here: `cursor-agent`, `gemini`, `qwen`. These are the likely Node-based ones (comm `node`), which is why matching is on argv. Unverified until installed.
- The `tty`/`isatty` probe inside Claude Code's Bash tool: no TTY on 0/1/2, and `/dev/tty` reports "device not configured". Codex is not logged in here, so it is unmeasured.

## Evidence correction: build concurrency including subagent transcripts

The interview's concurrency evidence (`evidence/build-concurrency.py`) first read only top-level session transcripts. Subagent transcripts live in `<session>/subagents/*.jsonl`: 870 under klynt's main project dir alone. That is where `build-in-waves` and `build-by-story` lanes run. Rerun with them included, 2026-09-24 → 2026-10-25:

| Metric | Top-level only | **All transcripts** |
|---|---|---|
| Build-like tool calls | 899 | **3,357** |
| Checkouts | 42 | **70** |
| Total build wall time | 8.1 h | **26.8 h** |
| Share of time with 1 / 2 / 3 / 4 checkouts building | 94.2 / 5.5 / 0.4 / — % | **95.0 / 4.3 / 0.7 / 0.0 %** (peak 4) |
| First build call in a new checkout | median 36 s, p90 105 s (n=41) | **median 37 s, p90 119 s (n=69)** |
| Max new checkouts in one day | 11 | **18** |
| Build hours by checkout kind | — | `.worktrees/` **16.46 h**, main 9.91 h, `.claude/worktrees/` 2.05 h |

**What changes and what doesn't.**
- Concurrency is still rare (≤ 5% of build time), so the jobserver lock stands.
- Worktrees now dominate build time. `build-in-waves` lane worktrees (`.worktrees/<branch>-lane<k>`, inside main) classify as secondary, so they are seeded and GC'd under the existing locks.
- The run is still blind to background runs, `cargo watch`, rust-analyzer, and terminal builds.

## Correction to §5 (2026-10-25, from `.skills/CORE/req-review.md` §9)

**Applesauce skips every file with more than one hardlink.** It never compresses them: `SkipReason::HardLink`, applesauce `src/threads/mod.rs:208-211`, confirmed on a fixture.

§5's "hardlink groups kept: True" is literally true, but only because those files were **never compressed**. The 23,537 hardlinked files in that corpus (incremental `.o` ↔ `deps/` pairs) stayed uncompressed. The 2.15–2.45× ratios therefore come from single-link files alone. The 7.45 → 3.04 GB seeded-target result likewise left hardlinked objects uncompressed.

**Design consequences:**
- Store files must be single-link copies (link count 1) to be compressible. The store fill must copy, not hardlink.
- Doctor's round-trip check uses a single-link file, and separately verifies that hardlinked files are left untouched.
- Applesauce does expose a library API (`FileCompressor::recursive_compress` / `recursive_decompress`), so embedding it (I1) stands.

## 16. Deleting sessions and temp entries under the owner's lock (ROAD-2)

*2026-10-28 · macOS 27, APFS · cargo and rustc 1.97.1 · scratch crate under `/private/tmp` (6,000 small functions, lib + bin) · throwaway Python using `fcntl.flock` / `fcntl.lockf` · load average about 3*

**Question.** ROAD-2 deletes rustc incremental sessions under each session's `s-*.lock`, and leftover `deps/rustc*` and `deps/rmeta*` under the build dir's `.cargo-build-lock`. The lock is held in hive's process from the try to the end of the delete. Open points:
- Can hive use one lock call (flock, non-blocking exclusive) for both kinds of lock file? rustc locks sessions with POSIX `fcntl` (`rustc_data_structures/src/flock/unix.rs` at rustc 8bab26f4f). Cargo locks `.cargo-build-lock` with std `File::lock` (flock on macOS) at cargo c980f4866, `core/compiler/layout.rs`.
- Does deleting during real builds break anything (S-2)?

**Q1. Conflict matrix, two processes, holder × non-blocking exclusive try.**

| Holder | flock `LOCK_EX\|LOCK_NB` | fcntl `F_WRLCK` `F_SETLK` |
|---|---|---|
| none | acquired | acquired |
| flock SH | blocked | blocked |
| flock EX | blocked | blocked |
| fcntl RD (rustc copying from a session) | blocked | blocked |
| fcntl WR (rustc's own `-working`) | blocked | blocked |

- Every try returned in ≤ 0.1 ms. After the holder died, every try acquired.
- Reverse direction: while hive holds flock EX, a rustc-style fcntl RD NB try is blocked, and so is an fcntl WR NB try.
- **The second-fd trap:**
  - A process that holds an **fcntl** lock and then opens and closes another fd on the same file loses the lock; another process then acquires it.
  - A **flock** survives the same sequence.
  - So flock-only removes the "one lock file, one fd" rule. This holds on macOS only: on Linux, flock and fcntl locks do not conflict (§10), and Linux is a v1 non-goal.

**Q2. End to end.**
- A GC loop ran every 10 ms during 12 edit-and-build cycles. On each pass it:
  1. takes the newest finalized session by name timestamp, never by mtime;
  2. for every other session, opens its lock read-only (never creating it) and tries flock EX NB;
  3. on success, removes the dir and then the lock file, and only then closes.
- Result: 901 passes. **0 build failures and 0 warnings.** 26 superseded finalized sessions were deleted. 739 of 739 `-working` sightings were skipped because they were held, so 0 were deleted.
- Natural steady state before GC: 2 sessions per crate dir, the same shape as klynt's 246 of 294. After GC: 1 per crate dir.
- **S-2:** `cargo build` with no edit after GC compiled nothing (`Finished … in 0.00s`).
- **Incremental still reused:** an edit-and-build after GC took 0.91 / 0.92 / 0.92 s. With all sessions removed first it took 1.50 / 1.51 / 1.51 s.

**Q3. Temp entries and `.cargo-build-lock`.**
- 15 builds, sampled every 5 ms: 2,215 samples.
- The lock was free in 78 samples. All 78 fall in the first ~30 ms of a cargo process, before it takes the lock.
- `rmeta*` or `rustc*` entries were present in 205 samples, and **never while the lock was free (0 of 2,215)**. None were left after a clean finish.
- If hive takes the lock in that first window, cargo waits on it ("Blocking waiting for file lock") until hive releases. It does not race.

**Answers.**
- On macOS, hive can use **one primitive, flock `LOCK_EX|LOCK_NB` on a read-only fd, never creating the file**, for both rustc session locks and Cargo's build lock. The lock is held until the deletion finishes.
- Mirroring rustc's own rule (keep the newest finalized session by name timestamp; never delete a held session) caused no failures and no rebuilds, and incremental reuse continued.
- Doctor check (d) must measure what production relies on: a child holding **fcntl F_RDLCK** and **F_WRLCK** must block the parent's flock EX NB. If it does not, Deletion is disabled.

**Not measured.**
- The newer build-dir layout.
- A target shared by several worktrees.
- rust-analyzer and `cargo watch` running live against the target.
- Any filesystem other than local APFS.
- A rustc started outside Cargo in the same `deps/`.

