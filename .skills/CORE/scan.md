# Scan digest: CORE (touched surface = external formats and APIs)

Machine: Darwin 27.0.0 arm64, APFS, git 2.54.0 (Apple Git-157), cargo/rustc 1.97.1 (default), toolchains on disk 1.96.1 / 1.97.1 / 1.98.1 / nightly (1.100.0-nightly 2026-08-17). Extends `req-review.md` (its verified premises are not repeated). All experiments read-only on real repos, or in `/private/tmp/scan-*` (removed afterwards). Cargo/rustc source cited from GitHub at the tag matching the installed toolchain (cargo `c980f4866` = 1.97.1, rust tag `1.97.1`) or master where noted.

## 1. Cargo fingerprint unit dir and `.rustc_info.json`

Corpus: 5 real target dirs (klynt, raki/bot, work/zroute, work/CommandOss/mailgate, experiments/porch debug+release), 10,336 unit dirs total, plus throwaway crates built with 1.96.1, 1.97.1, 1.98.1 and nightly.

### 1.1 Fingerprint JSON key set (identical for every unit kind and every toolchain that has a `.fingerprint/` dir)

Exactly 11 keys. Every corpus JSON (lib, bin, test, build-script, run-build-script, example) and every throwaway-crate JSON (1.96.1, 1.97.1, 1.98.1, nightly) had this set, with 1 distinct signature per kind:

| key | JSON type | notes |
|---|---|---|
| `rustc` | integer (u64) | `hash_u64(rustc -vV stdout)`; see 1.3 |
| `features` | string | e.g. `"[\"default\", \"std\"]"` (a JSON array serialized into a string); `""` for run-build-script |
| `declared_features` | string | same style; `""` for run-build-script |
| `target` | integer (u64) | `0` for run-build-script |
| `profile` | integer (u64) | `0` for run-build-script |
| `path` | integer (u64) | `0` for run-build-script |
| `deps` | array of `[u64, string, bool, u64]` | 4-tuples in 100% of 107,000+ edges: (pkg-id hash, extern crate name, public flag, dep fingerprint u64) |
| `local` | array of single-key objects | variants seen: `CheckDepInfo{dep_info,checksum}`, `RerunIfChanged{output,paths}`, `RerunIfEnvChanged{var,hash}`, `Precalculated(...)` |
| `rustflags` | array | |
| `config` | integer (u64) | `0` exactly for run-build-script units (82/82, 46/46, 48/48, 166/166, 25/25, 22/22); nonzero for all others |
| `compile_kind` | integer | `0` everywhere seen (host builds) |

Field order in the file is not alphabetical: `rustc, features, declared_features, target, profile, path, deps, local, rustflags, config, compile_kind`. There is no version field.

Evidence (klynt `debug`, 2812 units): `lib 2086, test 478, bin 90, build-script 76, run-build-script 82`, one signature each. Sample run-build-script JSON: `{"rustc":11722972603611519644,"features":"","declared_features":"","target":0,"profile":0,"path":0,"deps":[[966925859616469517,"build_script_build",false,10849764486113511199]],"local":[{"RerunIfChanged":{"output":"debug/build/ahash-aa3e29b18f6ac969/output","paths":["build.rs"]}}],"rustflags":[],"config":0,"compile_kind":0}`.

Values exceed `i64::MAX` (e.g. 17112352188342185299): parse as u64, never i64 or f64.

### 1.2 Unit dir contents and kinds

