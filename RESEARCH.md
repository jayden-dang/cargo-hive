# Rust build time & artifact disk usage: root causes, state of the art, and where a new tool fits

*2026-10-25 · measured on klynt (`backend/` workspace) · Apple M5 Pro, 18 cores, 48 GB, macOS 27.0.1 · rustc 1.97.1 (your pinned toolchain) + nightly 2026-08-17*

Reproduction scripts, logs, and `cargo --timings` reports are in [`bench/`](bench/). All builds ran on a copy of the source in `/tmp/klynt-bench/src` with separate target dirs, so your checkout and its `target/` were never touched. Each incremental number comes from a single run, so read them as roughly ±10%.

---

## 1. TL;DR

1. **It is not "just the compiler."** Four layers stack up. On klynt, layer 3 (Cargo's rebuild and caching model) and layer 4 (your crate structure) dominate the edit-rebuild loop. Layer 2 (rustc's single-threaded frontend) dominates clean builds. LLVM is only **~17%** of the time spent on your biggest crate.
2. **Your clean build is serialized on two crates.** `persistence` (177k LOC) and then `gateways` (141k LOC) take **53 s of an 84 s** clean `server` build. The build used 377 CPU-seconds, an average of only 4.5 of your 18 cores.
3. **Most of your incremental time is wasted.** Adding a private function to `domain` costs **8.8 s**. Rebuilding `domain` itself takes ~1 s; the rest is Cargo re-running every downstream crate. Yet the `.rmeta` those crates depend on changed in only **83 of 12.6 M bytes**. This is the "relink, don't rebuild" problem. It is the #1 complaint in the 2025 compiler performance survey, and upstream has stalled on it.
4. **About half of your disk is incremental-compilation state.** One clean build + check + test pass produces 21–24 GB, and **48%** of it is incremental `dep-graph.bin`/`query-cache.bin`. Every combination of crate × profile × features × target kind gets its own copy. Nothing garbage-collects any of it. You currently have **~185 GB** of Rust build output across checkouts.
5. **Experimental compiler options help clean builds, not edits.** The nightly parallel frontend (`-Zthreads=8`) cut the clean build **86 s → 55 s (−36%)**. Adding Cranelift on top brought it to **48 s (−45%)**. Neither changed incremental rebuild time at all.
6. **Transparent filesystem compression is a free ~2.2–2.45×.** Applesauce (APFS LZFSE) shrank a 7.45 GB target dir to **3.04 GB** in 9 s, with byte-for-byte verification and no measurable steady-state slowdown. No existing Rust tool does this. *(Corrected 2026-10-25: an earlier "23 → 8.6 GB, 2.7×" figure came from afsctool, which turned out to zero every hardlinked file. See SPIKES.md §5.)*
7. **Where a new tool fits:** below rustc, in the layers Cargo leaves unmanaged.
   - **(A)** A target-dir storage manager: reachability GC, cold-artifact compression, CoW dedup, and a "why do I have 14 copies" explainer. Low risk, ships now, and the numbers are measured.
   - **(B)** A "relink-don't-rebuild" layer. High risk and high reward: an estimated ~4× on your common edit loop. It is blocked on rustc's crate hash (SVH), which is why it probably has to be a rustc contribution rather than a wrapper.

---

## 2. What I measured on klynt

### 2.1 Shape of the workspace

| | |
|---|---|
| Workspace members | 42 crates, **620k LOC** of Rust |
| Resolved packages | 597 (23 crates with >1 version: hashbrown ×4, getrandom ×3, syn ×2, …) |
| Proc-macro crates | 46 (serde_derive, sqlx-macros, utoipa-gen, async-trait, tracing-attributes, …) |
| Build scripts | 76 (aws-lc-sys alone: 19 s of C compilation) |
| Targets | 39 libs, 15 bins, **76 test targets**, 2 benches |
| `persistence` | 177k LOC, **1,136 `sqlx::query*!` macros**, 1,571 `async fn`, 110 `#[async_trait]` |
| `gateways` | 141k LOC, 269 `#[utoipa::path]`, 327 `ToSchema` derives, 255 `.route(` |

