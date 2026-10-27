# Requirements factual review: CORE (2026-10-25-foundation/requirements.md)

Scope: only whether claims about external systems are true on this Mac (Darwin 27.0.0, APFS, git 2.54.0 Apple Git-157, cargo 1.97.1, applesauce-cli 0.5.30). All experiments ran read-only against real repos or in `/private/tmp/req-review-*` (removed afterwards).

## 1. CORE-4.1 CACHEDIR.TAG and `.fingerprint/`

Method: `cat -A` the tag in both target dirs and the moon cache; list `*/.fingerprint`.

Evidence (both `backend/target` and `~/.cache/cargo-target/klynt` are byte-identical, 177 bytes):

```
Signature: 8a477f597d28d172789f06886806bc55
# This file is a cache directory tag created by cargo.
# For information about cache directory tags see https://bford.info/cachedir/
```

- Profile dirs: `backend/target/debug/.fingerprint` and `~/.cache/cargo-target/klynt/debug/.fingerprint` both exist.
- Moon tag (`klynt/.moon/cache/CACHEDIR.TAG`): line 2 is `# This file is a cache directory tag created by moon.` (differs; also no `.fingerprint`).

Verdict: TRUE. Note the cargo line is line 2, after the `Signature:` line, and the file has a third line; "contains the line" is accurate, "is exactly" would not be.

## 2. CORE-4.8 deps resolution via little-endian hex

Method: python script (read-only) over every `.fingerprint/<unit>/` of `backend/target/debug` (2812 dirs; also a 300-unit random sample) and `~/.cache/cargo-target/klynt/debug`. For each `deps` entry, took the last element (u64) and looked it up among the 16-char contents of the per-unit short hash files (`lib-<name>`, `run-build-script-*`, `build-script-*`, `test-*`, `bin-*`).

Entry shape: `[435413754877081646, "domain", false, 17112352188342185299]` (u64 pkg-id hash, name, public flag, u64 fingerprint). The final element is the u64.

Evidence:

| Set | Edges | LE-bytes hex (`u64.to_bytes(8,'little').hex()`) | plain `%016x` of the u64 (big-endian reading) |
|---|---|---|---|
| backend/target, 300-unit sample | 2656 | resolved 2656, unresolved 0, ambiguous 0 | unresolved 2656 |
| backend/target, all units | 24855 | resolved 24855, unresolved 0, ambiguous 0 | unresolved 24855 |
| ~/.cache/cargo-target/klynt, all units | 4745 | resolved 4745, unresolved 0, ambiguous 0 | unresolved 4745 |

Example hash file: `lib-adler2` contains `a62f178930ee1228` (16 chars, no newline).

Verdict: TRUE, with a wording caution. "16-character little-endian hex" is correct only in the byte-order sense: the file text is the hex of the u64's 8 little-endian bytes (byte-swapped relative to a normal `{:016x}` print). A naive `format!("{:016x}", u64)` resolves nothing. Suggested wording: "encoded as the lowercase hex of the u64's 8 little-endian bytes (16 characters, i.e. the byte-swap of `{:016x}`)". Ambiguity is 0 on this data (all short hash files unique), but a hash collision across files is possible in principle, so the ambiguous counter is untested here.

## 3. CORE-4.7 unit = one directory under `.fingerprint/`

Method: same script, listing contents of each dir.

Evidence (backend/target/debug/.fingerprint): 2812 entries, all directories; 0 dirs without a `*.json`; 0 dirs with more than one `*.json`. Every unit dir has exactly one fingerprint JSON. Unit kinds seen by file names:

- `libc-6c73...`: `dep-lib-libc invoked.timestamp lib-libc lib-libc.json`
- `libc-9c5b...` (build script compile): `build-script-build-script-build{,.json} dep-build-script-build-script-build invoked.timestamp`
- `libc-02cf...` (build-script run): `run-build-script-build-script-build{,.json}` only (no `dep-*`, no `invoked.timestamp`)
- test units: `test-lib-<name>`, `test-integration-test-<name>`, `test-bin-<name>` (+ `.json`); some dirs also have `output-lib-<name>` (compiler diagnostics cache, not a hash file).

Verdict: TRUE on this data. Every dir has a JSON, so "units = dirs" is well defined here. Caveats for the implementer: the hash file name is not always `lib-*`; the JSON file may be absent in a dir mid-build (not observed); `output-*` and `dep-*` and `invoked.timestamp` files must be ignored when indexing short hash files.

## 4. CORE-3.1 / 3.7 `git worktree list --porcelain` and missing dirs

Method: throwaway repo in `/private/tmp/req-review-git`, main + two linked worktrees, `rm -rf` one, list.

Evidence after `rm -rf wt2`:

```
worktree /private/tmp/req-review-git/wt2
HEAD 95f3...
branch refs/heads/wt2b
prunable gitdir file points to non-existent location
```

The other entries list normally; the list works from inside a linked worktree and shows the main first. Extra probe: with the worktree locked (`git worktree lock`) and its directory moved away, the entry shows `locked x` and NO `prunable` line (git does not mark locked worktrees prunable).