- Unit dir = `.fingerprint/<pkg>-<16hex metadata>/`. Hash file name = `<kind>-<target>` and the JSON is `<hash file name>.json`. Hash file content: exactly 16 lowercase hex chars, no newline (2812/2812 in klynt match `[0-9a-f]{16}`).
- Kind prefixes seen: `lib-`, `bin-`, `test-` (also `test-lib-x`, `test-bin-x`, `test-integration-test-x`, `test-example-x`), `build-script-`, `run-build-script-`, and **`example-<name>`** (mailgate: 18 dirs, e.g. `example-preflight`, `example-preflight.json`, with `dep-example-preflight`, `invoked.timestamp`). Any hash-file indexer must accept `example-` (and by extension `bench-`, not observed); req-review's list omitted it.
- `invoked.timestamp` presence per kind: present for lib, bin, test, build-script, example; **absent for run-build-script** (82/82 klynt; same in all other corpora). Also present in throwaway builds from all four toolchains. It is a 48-byte text file whose content is `This file has an mtime of when this was started.`: the mtime is the signal. Also exist per unit: `dep-<kind>-<target>` (dep-info binary), optional `output-<kind>-<target>` (cached diagnostics). None of these three are hash files.
- Empty unit dirs exist: raki/bot has 6 dirs under `.fingerprint/` with zero files (e.g. `raki-desktop-1c5ae30d4fd76667`, `tauri-plugin-deep-link-70c4143e8ce150a7`). CORE-4.7's "holding exactly one fingerprint JSON" correctly excludes them; they must not count as unsupported-format evidence. klynt: 0 empty dirs, 0 multi-JSON dirs.
- Edge resolution (CORE-4.8): Cargo source confirms the encoding, `cargo src/util/hex.rs`: `pub fn to_hex(num: u64) -> String { hex::encode(num.to_le_bytes()) }` and `short_hash = to_hex(hash_u64(..))`. Matches req-review's 24855/24855 result.

### 1.3 `rustc` u64 and `.rustc_info.json`

`.rustc_info.json` at target root, shape: `{"rustc_fingerprint": <u64>, "outputs": {"<u64-as-string>": {"success":bool,"status":str,"code":int,"stdout":str,"stderr":str}, ...}, "successes": {}}`. Three `outputs` entries in klynt (key = hash of the rustc probe args). One entry's `stdout` is exactly the `rustc -vV` text (`rustc 1.97.1 (8bab26f4f 2026-07-14)\nbinary: rustc\ncommit-hash: 8bab26f4f68e...\ncommit-date: 2026-07-14\nhost: aarch64-apple-darwin\nrelease: 1.97.1\nLLVM version: 22.1.6\n`); the others are `--print` probes (cfg, sysroot).

Two different "rustc" numbers, unrelated:

1. Top-level `rustc_fingerprint` is NOT a toolchain identity. Cargo `src/util/rustc.rs` `rustc_fingerprint()` hashes the rustc executable path, file length, creation time and mtime, wrapper executables, and RUSTUP_TOOLCHAIN/RUSTUP_HOME plus the real rustc mtime. Evidence: same rustc 1.97.1 (commit 8bab26f4f) gives `4321599665511194399` (klynt), `7861893566433789548` (raki, porch), `12028122066424590362` (mailgate).
2. Per-unit JSON `rustc` equals `hash_u64(rustc -vV stdout)` using `rustc_stable_hash::StableSipHasher128` (cargo `src/util/hasher.rs`, `src/compiler/fingerprint/mod.rs:1714` `rustc: util::hash_u64(&...rustc().verbose_version)`). **Verified numerically**: a 6-line Rust program with `rustc-stable-hash 0.1.2` hashing the `String` of `rustc -vV` gave `11722972603611519644` for 1.97.1 and `17329007180185699724` for 1.98.1, matching the JSON `rustc` field of every unit in klynt/raki/mailgate/porch (1.97.1) and zroute (1.98.1) respectively. 1.96.1 gives `776363091828236830`; plain `std::hash::SipHasher` does not match.

So a profile dir's rustc identity = the single (or set of) `rustc` u64 values in its JSONs; it can be decoded to a version only by hashing candidate `rustc -vV` outputs (the active toolchain's, or the `-vV` entry in `.rustc_info.json`). Each real profile dir had exactly 1 distinct `rustc` value (corpus), but stale mixed values are possible after a toolchain switch (not observed). `.rustc_info.json` records the most recent rustc probed for the target root, not necessarily the one that built every unit.

### 1.4 Toolchain comparison

| source | rustc | JSON signature | layout |
|---|---|---|---|
| klynt, raki, mailgate, porch | 1.97.1 | same 11 keys | `<profile>/.fingerprint/<unit>/` |
| zroute | 1.98.1 | same 11 keys | same |
| throwaway crate 1.96.1 / 1.97.1 / 1.98.1 | | same 11 keys, same kinds, same invoked.timestamp rule | same |
| throwaway crate nightly 1.100.0-nightly (cargo 8a0d8afba, 2026-08-15) | | same 11 keys | **different layout, no flag**: `<profile>/build/<pkg>/<16hex>/{fingerprint,out}/`, no `.fingerprint/` dir. JSON `local[].CheckDepInfo.dep_info` paths read `debug/build/c/7246d1137464ada2/fingerprint/dep-test-bin-c`. Cargo master `layout.rs` gates this on `-Zbuild-dir-new-layout`, yet the installed nightly produced it with no `-Z`, no config, no env set (cargo `-Zhelp` lists `build-dir-new-layout`). Not stable yet; stable up to 1.98.1 uses `.fingerprint/`. |