Plain `cargo build --workspace` **fails**: `continuity_service` uses `persistence::testkit`, which is only switched on through dev-dependencies. That is a symptom of the feature-divergence problem in §2.4.

### 2.2 Build times (stable 1.97.1 unless noted)

| Scenario | Wall | CPU | Notes |
|---|---|---|---|
| Clean `cargo build --bin server` | **84 s** | 377 s | `persistence` starts at 32 s and runs 32 s; `gateways` runs 21 s |
| Clean `cargo check --workspace --all-targets --all-features` (after the above) | 65 s | 327 s | Shares nothing with `build` |
| Clean `cargo test --no-run --workspace --all-features` (after the above) | 114 s | 770 s | 144 test/bin executables |

Incremental edit → `cargo build --bin server`. The edit appends a private `fn` to the named crate.

| Edited crate | Stable | Nightly | `-Zthreads=8` | Cranelift | Cranelift + threads | Ideal (estimated) |
|---|---|---|---|---|---|---|
| `server` (leaf bin) | 1.9 s | 1.4 s | 1.3 s | 1.4 s | 1.3 s | ~1.3 s |
| `gateways` | 4.9 s | 3.7 s | 3.5 s | 3.6 s | 3.7 s | ~3.5 s |
| `persistence` | 7.1 s | 6.2 s | 6.0 s | 6.1 s | 6.1 s | ~3.8 s |
| `domain` (near the root) | 9.4 s | 8.8 s | 8.4 s | 8.7 s | 9.6 s | **~2.2 s** |

The "ideal" column is the crate's own incremental rustc time plus relinking `server`. It is what you would get if Cargo skipped dependents whose inputs had not meaningfully changed.

| Clean build, nightly | Time | Target size |
|---|---|---|
| LLVM baseline | 86.2 s | 4.7 GB |
| `-Zthreads=8` (parallel frontend) | **54.9 s** | 4.7 GB |
| `-Zcodegen-backend=cranelift` | 71.6 s | 4.2 GB |
| Cranelift + `-Zthreads=8` | **47.8 s** | 4.2 GB |
| `debug = 0` (stable; vs your `line-tables-only`) | 78.4 s | 3.8 GB (vs 4.8 GB) |

### 2.3 Where the time goes inside `persistence`

From `-Ztime-passes`; full listings are in `bench/timepasses.txt` and `bench/tp-incr.txt`.

**Full (non-incremental) compile: 24.5 s total**

| Pass | Time |
|---|---|
| `generate_crate_metadata` (writing the 40 MB `.rmeta`) | 7.5 s |
| `monomorphization_collector` | 7.0 s |
| `MIR_borrow_checking` | 6.8 s |
| `type_check_crate` (includes `coherence_checking` 4.7 s ← unusually high, worth a self-profile) | 6.3 s |
| `codegen_to_LLVM_IR` | 2.0 s |
| `LLVM_passes` | 2.2 s |
| `macro_expand` | 0.8 s |

LLVM + codegen account for about **4.2 s of 24.5 s (~17%)**.

**Incremental compile after a 1-line private edit: 2.7 s total**

| Pass | Time |
|---|---|
| `macro_expand` | 0.77 s (re-runs all 1,136 sqlx macros: proc-macro output is never cached) |
| `generate_crate_metadata` | 0.70 s (re-encodes the whole rmeta) |
| Loading/saving the 257 MB dep-graph and the query cache | ~1 s |

**Implication:** a faster backend (Cranelift, TPDE) cannot fix this crate. What helps is less frontend work per crate (smaller crates), more frontend parallelism, and not re-running dependents.

### 2.4 Disk

Current footprint:

| Location | Size | What it is |
|---|---|---|
| `klynt/backend/target` | **54 GB** | 29 GB `incremental/`, 24 GB `deps/` (140k files) |
| `.worktrees/feat-avmed/backend/target` | **91 GB** | One worktree |
| `klynt/target` (repo root) | 26 GB | Last written Oct 21. The workspace lives in `backend/`, so this looks like a stray build from the repo root |
| `~/.cache/cargo-target/klynt` | 14 GB | Orphaned by your Oct 8 config change (the comment in `.cargo/config.toml` describes it) |
| `~/.cargo` + `~/.rustup` | 11 GB | Registry, git checkouts, 5 toolchains |

**Why `backend/target` holds so many copies.** `.fingerprint/` has 2,669 units with **217 distinct profile hashes** but only one rustc version. `persistence` alone exists in ~20 variants: dev, test, check, and clippy × `testkit` on/off × lib/test/bench targets. Things that diverge in your setup:

- rust-analyzer runs `clippy` **without** `--all-features`, while moon runs `clippy --all-targets --all-features`. Different feature sets mean different units.
- `testkit` features are enabled only through dev-dependencies, so `build` and `test` resolve different feature sets for `base` and `persistence`.
- `nextest --all-features`, `test -p gateways --doc`, and `llvm-cov` (instrumented, a full extra copy) each add more variants.
- Nothing ever deletes the old ones. Cargo has no target-dir GC (cargo#13136, not started).

**One fresh pass** (build server + check + test --no-run) totals 24.3 GB of unique blocks:

| Size | Share | What |
|---|---|---|
| 11.65 GB | **48%** | incremental `dep-graph.bin` + `query-cache.bin` (the `persistence` dep-graph alone is 257 MB per variant) |
| 3.88 GB | 16% | 144 test/bin executables (~200–290 MB each: each statically links the whole graph) |
| 3.08 GB | 13% | rlibs |
| 1.88 GB | 8% | incremental object files (on macOS, `split-debuginfo=unpacked` keeps them and hardlinks them into `deps/`) |
| 1.05 GB | 4% | rmeta |

**Compressibility (zstd -1 / -19):**

| Artifact | zstd -1 | zstd -19 |
|---|---|---|
| `.o` | 7.5× | 11.2× |
| rlib | 5.8× | 8.6× |
| test executable | 4.8× | 7.1× |
| `query-cache.bin` | 4.3× | 6.6× |
| rmeta | 2.6× | 3.6× |
| `dep-graph.bin` | 1.9× | 2.4× |

**APFS transparent compression — CORRECTED.** The original measurement here used `afsctool` and reported "23 GB → 8.6 GB with no slowdown". That number is invalid: afsctool 1.7.3 on macOS 27 **truncates every hardlinked file to 0 bytes** while still returning success, and Cargo target dirs are >75% hardlinks. The re-measurement with Applesauce (SPIKES.md §5) gives **7.45 → 3.04 GB (2.45×)**, verified byte for byte, with no steady-state build slowdown. rustc rewrites rebuilt files uncompressed (3.04 → 5.22 GB after 6 rebuilds), so cold files need periodic re-compression. Note that `ditto --hfsCompression` silently does nothing on macOS 27.

**Cross-worktree sharing.** I compared main vs `feat-avmed`. They have 6.5 GB of identically named dependency artifacts, but only **1.2 GB is byte-identical**. The rest diverge because one upstream fingerprint differs: I traced one case to `sqlx_core`'s dependency fingerprint and the difference propagating to every dependent. Naive dedup therefore gets little. Deduplication only pays off once configurations converge.

### 2.5 The "relink, don't rebuild" evidence

I changed a private, non-generic function body in `domain` (`library_match.rs::normalize`) and diffed the resulting `.rmeta`:

- Same size, 12,598,848 bytes.
- **83 bytes differ**, in 25 small regions: the crate hash (SVH) in the header, derived hashes, and the span of the edited fn.
- 99.9% of content-defined chunks are shared.
- Rebuilding the original source reproduces the original rmeta byte for byte.

Even so, Cargo re-ran rustc for `base`, `observability`, `persistence`, `gateways`, every service crate, and `server`. That is the bulk of the 8.8 s.

### 2.6 A bug I hit

`debug = 0` makes `sqlx_macros`' proc-macro dylib fail to load: `dlopen … mis-aligned LINKEDIT string pool`. Cargo auto-applies `strip = "debuginfo"` when `debug = 0`, and setting `CARGO_PROFILE_DEV_STRIP=none` fixes it. My guess is the strip step interacting with Apple's ld on macOS 27, but I haven't confirmed the root cause. If you ever turn debuginfo off, set `strip = "none"` explicitly.

---

## 3. Why Rust builds are slow: the four layers

### Layer 1: language design (sets the floor)

- **Monomorphization.** Every generic instantiation (`serde`, `tokio`, `axum` extractors, sqlx row decoders) is type-checked once, but code is generated *per using crate*. Two sibling crates using `Vec<MyRow>` both compile it. `-Zshare-generics` (on by default in debug builds) mitigates this but has open bugs. Polymorphization was removed around 1.84; a new dictionary-passing design is a 2026 goal, unfunded.
- **The crate is the compilation unit.** There is no header/implementation split, so a downstream crate depends on its upstream's *entire* metadata, including private items' hashes.
- **Trait coherence and solving** are global, expensive analyses (your 4.7 s `coherence_checking`).
- **Proc macros are arbitrary programs.** Their output can't be assumed deterministic, so rustc never caches it, and sccache can't cache proc-macro crates. Your 1,136 sqlx macros re-run on every edit to `persistence`.
- **Async** generates large state machines plus lots of MIR. `async_trait` boxes futures.

### Layer 2: compiler implementation (being worked on)

- **The frontend is single-threaded on stable.** Parallel frontend: compiler-team MCP #1005 was accepted 2026-07-08. The plan is a stable `-j` flag, a `--deterministic` mode, nightly-default capped at 2 threads for 3–6 months, then stabilization. My inference is that stable is unlikely before mid-2027. On this nightly, the flag is now `--jobs-frontend`.
- **LLVM at `-O0` is slow** compared with baseline backends. Cranelift (nightly component; 2026 goal, $65k funded) measures ~5–15% end-to-end, and I measured −17% clean. TPDE (CGO 2026; 8–24× faster than LLVM -O0 on SPEC) has an accepted 2026 goal but no funded contributor and no working rustc integration.
- **The next-generation trait solver** has been default on nightly since 2026-08-21. Most crates are neutral; a few are much faster (datafusion >8×).
- **Incremental compilation** starts at HIR, so name resolution and macro expansion aren't incremental. `check`, `build`, and `test` share nothing; "Incremental Systems Rethought" (RFC 3881) is a 2026 goal, partially funded. The dep-graph scales with crate size (257 MB for `persistence`).
- Nethercote's 2026 goal notes rustc got ~3× faster over 2016–2023 and has hit diminishing returns since.

### Layer 3: Cargo's build model (largely unaddressed)

- **No early cutoff.** If a dependency is rebuilt, every dependent is rebuilt, even when its interface is byte-for-byte equivalent (§2.5). "Relink, don't rebuild":
  - MCP compiler-team#790 was accepted in 2025.
  - The 2025H2 goal was reportedly marked "will not complete" (I saw that only in a search summary, not the page itself).
  - There's no 2026 goal page; it appears only in the 2026 roadmap. Its open problems are stale diagnostics and correctness.
- **Fingerprints are keyed by configuration** (features, profile, RUSTFLAGS, target kind). Every combination is a full copy (§2.4). `resolver.feature-unification = "workspace"` (RFC 3692) is still unstable.
- **No target-dir GC.** `cargo clean gc` covers only `~/.cargo` (stable automatic GC since 1.88). Target GC (#13136) hasn't started.
- **No cross-project cache.**
  - #5931 has been open since 2018. A 2026 goal is funded at $30k by AWS; the nightly MVP covers registry crates only, with no build scripts or proc macros.
  - A prototype reported 12m59s → 6m46s across 20 projects and 5.8 → 3.5 GB on worktrees.
  - The Cargo team does not endorse sharing one build-dir across workspaces.
- **Path dependence.** Moving a target dir forced 29 units to rebuild in my test. Absolute paths get into fingerprints and `.d` files, which hurts sccache, Docker layers, and worktrees.
- The new `build-dir` layout (`-Zbuild-dir-new-layout`, per-unit directories) is in call-for-testing (March 2026). `build.build-dir` itself has been stable since 1.91.

### Layer 4: project architecture (in your control today)

- **God crates.** `persistence` and `gateways` serialize the build and make every incremental step expensive. matklad's rule: a linear chain of fat crates is the worst case.
- **Wide fan-out.** Almost everything depends on `persistence`, so any edit there re-runs `gateways` and `server`.
- **Feature divergence** between tools: rust-analyzer vs clippy vs nextest vs `cargo build`.

**Verdict:** layers 1–2 set a floor, and upstream is working on them, but slowly. Partial funding and repeated slips are the norm; the parallel frontend has been "next year" since 2023. **For a 620k-LOC app workspace like yours, layers 3–4 cost more than the compiler does.** They are also the layers outsiders can change without forking rustc.

---

## 4. What has already been tried, and where it stands

| Approach | Status (Oct 2026) | Real gain | Limitation |
|---|---|---|---|
| lld default (Linux x86_64) | **Stable 1.90** | Linking ~7× faster on incremental; ripgrep end-to-end −40% | Linux only. On macOS, Apple's ld-prime is already fast (your `server` relink is ~1 s) |
| mold / Wild | mold stable; Wild Linux-only, incremental linking "planned" | mold up to 5× vs lld | No Mach-O; Wild's Mach-O port only started |
| Parallel frontend | Nightly; stabilization MCP accepted | **−36% clean** (measured) | Determinism and diagnostics-order bugs; nothing on incremental |
| Cranelift | Nightly component | −17% clean (measured), 0% incremental | Debuginfo and SIMD gaps; small win because the frontend dominates |
| TPDE backend | Accepted goal, unfunded | 8–24× vs LLVM -O0 (paper) | Not integrated |
| Polymorphization | Old version removed; new design unfunded | Projected 30%–2× post-mono | Years out |
| `-Zhint-mostly-unused` | Nightly + Cargo unstable | Big for huge, mostly-unused deps (e.g. `windows`, AWS SDKs) | Narrow use case |
| RDR / public-API hash | Stalled | Projected 5–10× rebuilds | SVH, diagnostics replay, correctness |
| Shared build cache (Cargo) | Design + nightly MVP planned 2026 | ~2× clean across projects (prototype) | Registry crates only |
| sccache | Mature | Good for CI dependencies | No incremental; no bin/proc-macro/dylib; path-sensitive |
| kache, mbx (CAS wrappers) | New, immature | No benchmarks published | Disable incremental |
| cargo-sweep, kondo, cargogc | Exist | Delete by age, project, or build record | No compression, dedup, or reachability |
| Dioxus subsecond hot-patch | Shipped in 0.7 (experimental) | Sub-second for leaf-crate edits | Tip crate only; needs code integration |
| cg_clif JIT, MIR-only rlibs, watt/precompiled proc macros | Experimental / dormant / contested (serde 2023 backlash) | — | Ecosystem and trust issues |
| Zig-style resident compiler + binary patching | Not attempted for Rust | Zig: 170–370 ms rebuilds (~300×) | Zig needed 5–6 years and its own backend |

---

## 5. What to do for klynt now, ranked by impact

### Disk: reclaim ~40 GB immediately, then stop the regrowth

1. **Delete the two orphans**, after a quick look: `~/.cache/cargo-target/klynt` (14 GB, orphaned since Oct 8) and `klynt/target` at the repo root (26 GB, last write Oct 21). I didn't delete either.
2. **Clean a worktree's `backend/target` when you remove the worktree**, or run `cargo sweep --time 7` in each checkout weekly. You already have cargo-sweep installed.
3. **Make the tools agree on features.** Add `"rust-analyzer.cargo.features": "all"` so editor clippy shares units with `moon run api:lint` and nextest. This removes a whole variant set of every workspace crate.
4. **Keep `llvm-cov` in CI only**, as you mostly do now. Locally it is a full instrumented duplicate.

### Speed: the structural lever is crate shape

5. **Split `persistence`** into a small `persistence_core` (pool, error, uow, ports, testkit), plus repository crates per bounded context (identity, course, assistant, quota, media, …). Have services depend only on the repository crates they use. This is expected to:
   - turn the 32 s serial block into parallel work across ~6–10 crates;
   - shrink each incremental step: a smaller dep-graph to load, a smaller rmeta to re-encode, and a fraction of the 1,136 sqlx macros to re-expand;
   - stop an edit in one repository from re-running `gateways` for unrelated services.
6. **Split `gateways`** the same way: per-feature router crates plus a thin assembly crate that merges routers and the `utoipa` `OpenApi` document. 269 `#[utoipa::path]` and 327 `ToSchema` derives spread across crates parallelize well.
7. **Profile two anomalies in `persistence`:**
   - `coherence_checking` at 4.7 s;
   - `generate_crate_metadata` at 7.5 s (likely a large exported MIR surface from generic or `#[inline]` code).

   Use `cargo +nightly rustc -p persistence -- -Zself-profile` plus `summarize`/`crox`. A handful of blanket impls or generic helpers is a common culprit; making the public API non-generic (generic outer function → non-generic `inner`) often helps.

### Speed: cheap toggles

8. **Nightly parallel frontend for clean builds**, e.g. a `just build-fast` recipe with `RUSTFLAGS="-Zthreads=8"` on a pinned nightly: −36% clean. It uses a separate target dir and is not for CI or release (determinism caveats).
9. **Leave the rest alone:**
   - Cranelift: marginal for you.
   - Debuginfo: `line-tables-only` is already near optimal; `debug = 0` only bought −7% time.
   - Linker: on macOS it isn't your bottleneck.
10. **CI:** on Cargo ≥ 1.99, incremental is disabled automatically when `CI` is set (cargo#17220). Use sccache or rust-cache for third-party dependencies only, and cargo-chef in Docker.

---

## 6. Is there room for a genuinely better, open-source tool?

### The landscape

- **Codegen speed** (Cranelift, TPDE, parallel frontend) is funded and staffed upstream. Competing there is a poor use of effort.
- **Distributed compilation:** the sccache author reports "practically zero" real-world use of sccache-dist.
- **Global prebuilt binaries** are blocked by the lack of a stable ABI and by trust; see the serde 2023 episode.
- **The open space is layer 3:** what Cargo does with artifacts, and when it decides to re-run rustc.

### Opportunity A: `cargo-zip`, a target-dir storage manager

Recommended first: low risk, and it ships now.

A daemon or CLI that owns the lifecycle of `target/` and `build-dir` contents across all your checkouts and worktrees.

| Feature | Measured or estimated value | Existing tools |
|---|---|---|
| **Reachability GC.** Keep only units reachable from the last N invocations per (command, profile, feature set), using Cargo's `--message-format=json`/`--unit-graph`. Prune superseded incremental sessions and orphaned build-dirs. | Your 54 GB vs ~21–24 GB for one fresh pass suggests over half is unreachable (estimate) | cargo-sweep (age/stamp), cargogc (build record). No reachability, no incremental-session awareness |
| **Cold-artifact transparent compression:** APFS decmpfs on macOS, btrfs/ZFS on Linux, NTFS on Windows. Compress files that haven't changed in N hours, in the background. | **2.45×** measured with Applesauce (7.45 → 3.04 GB), no steady-state slowdown | None |
| **CoW dedup across worktrees** (clonefile/reflink, never hardlinks: in-place rewrites would corrupt) | 1.2 GB identical today; much more once configurations converge | kache (hardlinks, disables incremental) |
| **"Why N copies?" explainer:** diff fingerprints and print "persistence ×20: testkit on/off × dev/test/check/clippy × lib/test/bench". Detect editor/CLI feature divergence. | Surfaced your 217 profile hashes and the RA-vs-clippy split | None |
| **CI cache packer:** zstd unit-level archives, with dep-only vs workspace split | 4–8× on rlibs, `.o`, and executables | Swatinem/rust-cache (tar, no unit awareness) |

**Design notes and risks:**

- Build on the new `build-dir` layout v2 (per-unit dirs) when it stabilizes; it makes unit-level GC much cleaner.
- Never touch files while a Cargo lock is held.
- Compression costs CPU: Applesauce LZFSE took ~9–15 s wall (~60–70 CPU-s) for a 6.7 GB target. Compress only cold files, at idle.
- **Do not use afsctool:** it zeroes hardlinked files (SPIKES.md §5). Applesauce preserves hardlink groups and mtimes, and is a Rust library that cargo-hive can embed.
- Coordinate with Cargo's #13136 (target GC) so this becomes a reference implementation rather than a competitor. The Cargo team has said default GC policy is hard *because* nobody has data. This tool would produce that data.

### Opportunity B: "relink, don't rebuild" outside rustc

Research-grade, high payoff.

**The evidence (§2.5):** for private, non-inlinable edits, the downstream-visible metadata is unchanged apart from hashes and spans. On your `domain` edit, skipping dependents would cut **8.8 s → ~2.2 s (~4×)**. The 2025 survey ranks "changing one crate rebuilds its dependents" as the top complaint.

**Prototype shape:**

1. A `RUSTC_WRAPPER` (or a `rustc_driver`, as clippy uses) computes an *interface hash* per crate. This covers exported types, signatures, trait impls, and the MIR of every cross-crate-inlinable or generic item, while ignoring private non-inlinable bodies.
2. When Cargo asks it to recompile a dependent whose own sources are unchanged and whose dependencies' interface hashes are unchanged, the wrapper reuses the previous outputs and replays the stored diagnostics.
3. The final binary is still relinked against the new object code.

**Why it is hard (and why upstream stalled):**

- **The SVH (crate hash) is embedded in every dependent.** Swap in a new upstream crate with a new SVH under an old dependent and you get "found possibly newer version of crate" (E0460). A wrapper would have to keep the SVH stable, i.e. patch artifacts. That is fragile and tied to rustc's version-specific format. The clean fix is for rustc to split the SVH into an interface hash and an implementation hash, which is what `-Z public-api-hash` (rust-lang/rust PR #155871, not in your nightly) heads toward.
- **Cross-crate inlining.** Small functions are auto-inlined across crates, and generics are instantiated downstream. The interface hash must cover all MIR exported into the rmeta. Conveniently, that MIR lives in the rmeta, which is why only 83 bytes moved in my test.
- **Diagnostics replay and `cargo check` interplay** are among the open problems upstream lists.

**Recommendation:** treat B as a contribution, or a measured proof of concept, aimed at upstream RDR. Don't plan it as a standalone product. Concretely: write a tool that *measures* how often real edits leave the interface unchanged across your git history. That data is missing upstream, and it is cheap to produce with the rmeta-diff technique I used here.

### Opportunities I'd skip

- **A Zig-style resident compiler.** It is the biggest theoretical win (~300× rebuilds in Zig), but it needs deep rustc cooperation. Subsecond already covers leaf-crate hot-patching.
- **A proc-macro output cache** that verifies idempotence (e.g. by sandboxing). It is attractive (0.8 s per `persistence` edit for you), but it needs rustc hooks too. Watch the wasm-proc-macro pre-RFC.

---

## 7. How far can it be pushed? (klynt, rough targets)

| Loop | Today | Today's tools + crate splits | + upstream RDR / interface hashing |
|---|---|---|---|
| Clean `server` | 84 s | ~35–45 s (split + parallel frontend) | same |
| Edit near the root (`domain`) | 8.8 s | ~5–6 s (smaller dependents) | **~2 s** |
| Edit in a repository | 6.2 s | ~2–3 s (only affected crates) | ~1.5–2 s |
| Disk per active checkout | 54–91 GB | ~10–15 GB (GC + feature alignment + compression) | — |

These targets are estimates extrapolated from the measurements above, not measured results, except where §2 shows the actual number.

---

## Sources

From the three research passes. Each claim was checked against the primary page where reachable; Zulip was not reachable.

- Rust Compiler Performance Survey 2025 results: https://blog.rust-lang.org/2025/09/10/rust-compiler-performance-survey-2025-results
- 2026 "Fast builds" roadmap: https://goals.rust-lang.org/2026/roadmap-fast-builds.html
- Parallel frontend stabilization MCP: https://github.com/rust-lang/compiler-team/issues/1005 · goal: https://goals.rust-lang.org/2026/parallel-front-end.html
- Cranelift goal: https://goals.rust-lang.org/2026/improve-cg_clif-performance.html · TPDE: https://goals.rust-lang.org/2026/tpde.html · https://arxiv.org/abs/2505.22610
- Polymorphization (new): https://goals.rust-lang.org/2026/polymorphization.html
- Incremental Systems Rethought: https://goals.rust-lang.org/2026/incremental-system-rethought.html
- Next-solver on nightly: https://blog.rust-lang.org/2026/08/21/enabling-next-solver-on-nightly
- RDR: https://goals.rust-lang.org/2025h2/relink-dont-rebuild.html · https://github.com/rust-lang/rust/pull/143249
- Cross-workspace cache: https://goals.rust-lang.org/2026/cargo-cross-workspace-cache.html · https://github.com/rust-lang/cargo/issues/5931
- build-dir layout v2 call for testing: https://blog.rust-lang.org/2026/03/13/call-for-testing-build-dir-layout-v2
- Fine-grained locking: https://github.com/rust-lang/cargo/issues/4282 · target GC: https://github.com/rust-lang/cargo/issues/13136 · cache GC: https://github.com/rust-lang/cargo/issues/12633
- Feature unification: https://github.com/rust-lang/cargo/issues/14774 · RFC 3692
- `-Zhint-mostly-unused`: https://github.com/rust-lang/rust/blob/main/src/doc/unstable-book/src/compiler-flags/hint-mostly-unused.md
- lld default: https://blog.rust-lang.org/2025/09/01/rust-lld-on-1.90.0-stable · Wild: https://github.com/davidlattimore/wild
- sccache Rust caveats: https://github.com/mozilla/sccache/blob/main/docs/Rust.md · distributed usage: https://brokenco.de/2025/01/05/sccache-distributed-compilation.html
- kache: https://github.com/AlJohri/kache · cargogc: https://github.com/LosEcher/cargogc
- Kobzol, "What part of Rust compilation is the bottleneck?": https://kobzol.github.io/rust/rustc/2024/03/15/rustc-what-takes-so-long.html
- matklad, "Fast Rust Builds": https://matklad.github.io/2021/09/04/fast-rust-builds.html
- TiKV compile-time series: https://www.pingcap.com/blog/reasons-rust-compiles-slowly
- Zig incremental: https://ziggit.dev/t/bun-s-zig-fork-got-4x-faster-compilation-times/15183/19
- Dioxus subsecond: https://dioxuslabs.com/blog/release-070
