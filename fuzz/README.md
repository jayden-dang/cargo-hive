# Compiler-reader fuzzing

The nine targets in `targets.txt` exercise production readers through small
`cfg(fuzzing)` adapters. Each combines arbitrary input with independently expected
valid structures or metadata. The same model assertions run in ordinary unit
tests under Rust 1.97.1 and 1.100.0. Fuzz dependencies belong only to this separate,
unpublished workspace. Normal builds and installation need no nightly compiler or
fuzz engine.

## Tooling and one-command run

The tested pins are cargo-fuzz **0.13.2**, libfuzzer-sys **0.4.13** (also locked in
`Cargo.lock`), and **nightly-2026-11-17**, rustc commit
`5df8c02afdf6b882bc301442cea70f026e3d3974`. The runner checks the compiler commit and
macOS host before building. Use an isolated tool root; keep the normal Rustup
default unchanged:

```sh
rustup toolchain install nightly-2026-11-17 --profile minimal
cargo install cargo-fuzz --version 0.13.2 --locked --root fuzz/tools
CARGO_FUZZ="$PWD/fuzz/tools/bin/cargo-fuzz" scripts/fuzz-readers.sh 30
```

An already-installed alias is accepted only if its compiler commit matches the
pin. Set `FUZZ_TOOLCHAIN=nightly` for such an alias. The runner uses that toolchain
explicitly, rather than inheriting the normal workspace's compiler. Instrumented
builds stay under `fuzz/target/` with address sanitization, debug assertions and
overflow checks enabled by cargo-fuzz's defaults. No build-std option is needed.

An optional second argument selects one registered target:

```sh
CARGO_FUZZ="$PWD/fuzz/tools/bin/cargo-fuzz" scripts/fuzz-readers.sh 600 debug_map
```

With no selector the runner executes all nine targets serially. A full acceptance
run takes at least 90 minutes plus build/replay overhead. Do not run local campaigns
alongside ROAD-17 timing measurements. `scripts/test-fuzz-readers.sh` checks invalid
arguments and retained replay/mutation failures using a disposable engine fixture.

## Target registry

| Target | Production entry points exercised | Oracle and ordinary regression location |
|---|---|---|
| `fingerprint` | core fingerprint `read_unit`/shared `decode_unit`, `read_hash`, `read_capped`, `dependency_name` (also used by `dep_name`), `coverage_of` | Exact typed dependencies, feature coverage, sibling-hash identity for both layouts, empty-hash NotCurrent, malformed signatures and JSON truncation; core fingerprint unit tests |
| `diagnostics` | core `is_summary_line`/`error_code`; GC explain `read_output_texts` | Known summary/error codes, deduplication and malformed-record indication while retaining complete records; core fingerprint and GC explain unit tests |
| `dep_info` | seed `read_dep_info_line` (also used by file reader), `is_registry_dep_info`, `classify_dep_lines` (unit-dir aggregation) | Escaped/Unicode prerequisites, prefix lookalikes, first-input classification and Workspace veto; seed source unit tests |
| `rustc_info` | seed `rustc_info_version`, `read_capped_stream`, `hash_matches` (also used by store-staging `hash_unchanged`) | Whole stdout from one matching output entry, absent/ambiguous output, UTF-8/size errors and hash equality; seed fill unit tests |
| `debug_map` | GC `debug_map::read`, thin/fat parsing and checked range reads | Exact loose-object set, archive count and fat union, malformed/truncated tables, slice containment and offset overflow; GC debug_map unit tests |
| `target_tag` | core tag decoder and both native/positional bounded read helpers used by `identify`/`identify_bounded` | Identifying line, short-read accumulation, exact/over-size limit and legacy I/O-result mapping; core target unit tests |
| `incremental_names` | GC session/lock timestamp grammar, `crate_dir` and `is_temp_entry` | Session/lock association, age boundary, base36 overflow and entry kinds; GC plan unit tests; no deletion runs |
| `artifact_names` | core hash/layout predicates and embedded hashes; GC explain `mode_index`, `unit_mode`, `directory_crate`, `crate_and_kind` | Unicode and hex boundaries, stable path joins, Build/Check/Unknown classification; core unit_path and GC explain unit tests |
| `timestamp_metadata` | GC `in_window`, `file_time`, `invoked_in_window`, `NotCurrentNode::last_wanted` | Cutoff/future/large-window values, missing versus unreadable metadata and latest hash/stamp time; GC liveness unit tests |