Lock files in the profile dir (all versions): `.cargo-lock`, `.cargo-build-lock`, `.cargo-artifact-lock` (see section 4).

### 1.5 Proposed "supported format" signature

A profile dir is supported iff it has `.fingerprint/` and every unit dir that holds exactly one `*.json` satisfies ALL of:

1. JSON is an object whose key set equals exactly `{rustc, features, declared_features, target, profile, path, deps, local, rustflags, config, compile_kind}`.
2. Types: `rustc, target, profile, path, config` unsigned 64-bit integers; `compile_kind` integer; `features, declared_features` strings; `rustflags, local` arrays; `deps` an array whose every element is `[u64, string, bool, u64]`.
3. Hash file `<name>` beside `<name>.json` exists, is 16 bytes of `[0-9a-f]`, and `<name>` starts with one of `lib-|bin-|test-|example-|bench-|build-script-|run-build-script-`.
4. (Soft, per profile) the set of distinct `rustc` values is reported; unsupported only if none can be matched when the doctor check (f) is run with the active toolchain.

Doctor check (f) concrete test: build a throwaway crate (lib + bin + build.rs + test) in the temp dir with the active toolchain, verify the above on every unit, additionally check JSON `rustc == hash_u64(rustc -vV)` (needs `rustc-stable-hash`, tiny dep) and that the hash-file/`deps` edge resolution gives 0 unresolved among the throwaway units. If the active toolchain writes the `build/<pkg>/<hash>/fingerprint/` layout instead, check (f) reports `fail` because there is no `.fingerprint/`.

Design-relevant takeaways
- Key set and value types are stable across kinds and across 1.96.1, 1.97.1, 1.98.1, nightly: use the exact-11-key plus types signature; do not key on run-build-script vs lib differences except `config==0 && target==0`.
- Add `example-` (and `bench-`) to the hash-file prefix whitelist; never rely on prefix `lib-` only.
- Tolerate empty and JSON-less unit dirs (count only dirs with exactly one JSON, as CORE-4.7 says); raki has 6 empties.
- Parse u64 as u64 (values above 2^63 are common).
- `.rustc_info.json` `rustc_fingerprint` is useless as identity; identity is the JSON `rustc` hash, decodable only by hashing a known `rustc -vV`. Reading the `-vV` entry from `.rustc_info.json` gives the last rustc, not necessarily the builder of every unit.
- The new build-dir layout (`build/<pkg>/<hash>/fingerprint/`) is live in the installed nightly: CORE-4.1 would not recognize such a dir as a target dir at all (no `.fingerprint/`), so it appears as "not a Cargo target dir" rather than "unsupported". Decide whether CORE-4.9 should cover that case.
- `invoked.timestamp` is absent for run-build-script; any "last used" signal must fall back to the sibling compile unit or the hash file mtime.

## 2. Git: worktree list, rev-list, rev-parse

Throwaway repos in `/private/tmp/scan-git`; `GIT_CONFIG_GLOBAL/SYSTEM=/dev/null`.

### 2.1 `git worktree list --porcelain` record format

Records are blocks separated by one empty line; each line is `<key>[ <value>]`. Observed lines (`cat -e` confirmed LF only):

```
worktree <abs path>          # always first, always present
HEAD <40-hex sha>            # present for all non-bare; unborn branch prints 0000000000000000000000000000000000000000
branch refs/heads/<name>     # attached HEAD
detached                     # bare word, replaces "branch" for detached HEAD
bare                         # bare repo record: only "worktree" and "bare" (no HEAD, no branch)
locked [<reason>]            # "locked" or "locked why not"
prunable <reason>            # e.g. "prunable gitdir file points to non-existent location"
```

Order inside a record: worktree, HEAD, branch|detached|bare, locked, prunable. Verified cases: main (first record, `branch refs/heads/main`), linked branch, detached (`detached`), locked with reason, locked without, deleted dir (prunable), locked and directory moved away (listed under the OLD path, `locked`, no `prunable`; confirms req-review), bare repo plus a worktree added from it (`worktree .../bare.git` / `bare`).