Verdict: PARTLY. Linked worktrees are reported, and a deleted directory is marked `prunable` in the normal case. But `prunable` is absent for locked worktrees whose directory is missing. Suggested wording for CORE-3.7: "WHEN a worktree reported by `git worktree list` has a directory that does not exist (checked by hive with a filesystem stat; git's `prunable` marker is not sufficient because locked worktrees are not marked) THE SYSTEM SHALL list it as missing ...".

## 5. CORE-2.1 root commits

Method: repo with two unrelated roots (orphan branch merged with `--allow-unrelated-histories`), linked worktree, and a `git clone`.

Evidence: `git rev-list --max-parents=0 HEAD` printed the same two hashes (`5ff72d9d...`, `e31ae413...`) in main, in the linked worktree, and in the clone.

Verdict: TRUE. Output order is not guaranteed stable, so sort before comparing or storing. (A shallow clone would give a different, truncated root set; not tested.)

## 6. CORE-2.2 linked vs main

Evidence:

```
main:  .git is a dir
wt1:   .git is a file: "gitdir: /private/tmp/req-review-git/main/.git/worktrees/wt1"
clone: .git is a dir
main  git-dir=.git                                   common=.git
wt1   git-dir=<main>/.git/worktrees/wt1              common=<main>/.git
```

Verdict: TRUE. In a linked worktree `--git-dir` differs from `--git-common-dir`; in a main checkout they are equal. `.git` file vs dir also works. Caveats: a submodule's `.git` is also a file (pointing into `.git/modules/`), so "file" alone would misclassify submodules; compare git-dir with git-common-dir. `--git-dir` returns a relative path (`.git`) in the main checkout, so resolve or use `--absolute-git-dir` before comparing. The main checkout's path is derivable as the parent of `--git-common-dir` (non-bare) or from the first entry of `git worktree list`.

## 7. CORE-1.1(c) clone id and SHARES_ALL

Method: python ctypes `getattrlist(path, attrlist{bitmapcount=5, forkattr=0x8|0x100|0x200}, buf, 64, FSOPT_ATTR_CMN_EXTENDED=0x20)` on an 8 MiB random file in `/private/tmp` (APFS). Buffer layout: u32 length, i64 PRIVATESIZE, u64 CLONEID, u64 EXT_FLAGS. Do NOT set `commonattr=0x80000000` (that is ATTR_CMN_RETURNED_ATTRS and prepends a 20-byte attribute set; my first run misread it). Flag values from the SDK `sys/stat.h`: `EF_MAY_SHARE_BLOCKS=0x1`, `EF_SHARES_ALL_BLOCKS=0x40`.

Evidence:

```
src alone              private=8388608 cloneid=223057924 ext_flags=0x0
src after cp -c        private=0       cloneid=223057924 ext_flags=0x41  SHARES_ALL
clone after cp -c      private=0       cloneid=223057924 ext_flags=0x41  SHARES_ALL
src after rewrite      private=8388608 cloneid=223057924 ext_flags=0x1   (MAY_SHARE only)
clone after rewrite    private=8388608 cloneid=223057926 ext_flags=0x1
```

Verdict: TRUE for (a) (private 0 for both after clone) and (c) (same clone id, SHARES_ALL on both, rewrite gives the clone a new id). Detail the spec omits: after the rewrite the ORIGINAL keeps its old clone id and loses SHARES_ALL, keeping only `MAY_SHARE_BLOCKS` (stale), and both files then report full private size. The task brief's "EF_SHARES_ALL_BLOCKS=0x40" is correct; `0x20` is not the MAY_SHARE flag (it is 0x1).

## 8. CORE-1.1(e) / CORE-6.4 LocalAuthentication availability query

Method: swiftc program in `/private/tmp` calling only `LAContext().canEvaluatePolicy(.deviceOwnerAuthentication, error:)`, run from the agent shell (no TTY input, `</dev/null`, not over SSH).

Evidence: `canEvaluate=true err=nil`, wall time 0.372 s, returned without blocking. A second probe with `.deviceOwnerAuthenticationWithBiometrics` returned `false` with `Code=-4 "Touch ID is not available in closed clamshell mode."` (so the biometric variant is machine-state dependent; `.deviceOwnerAuthentication` stayed true because of the password fallback).

Verdict: TRUE (query works from a command-line binary, no blocking). UNVERIFIABLE-HERE: the absence of any visible dialog cannot be observed from this session, only that the call returned in 0.37 s without input; and the "no GUI session" harness case of CORE-6.4 (e.g. pure SSH or a launchd daemon with no Aqua session) was not run.

## 9. CORE-1.1(b) Applesauce preserves hardlink groups and mtimes

Method: `applesauce compress` (CLI 0.5.30) on /private/tmp fixtures of a 3.4 MB compressible file; crate source `applesauce` 0.8.10 downloaded to /private/tmp and grepped; crates.io/docs.rs checked.

Evidence:

- Solo file: compressed (`flags=compressed`, blocks 6648 to 24), content identical (`cmp`), mtime unchanged (2020-01-02 preserved).
- Hardlinked pair (same dir: `Total Files: 1`; separate dirs: `Total Files: 0 / New Files Compressed: 0`): neither link was compressed. After the run: same inode, links=2, `flags=-`, blocks unchanged 6648, content identical, mtime unchanged.
- Source: `src/threads/mod.rs:208-211`: `let link_count = metadata.nlink(); // We don't want to break the hard link, so we skip it.` then `SkipReason::HardLink` ("Other hard links exist to file"). Test `compress_with_hardlinks` (`src/lib.rs:517-537`) asserts both files are skipped.
- Library API: crate `applesauce` 0.8.10 (published 2026-09-27) exposes `pub mod info`, `pub mod progress`, `pub use applesauce_core::compressor`, `pub struct FileCompressor` with `new`, `recursive_compress`, `recursive_decompress`. So compression is available as a library API, not just a CLI.

Verdict: PARTLY. Hardlink groups and mtimes are preserved, but only because Applesauce does not compress any file with link count above 1; it silently skips it. SPIKES.md §5's "correct on hardlinks" is true in that sense (no data loss), but the 23,537 hardlinked files in that corpus were therefore left uncompressed. A doctor check that compresses a hardlinked file will see it unchanged and "uncompressed", so (b) as worded ("a file compressed ... with its hardlink group ... unchanged") must use a non-hardlinked file for the round trip. Suggested wording: "a file compressed with the embedded Applesauce library reads back byte-for-byte identical with its mtime unchanged; a file that has other hardlinks is skipped by Applesauce and its hardlink group, content and mtime remain unchanged."

## 10. CORE-4.4 resolved target dir, read-only

Method: snapshot (path, mtime, size) of `backend/` top level (excl. target), `backend/target` to depth 2, `~/.cargo` top level, `~/.cargo/.global-cache`, `~/.cargo/registry/index` (depth 3), `~/.cargo/registry/cache`; ran `cd backend && SQLX_OFFLINE=true cargo metadata --no-deps --format-version 1`; snapshot again; diff. 1799 entries compared.

Evidence: output `target_directory /Users/jayden/Developer/klynt/klynt/backend/target`, `workspace_root .../backend`; stderr empty; diff of the snapshots: no difference (Cargo.lock untouched). `CARGO_TARGET_DIR` was unset in this shell and no `config.toml` exists in `~/.cargo` or `backend/.cargo`. With `CARGO_TARGET_DIR=/private/tmp/zzz` the same command reported `/private/tmp/zzz` (without creating it), so the answer depends on the environment of the invoking process.

Verdict: TRUE as an observation (read-only for these paths, with `--no-deps` and a populated lockfile/registry). Not shown: behavior when Cargo.lock is missing or stale (Cargo may write it) or when the registry index must be fetched; `--no-deps` here did not need them. Also note this contradicts nothing in the requirements but does run `cargo`, which the Open Question flags against "GC never invokes cargo". The target dir klynt uses for the `~/.cache/cargo-target/klynt` dir is not what Cargo resolves in this shell (it resolves `backend/target`), so that cache dir would be an orphan candidate under CORE-4.4/4.5 unless the user's builds set `CARGO_TARGET_DIR`/config differently.

## Summary

| # | Claim | Verdict |
|---|---|---|
| 1 | CORE-4.1 tag line and `.fingerprint/`; moon tag differs | TRUE (cargo line is line 2 of 3, after `Signature:`) |
| 2 | CORE-4.8 deps tail u64 as 16-char little-endian hex resolves to one unit | TRUE (24855/24855 and 4745/4745 resolved, 0 ambiguous; big-endian reading resolves 0); wording needs "hex of LE bytes" |
| 3 | CORE-4.7 unit = one dir under `.fingerprint/` | TRUE (2812 dirs, each exactly one JSON; hash file names vary by kind) |
| 4 | CORE-3.1/3.7 worktree list marks missing dir `prunable` | PARTLY (not for locked worktrees) |
| 5 | CORE-2.1 root commits shared by worktree and clone | TRUE |
| 6 | CORE-2.2 linked worktree detectable by `.git` file or git-dir vs common-dir | TRUE (submodules also have a `.git` file; prefer git-dir comparison) |
| 7 | CORE-1.1(a)(c) clone shares all blocks, same clone id, rewrite gives new id | TRUE (original keeps old id and drops SHARES_ALL) |
| 8 | CORE-1.1(e)/6.4 `canEvaluatePolicy` from CLI, no dialog | TRUE for the query; no-dialog and no-GUI-session cases UNVERIFIABLE-HERE |
| 9 | CORE-1.1(b) Applesauce preserves hardlinks and mtimes | PARTLY (preserved only because hardlinked files are skipped, not compressed); library API exists |
| 10 | CORE-4.4 `cargo metadata --no-deps` read-only | TRUE for this repo state; depends on invoking environment |