`invoked.timestamp` has no interpreted body: its mtime is modeled. Ancillary
display/cache timestamp queries do not independently authorize deletion. Binary
fingerprint dep files, build-script output/root-output, rlib/rmeta bodies and
Cargo.lock byte comparisons are copied or compared without decoding; no content
parser is invented for them. Hive state, Git/config readers and subprocess JSON
are outside this file-reader issue.

## Input and resource policy

Seed files are small synthetic examples, not private workspace captures. Each
file must be regular, nonempty and at most 65,536 bytes before replay. This check
matters because libFuzzer can truncate an oversized replay file to its input cap.
The checked-in corpus is read-only during a run; discoveries go into a scratch
copy. The runner verifies that the manifest and registry contain the same targets.

A whole-run watchdog also covers tool preflight, Cargo compilation and replay.
Its wall-clock allowance is 600 seconds for setup plus each selected target's
requested mutation time and 40 seconds for replay/teardown. `FUZZ_RUN_TIMEOUT`
can select a different explicit deadline (1 to 1,000,000 seconds). Timeout exits
124 and interruption exits 130. Both stop the owned process group, including
compiler/fuzzer descendants, and retain an incomplete-phase record. A process
killed before producing an input is reported as an incomplete reproduction.

Each phase uses a 5-second input timeout, 2,048 MiB RSS guard, 65,536-byte raw-input
cap, a recorded engine seed (default 20261123), final engine statistics and a
target-specific artifact directory. Replay passes individual files, which runs
them without mutation. The subsequent phase passes a writable corpus directory
and the requested total-time limit. See the owning
[libFuzzer documentation](https://llvm.org/docs/LibFuzzer.html#options).

Large production boundaries are tested with bounded streams and sparse read
models. Fuzz input cannot request host filesystem writes, run a build or trigger
GC deletion. The read-only fixture fails if such an operation is attempted.

Truncation is format-specific. An empty fingerprint hash means NotCurrent; a
complete diagnostic line remains a valid record. A textual dep-info prefix that
still satisfies the existing first-prerequisite rule cannot be distinguished
from a complete valid token without information the format does not supply.
Required binary ranges and incomplete JSON structures must be refused. Fuzzing
does not establish a universal corruption-detection guarantee.

## Evidence and replay

Runs create a fresh `fuzz/reports/run.*` directory. `FUZZ_REPORT_DIR` may select a
new directory explicitly; an existing directory is refused to preserve earlier
evidence. Each run records source commit/tree and dirty state, toolchain, budget
and timestamps. Each target retains phase commands, full logs, exit statuses,
elapsed seconds, writable corpus, failure inputs and engine statistics. Raw logs
carry coverage, execution count and peak RSS when the engine reports them; missing
statistics are stated explicitly. A killed process without an input artifact is
an incomplete reproduction, not a passing run.

Replay a retained file using the same pin and resource guards:

```sh
RUSTUP_TOOLCHAIN=nightly-2026-11-17 fuzz/tools/bin/cargo-fuzz fuzz run \
  --fuzz-dir fuzz debug_map path/to/retained-input -- \
  -timeout=5 -rss_limit_mb=2048 -max_len=65536
```

Preserve the original input before minimization. `cargo-fuzz fuzz tmin` can reduce
it; see the [cargo-fuzz guide](https://rust-fuzz.github.io/book/cargo-fuzz/guide.html).
Promote each confirmed discovered defect into an ordinary regression before a
correction, verify it fails without the correction, then run the affected
campaign again. Describe only the bounded campaign performed, including voids or
incomplete targets. Keep local absolute paths and private data out of committed
summaries and seeds.

## CI and acceptance

The separate `reader fuzzing` workflow runs 30 seconds per target on relevant pull
requests and 300 seconds per target on its daily schedule or manual dispatch. It
retains reports even after failure and has a 90-minute whole-job deadline. The
existing merge gate remains unchanged. Scheduled runs start only after this
workflow reaches the default branch, subject to
[GitHub's scheduling behavior](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#schedule).

Feature acceptance requires seed replay and 600 seconds per target on the final
reviewed source, ordinary regression/removal controls, both native toolchain
gates, package verification and whole-branch review. A CI configuration file is
not evidence that its hosted job ran. No completed campaign is claimed by this
guide; the branch's acceptance report records observed results separately.