Paths with unusual characters: in `--porcelain` a path containing a space prints raw (`worktree /private/tmp/scan-git/wt sp ü`), but the non-porcelain `git worktree list` quotes it as C-style (`"/private/tmp/scan-git/wt sp \303\274"`). `--porcelain -z` (NUL-terminated lines, empty line between records as `\0\0`) exists and avoids quoting questions; use it.

### 2.2 `git rev-list --max-parents=0 HEAD` on unborn branch

Empty repo (`git init`, no commits): exit 128, stdout empty, stderr `fatal: ambiguous argument 'HEAD': unknown revision or path not in the working tree.` (plus `Use '--'...` lines). `git rev-parse HEAD` prints `HEAD` on stdout and exits 128. `git rev-list --max-parents=0 --all` exits 0 with empty output. `git worktree list --porcelain` works (HEAD all zeros). Shallow clone (`--depth 1`): `rev-list --max-parents=0 HEAD` returns the shallow boundary commit (a different hash from the true root); `git rev-parse --is-shallow-repository` prints `true`. In a linked worktree it returns the same root as main (`49d75c04...` both).

### 2.3 `rev-parse` outputs

| context | `--git-dir` | `--git-common-dir` | `--show-toplevel` |
|---|---|---|---|
| main, at root | `.git` (relative) | `.git` | `/private/tmp/scan-git/main` |
| main, subdir `deep/er` | absolute `.../main/.git` | **relative to cwd**: `../../.git` | root |
| linked worktree | `.../main/.git/worktrees/wt-branch` | `.../main/.git` | the worktree |
| submodule `main/sub` | `.../main/.git/modules/sub` | **equal to git-dir** | `.../main/sub` |
| bare repo | `.` | `.` | exit 128 `this operation must be run in a work tree` |

- `--absolute-git-dir` is absolute in every case. `--path-format=absolute --git-dir --git-common-dir` (git >= 2.31) returns absolute for both in every case tested (main subdir, linked, bare). Use it instead of manual resolution.
- git prints already-realpath'd paths when entered via the `/tmp` symlink (`/tmp/scan-git/wt-branch` printed as `/private/tmp/scan-git/wt-branch`).
- A submodule's `.git` is a file (`gitdir: ../.git/modules/sub`) but git-dir equals git-common-dir, so CORE-2.2's test ("git-dir differs from common-dir") treats a submodule as a **main** checkout. Distinguish with `git rev-parse --show-superproject-working-tree` (non-empty inside a submodule; empty in main and in linked worktrees). The linked worktree's `.git` file is `gitdir: <abs>/.git/worktrees/<name>`.
- `GIT_DIR=/nonexistent git -C <repo> rev-parse --git-dir` fails (`not a git repository: '/nonexistent'`): inherited `GIT_DIR`/`GIT_*` env overrides `-C`, so hive must scrub `GIT_DIR`, `GIT_WORK_TREE`, `GIT_COMMON_DIR` etc. from child git processes (also relevant to CORE-5.7).

Design-relevant takeaways
- Parse `--porcelain -z`; treat `worktree`/`HEAD`/`branch`/`detached`/`bare`/`locked`/`prunable` as the full key set; `HEAD` absent for bare records; all-zero sha means unborn.
- Missing-dir test must be a filesystem stat (confirmed); do not trust `prunable`.
- Root commits: catch exit 128 on unborn HEAD and treat as "no root commits yet" (registration should refuse or defer, CORE-2.1 is silent); sort the output; shallow repos give wrong roots, so check `--is-shallow-repository` and refuse or flag.
- Main vs linked: use `--path-format=absolute` git-dir vs common-dir, but add the superproject check so submodules are not registered as main checkouts.
- Run git with a scrubbed environment and `-C <path>`; scope comes from arguments (CORE-5.7).

## 3. Crate APIs

### 3a. `applesauce` 0.8.10 (published 2026-09-27; core 0.4.14; CLI 0.5.30)

Source read from the downloaded `.crate` (extracted in a scratch dir). Public surface (`src/lib.rs`):

```rust
pub mod info; pub mod progress; pub use applesauce_core::compressor;
#[derive(Default)] pub struct FileCompressor { .. }
impl FileCompressor {
  pub fn new() -> Self;
  pub fn recursive_compress<'a, P>(&mut self, paths: impl IntoIterator<Item=&'a Path>, kind: compressor::Kind,
        minimum_compression_ratio: f64, level: u32, progress: &P, verify: bool) -> Stats
        where P: Progress + Send + Sync, P::Task: Send + Sync + 'static;
  pub fn recursive_decompress<'a, P>(&mut self, paths: impl IntoIterator<Item=&'a Path>, manual: bool, progress: &P, verify: bool) -> Stats;
}
#[derive(Debug, Default)] pub struct Stats { pub files, total_file_sizes, compressed_size_start, compressed_size_final,
   compressed_file_count_start, compressed_file_count_final, incompressible_file_count: AtomicU64 }  // + compression_savings(), compression_change_portion()
pub enum compressor::Kind { Zlib = 0, Lzvn, Lzfse }        // Copy; Kind::name(), supported()
pub trait progress::Progress { type Task: Task; fn error(&self,&Path,&str); fn file_skipped(&self,&Path,SkipReason){} fn file_task(&self,&Path,u64)->Self::Task; }
pub trait progress::Task { fn increment(&self,u64); fn error(&self,&str); fn not_compressible_enough(&self,&Path){} }
pub enum progress::SkipReason { NotFile, AlreadyCompressed, NotCompressed, EmptyFile, TooLarge(u64), ReadError(io::Error), ZfsFilesystem, HasRequiredXattr, FsNotSupported, HardLink }
info::get(path) -> io::Result<AfscFileInfo { is_compressed, on_disk_size, stat_size, xattr_count, total_xattr_size, resource_fork_size, decmpfs_info, .. }>   // #[non_exhaustive]
info::get_recursive(path) -> io::Result<AfscFolderInfo>
```

- One call handles a file or a directory tree (recursive; uses jwalk, rayon, background threads). `verify: true` re-reads and compares inside the writer thread (`threads/writer.rs:129`). CLI defaults: `level 5`, `minimum_compression_ratio 0.95`, default algorithm from `Compression::default()`; level ignored for non-zlib (`applesauce-cli/src/main.rs:234`). There is no per-file result return: outcomes come through the `Progress` callbacks (`file_skipped`, `error`, `Task::not_compressible_enough`) and `Stats` counters. `Stats` fields are atomics.
- Verified by compiling and running a spike (`FileCompressor::new().recursive_compress(paths, Kind::Lzfse, 0.95, 5, &P, true)`) on /private/tmp: 3,600,000-byte file became `blocks=24 flags=compressed`, content identical (`cmp`), mtime 1577909040 unchanged; `Stats`: `files=1 total=3600000 start=3600384 final=12288 compressed_final=1`. The same file with a second hardlink: `skip ... Other hard links exist to file`, `files=0` (the skipped file is not even counted in `Stats.files`).
- `compile_error!` on non-macOS/iOS (matches macOS-only scope).
- Dependency footprint (resolved with `cargo tree -e normal`): **40 transitive crates** beyond applesauce itself (41 incl. it): applesauce-core, bitflags, cfg-if, crossbeam (+channel, deque, epoch, queue, utils), dashmap, either, errno, fastrand, getrandom, hashbrown 0.14, jwalk, libc, libdeflate-sys + libdeflater (C), lock_api, lzfse-sys (C), memchr, once_cell, oneshot, parking_lot_core, pin-project-lite, proc-macro2 + quote + syn + unicode-ident (via `tracing-attributes`), rayon + rayon-core, resource-fork, rustix 1.1.5, scopeguard, smallvec, tempfile, tracing + tracing-attributes + tracing-core. Two C libraries need a C compiler at build (cc, via -sys crates). `applesauce-core` features default `zlib, lzfse, lzvn`. Resolved set for applesauce + objc2 stack + libc together: 103 distinct crates.

### 3b. `objc2-local-authentication` 0.3.2 (2025-10-04) with `objc2` 0.6.5, `block2` 0.6.2, `objc2-foundation` 0.3.2

Transitive deps of `objc2-local-authentication` with default features: 8 (bitflags, block2, libc, objc2, objc2-core-foundation, objc2-encode, objc2-foundation, objc2-security). Defaults enable `block2`, `LAContext`, `LAError`, `objc2-security`, etc.

Signatures (`src/generated/LAContext.rs`), all `unsafe`:

```rust
pub unsafe fn LAContext::new() -> Retained<LAContext>;                                   // NSObject new (line 572)
pub unsafe fn canEvaluatePolicy_error(&self, policy: LAPolicy) -> Result<(), Retained<NSError>>;
pub unsafe fn evaluatePolicy_localizedReason_reply(&self, policy: LAPolicy, localized_reason: &NSString,
        reply: &block2::DynBlock<dyn Fn(Bool, *mut NSError)>);
pub struct LAPolicy(pub NSInteger);  // DeviceOwnerAuthenticationWithBiometrics = 1, DeviceOwnerAuthentication = 2, ...WithCompanion = 3
```

Verified by compile and run: `LAContext::new()` then `canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthentication)` printed `canEvaluate=true` with no dialog, stdin `/dev/null`. The `evaluatePolicy` form compiles with `block2::RcBlock::new(|ok: Bool, err: *mut NSError| ..)` and `&reply` auto-derefs to `&DynBlock` (NOT called; it would prompt). The reply block is invoked on an arbitrary queue; keep the context alive until it fires (per the header doc) and wait on a channel; no CFRunLoop was needed to compile, but run-loop behaviour for an actual prompt is untested (belongs to ROAD-7). `NSError::code()` / `localizedDescription()` available via `objc2-foundation` for the unavailable reason.

### 3c. `libc` 0.2.191 (latest; also cached 0.2.174/186/189/190) on aarch64-apple-darwin

All present and compile (a spike referenced them and ran):

| item | status |
|---|---|
| `clonefile(src,dst,flags:u32)`, `clonefileat`, `fclonefileat` | yes (`bsd/apple/mod.rs:5192-5200`); `CLONE_NOFOLLOW`/`CLONE_NOOWNERCOPY`/`CLONE_ACL` consts **absent**: define locally (SDK `sys/clonefile.h`: 0x1, 0x2, 0x4, `CLONE_NOFOLLOW_ANY` 0x8, `CLONE_RESOLVE_BENEATH` 0x10) |
| `getattrlist(path:*const c_char, attrList:*mut c_void, attrBuf:*mut c_void, attrBufSize:size_t, options:u32)`, `fgetattrlist`, `getattrlistat`, `getattrlistbulk` | yes (options is `u32`, not `c_ulong`) |
| `struct attrlist {bitmapcount:c_ushort, reserved:u16, commonattr, volattr, dirattr, fileattr, forkattr: attrgroup_t(u32)}` | yes, `size_of == 24`; `ATTR_BIT_MAP_COUNT = 5` |
| `FSOPT_ATTR_CMN_EXTENDED = 0x20`; `ATTR_CMNEXT_PRIVATESIZE 0x8`, `_LINKID 0x10`, `_CLONEID 0x100`, `_EXT_FLAGS 0x200`, `_RELPATH 0x4`, `_RECURSIVE_GENCOUNT 0x400` | yes. Must go in `forkattr` (extended group) as in req-review §7. |
| `EF_MAY_SHARE_BLOCKS 0x1`, `EF_SHARES_ALL_BLOCKS 0x40` | **absent from libc**; define locally (SDK `sys/stat.h:368,374` confirms) |
| `flock(fd,op)`, `LOCK_SH=1, LOCK_EX=2, LOCK_NB=4, LOCK_UN=8` | yes (`unix/mod.rs:2273`, `bsd/apple/mod.rs:3287-3290`) |
| `fcntl(fd,cmd,...)`, `F_GETLK=7, F_SETLK=8, F_SETLKW=9`, `F_RDLCK=1, F_UNLCK=2, F_WRLCK=3`, `struct flock {l_start,l_len,l_pid,l_type,l_whence}` | yes |
| `mkdtemp(*mut c_char)` | yes (`unix/mod.rs:2161`); spike created `/private/tmp/scan-SIDR60` and removed it |

Design-relevant takeaways
- Applesauce is usable as a library with a 3-callback `Progress` plumbing; there is no per-file return value, so doctor (b) must read the outcome from `Stats` plus `file_skipped` calls, and re-check `info::get(path)` / `stat` flags. A hardlinked file yields `files=0` and `SkipReason::HardLink`; assert on that signal.
- Cost: +40 crates (2 C libs, proc-macro stack) for compression; decide pin policy (this scan resolved 0.8.10 / core 0.4.14).
- LAContext path is small (8 crates, synchronous `canEvaluatePolicy`); it is `unsafe` ObjC but a ~5 line wrapper. Prefer `DeviceOwnerAuthentication` (2) as CORE specifies.
- Define `EF_*`, `CLONE_*` constants locally; everything else is in libc.
- `getattrlist` result buffer layout and `FSOPT_ATTR_CMN_EXTENDED` placement from req-review §7 are consistent with libc's `attrlist` (24 bytes); ext flags/clone id go in `forkattr`.

## 4. Lock syscalls

- **Cargo** (1.97.1, `src/cargo/util/flock.rs` at c980f4866): `f.try_lock()` / `f.lock()` / `f.try_lock_shared()` on `std::fs::File`; doc comment line 153: "advisory using flock". Std 1.97.1 `library/std/src/sys/fs/unix.rs:1455-1457,1542-1543,1604-1605`: for `target_vendor = "apple"` these call `libc::flock(fd, LOCK_EX | LOCK_NB)` / `LOCK_SH | LOCK_NB`. So **Cargo = `flock(2)`** on macOS (master is the same via std; the older `fs2`/`fs4` shim is gone). ENOTSUP/EOPNOTSUPP is treated as "unsupported, assume locked OK"; NFS mounts skip locking (`is_on_nfs_mount`).
- Which files (`src/cargo/core/compiler/layout.rs` at c980f4866, lines 262-305): `<profile>/.cargo-build-lock` exclusive (`open_rw_exclusive_create`) in the build-dir profile dir (shared only with `-Zfine-grain-locking` and not exclusive-needed); `<profile>/.cargo-lock` **shared** (`open_ro_shared_create`, for compatibility with old Cargo that took it exclusive); `<profile>/.cargo-artifact-lock` exclusive only when the artifact dir is needed. Files are created at first use and never deleted.
- **Empirically verified** with a live `cargo build` (build script sleeping 8 s), on both 1.97.1 and 1.98.1, probing with a read-only fd and `flock(..|LOCK_NB)`: `.cargo-build-lock` SH blocked and EX blocked (errno 35, EWOULDBLOCK); `.cargo-lock` SH acquired, EX blocked; `.cargo-artifact-lock` SH/EX blocked. So "a cargo is running here" = `flock(.cargo-build-lock, LOCK_SH|LOCK_NB)` fails; a `LOCK_SH` probe on `.cargo-lock` can never detect cargo >= current.
- **rustc incremental session lock**: `compiler/rustc_data_structures/src/flock/unix.rs` (1.97.1; macOS selects `unix`, Linux has a separate `linux.rs`) uses **`fcntl(fd, F_SETLK | F_SETLKW, &flock{l_type, l_whence: SEEK_SET, l_start:0, l_len:0})`** (POSIX record locks), with an in-process `LOCK_REGISTRY` to avoid reopening (closing any fd of the file drops the lock). Lock files are `target/<profile>/incremental/<crate>-<hash>/s-<ts>-<hash>.lock` (listed in klynt, e.g. `s-hnih1siuhv-0ki79k4.lock` next to its `-cn4qw4...` session dir).
- **Cross-interaction on this macOS (experiment, two Python processes)**: a `fcntl` write lock held by process A blocks `flock` LOCK_EX, LOCK_SH, and `fcntl` EX in process B (errno 35); a `flock` LOCK_EX held by A blocks `fcntl` EX/SH and `flock` EX in B. Within one process, a second fd's `flock` or `fcntl` attempt is also blocked after a `flock` is held. So on Darwin `flock` and `fcntl` locks conflict with each other: a single `flock(LOCK_*|LOCK_NB)` probe detects both Cargo's and rustc's locks. Caveat: a probe must not `fcntl`-lock-and-close fds of the same file in-process if hive itself holds an fcntl lock (POSIX close semantics).

Design-relevant takeaways
- Use `flock(LOCK_SH|LOCK_NB)` on a read-only fd (no O_CREAT, no mtime change) of `<profile>/.cargo-build-lock` for "cargo is building"; for older Cargo also `LOCK_EX|LOCK_NB` on `.cargo-lock`. Handle EWOULDBLOCK=35 as "held" and ENOENT as "no lock file ever taken" (not held).
- For rustc incremental dirs, the same `flock` probe on `incremental/*/s-*.lock` detects an `fcntl`-held lock on this OS; doctor check (d) should verify exactly that cross-syscall case with a child process holding an `fcntl` lock.
- Holding a `LOCK_SH` during a deletion makes a starting cargo wait (its `lock()` blocks), not fail; keep hold times short.

## 5. `cargo metadata --no-deps --format-version 1`

Throwaway crates in `/private/tmp/scan-meta`, `CARGO_HOME` pointed at an empty temp dir, `HTTPS_PROXY`/`CARGO_HTTP_PROXY=http://127.0.0.1:9`, `CARGO_NET_RETRY=0` (dead proxy: any network attempt would fail).

- A crate with no dependencies and a crate with a crates.io dependency (`serde = "1"`), no `Cargo.lock`, empty `CARGO_HOME`: both exit 0, stdout 964 and 1172 bytes, **no `Cargo.lock` created, `CARGO_HOME` left empty, no network attempted**. `--locked` and `--offline` also succeed. A path dependency that does not exist (`../nope`) also succeeds under `--no-deps`. By contrast, `cargo metadata` without `--no-deps` for the serde crate fails (`Updating crates.io index ... failed to get serde`).
- Top-level keys (cargo 1.97.1): `build_directory, metadata, packages, resolve, target_directory, version, workspace_default_members, workspace_members, workspace_root`. `version` is `1`. **New field `build_directory`** (absolute) exists and can differ from `target_directory`.
- `target_directory` follows the environment: relative `CARGO_TARGET_DIR=rel/t` returns `<cwd>/rel/t` (absolute, not created); `.cargo/config.toml` `build.target-dir = "rel-cfg"` returns `<crate>/rel-cfg`; `{workspace-root}` is NOT expanded in `target-dir` (it returned the literal `<crate>/{workspace-root}/tgt-x`), while `build.build-dir = "{cargo-cache-home}/bd/{workspace-path-hash}"` is expanded (`build_directory` = `<CARGO_HOME>/bd/4b/1c6a5df82a038e`), and then `.fingerprint/` lives under the build dir rather than the target dir.
- Run from a workspace member dir (virtual workspace): `workspace_root` = the workspace root and `target_directory` = `<root>/target`. `cargo locate-project --workspace` prints `{"root": ".../Cargo.toml"}` and needs no resolution either.

Design-relevant takeaways
- `--no-deps` is safe offline and does not write a lockfile for these cases; still not a read-only guarantee in states not tested (stale/malformed lock, `[patch]`, `rust-toolchain.toml` that triggers a rustup download of a missing toolchain, `-Z` config). Run it with `--locked --offline` and a scrubbed env; do not point `CARGO_HOME` at a temp dir in production, since that hides the user's real `config.toml` (the temp `CARGO_HOME` here was only to prove no writes and no network).
- Resolve `target_directory` and `build_directory` both; CORE-4.4 compares to the former, but fingerprints are in the latter. With default config they are equal.
- The result depends on `CARGO_TARGET_DIR` and the user's config (relative paths are made absolute against the cwd); the open question about environment dependence stands. Alternative that avoids invoking cargo: read `CARGO_TARGET_DIR`/`.cargo/config.toml` yourself, but `{workspace-path-hash}`-style templates make that non-trivial.

## Unknowns

- Whether stable Cargo will ship the `build/<pkg>/<hash>/fingerprint/` layout and when; nightly 1.100.0-nightly (2026-08-15) already writes it by default with no flag found. Which release and whether `.fingerprint/` still appears alongside it were not determined.
- Mixed-`rustc` profile dirs (units built by two toolchains) were not produced; whether hive should treat them as supported is open.
- Whether `invoked.timestamp` mtime updates on every build or only on recompile was not measured.
- Behaviour of `cargo metadata --no-deps` with a stale or malformed `Cargo.lock`, `rust-toolchain.toml` pointing at an uninstalled toolchain, or `-Z`-gated manifests was not tested; "read-only" is shown only for the states above.
- `evaluatePolicy` reply-block threading and whether a CFRunLoop is required to receive it in a CLI (ROAD-7); not run because it would present a dialog.
- No-GUI-session behaviour of `canEvaluatePolicy` (CORE-6.4) still unverified (carried from req-review).
- `rustc` incremental lock behaviour with a live rustc was inferred from source plus the generic flock/fcntl cross-conflict experiment, not observed against a running rustc.
- Dependency counts are for the versions resolved today (applesauce 0.8.10); a Cargo.lock pin may change them.
- `bench-<name>` hash-file prefix was assumed from Cargo's target kinds, not observed on disk.
