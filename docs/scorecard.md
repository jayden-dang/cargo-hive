# Worktree scorecard

What an agent worktree costs, cold and seeded, on the measured repo. A cold checkout is one hive did not seed. A seeded checkout is one that `cargo hive seed` seeded just before its first build. Every durable value in rows R1 to R7 is an integer: durations in milliseconds, sizes in bytes. R8, R9 and R10 are text. A cell reading `not yet measured` has no run behind it yet.

## Values

| Row | Cost | Source | Cold n | Cold median | Seeded n | Seeded median | Target (cold) | Target (seeded) | Target set |
|---|---|---|---|---|---|---|---|---|---|
| R1 | First-command time for `build` (ms) | `cold_build_ms_median` / `seeded_total_ms_median` | 4 | 90673 | 4 | 79340 | open — operator | open — operator |  |
| R2 | First-command time for `check --all-targets` (ms) | `cold_build_ms_median` / `seeded_total_ms_median` | 4 | 62946 | 4 | 46561 | open — operator | open — operator |  |
| R3 | First-command time for `clippy --all-targets` (ms) | `cold_build_ms_median` / `seeded_total_ms_median` | 4 | 60742 | 4 | 56694 | open — operator | open — operator |  |
| R4 | First-command time for `test --no-run` (ms) | `cold_build_ms_median` / `seeded_total_ms_median` | 4 | 97891 | 4 | 86345 | open — operator | open — operator |  |
| R5 | First-command time for `check -p <package> --lib` (ms) | `cold_build_ms_median` / `seeded_total_ms_median` | 4 | 42375 | 4 | 35282 | open — operator | open — operator |  |
| R6 | Next build after a one-file edit in a workspace crate (ms) | by hand (outside bench) | 4 | 6631 | 4 | 6821 | open — operator | open — operator |  |
| R7 | Private bytes each new worktree adds (bytes) | by hand (outside bench): free space | 4 | 5210083328 | 4 | 3475857408 | open — operator | open — operator |  |
| R8 | Units rebuilt because of a hive deletion | by hand (outside bench): cargo -v after gc --checkout | 4 | 0 rebuilt over 4 non-vacuous samples; 0 vacuous | 4 | 0 rebuilt over 4 non-vacuous samples; 0 vacuous | open — operator | open — operator |  |
| R9 | Stale results on a probe that changes a file a proc macro reads without declaring it | by hand (outside bench): cargo -v after probe | 4 | 4 stale of 4 | 4 | 4 stale of 4 | open — operator | open — operator |  |
| R10 | Setup footprint | listed | — | nothing | — | `cargo install cargo-hive`, `cargo hive doctor`, `cargo hive repo add <main>`; Cargo config files 0, environment variables 0, rustc wrappers 0, cargo shims 0; files outside the state root: 0, beyond clones into target dirs | open — operator | open — operator |  |

R8 cells read `R rebuilt over k non-vacuous samples; m vacuous`, or `all vacuous`. R9 cells read `s stale of n`.

R8 reads 0 rebuilt even when gc deleted only debug objects and no unit files; see the R8 note.

## Provenance

| Row | Measured on | Hive commit | Toolchain | Machine | Evidence |
|---|---|---|---|---|---|
| R1 | 2026-11-23 | 79adee19822d | 1.97.1 | Mac17,9, Apple M5 Pro, 18 | .skills/verify/2026-11-22-scorecard |
| R2 | 2026-11-23 | 79adee19822d | 1.97.1 | Mac17,9, Apple M5 Pro, 18 | .skills/verify/2026-11-22-scorecard |
| R3 | 2026-11-23 | 79adee19822d | 1.97.1 | Mac17,9, Apple M5 Pro, 18 | .skills/verify/2026-11-22-scorecard |
| R4 | 2026-11-23 | 79adee19822d | 1.97.1 | Mac17,9, Apple M5 Pro, 18 | .skills/verify/2026-11-22-scorecard |
| R5 | 2026-11-23 | 79adee19822d | 1.97.1 | Mac17,9, Apple M5 Pro, 18 | .skills/verify/2026-11-22-scorecard |
| R6 | 2026-11-23 | 715cf4b5ade6 | 1.97.1 | Mac17,9, Apple M5 Pro, 18 | .skills/verify/2026-11-23-scorecard |
| R7 | 2026-11-23 | 715cf4b5ade6 | 1.97.1 | Mac17,9, Apple M5 Pro, 18 | .skills/verify/2026-11-23-scorecard |
| R8 | 2026-11-23 | 715cf4b5ade6 | 1.97.1 | Mac17,9, Apple M5 Pro, 18 | .skills/verify/2026-11-23-scorecard |
| R9 | 2026-11-23 | 715cf4b5ade6 | 1.97.1 | Mac17,9, Apple M5 Pro, 18 | .skills/verify/2026-11-23-scorecard |
| R10 | listed | listed | listed | listed | listed |

Run evidence is local to the measuring machine. The Evidence cells name a directory under `.skills/verify/`, which git ignores, so no clone can open it, and the committed values are copies of that run's `summary.txt`. The directory holds: the bench receipts `R1.json` to `R5.json` and any `<row>.refusal.json`; `pipe.jsonl`, one line per by-hand sample with its void reasons, and the per-sample receipts and build logs `pipe-<arm><i>-*`; `summary.txt` and `cells.tsv`, which `summarize` writes; `load-start.txt` and `load-end.txt`; `doctor.json` and `hive.txt` (the hive binary's path, its version and its build profile); and `preflight-fail.txt` when preflight stopped.

Medians are only as quiet as the machine they were measured on. Preflight records the load average at the start (`load-start.txt`), `summarize` records it at the end (`load-end.txt`) and prints a warning when the start load was above half the logical core count, and `summary.txt` holds the min, max and spread per arm; read the medians together with them. "Measured on" is the date the summary was written, so a run that straddles midnight shows the later date.

**Latest run** (rewritten by hand at each rerun; `record` never writes it). The hive used for both sessions was a debug build (`target/debug`), so the seed times inside the seeded values of R1 to R5 and R7 come from a debug-built seed; those seeded values are therefore not a release-build figure. The run was on a non-quiet machine. The 1-minute load average was 1.59 at the start and 10.95 at the end of the first session (R1 to R5), and 4.14 at the start and 6.85 at the end of the second (R6 to R9); the second session's start already had 5- and 15-minute averages of 7.79 and 8.81. Spread (max minus min) as a share of the median, cold arm and seeded arm: R1 9.9 and 11.7 percent, R2 32.5 and 16.0, R3 24.7 and 31.1, R4 38.9 and 16.4, R5 60.9 and 15.4; R6, from the second session, 5.6 and 40.5 (one slow seeded sample); R7, from the second session, 1.2 and 11.6. R1 to R5 and R6 to R9 come from two sessions on two nights, each with its own evidence directory, so they carry different hive commits and evidence paths; every row shows the same "Measured on" date because it is the date the summary was written. The two hive commits in the Provenance column differ only by documentation: the later one adds two docs-only commits on top of the first, and `git diff --stat <first>..<second> -- crates` shows nothing.

## Targets

The operator sets a target by editing that row's Target cell and its Target set date in a plain commit. A rerun never touches either.

## Notes per row

- R1 to R5: bench times one first build per fresh checkout. Rows measured later in a run see a store and a main target dir that earlier shapes already filled, so R2 to R5 are noted as measured after earlier shapes.
- R6: by hand (outside bench); environment matches bench's, but bench did not time it. The environment is the same variable scrub that hive applies to the builds it runs. Only the cargo exec is timed, with the environment arguments built before the window, after a fixed one-file edit to a workspace crate. The median follows bench's rule: the middle value for odd n, and the floor of the mean of the two middle values for even n.
- R7: the bytes the volume lost, read as the drop in free space from before a worktree's first build to after it, never bytes that clones share. The reading also carries the seed's store writes, any Cargo source unpacks, and any other writer on the same APFS container, so the quiet-machine rule and the median over four samples are the only guards. On the seeded arm the seed receipt's `clone_bytes_not_shared` is recorded beside the value as the seed's registry part at seed time; it is 0 on a fully shared clone and never stands in for the drop.
- R8: a sample is vacuous when the deletion removed no unit files and no objects, because then nothing could be rebuilt because of it. A vacuous sample is valid and counted. A sample is void when the deletion or the rebuild fails. Each pipeline line holds `r8_units`, the count of lines in the saved rebuild log matching ``Running `.*--crate-name`` (one per unit compiled; a `Compiling` line is one per package and is not used), with `r8_unit_files_deleted` and `r8_objects_deleted` from the saved gc receipt's unsuffixed totals and `r8_vacuous`, true when both totals are 0. A vacuous sample's `r8_units` is kept but is not a rebuild count. A sample whose gc deleted objects but 0 unit files is non-vacuous, so a cell reading 0 rebuilt says nothing about unit-file deletion when `r8_unit_files_deleted` is 0; the per-sample `r8_unit_files_deleted` and `r8_objects_deleted` are in the run's `pipe.jsonl`. The cell reads `R rebuilt over k non-vacuous samples; m vacuous`, with R the median of `r8_units` over the non-vacuous, non-void samples of the arm, or `all vacuous` when every sample is vacuous; void samples are in neither count.
- R9: a sample is stale when the rebuild after the probe change exits 0 and Cargo reports `P` as `Fresh` under `-v`; it is `fresh-recompiled` when the rebuild recompiles `P`, whatever its exit code; and it is void when the control rebuild before the change does not exit 0 without compiling a unit, or when the probe rebuild exits non-zero without recompiling `P`. The probe flips the first `nullable` entry of the `describe` section of the `.sqlx` query file, which changes the type the macro emits; with a `.env` it changes the value of its first variable. The cell reads `s stale of n`, with n the valid samples of the arm; a void sample and a `probe input absent` sample are not counted. Seed clones registry units and not workspace crates, so the probe measures Cargo's own input tracking in both arms and does not single out staleness hive caused. A stale sample is the unfavourable outcome: Cargo reported the crate Fresh after the probe change, so it did not notice the changed input. Both arms showing it means hive neither causes nor fixes it.
- R10: listed, not measured. Its Provenance cells read `listed` because the row is a list of commands and file counts re-checked against the README at each rerun, not a run's measurement.

## What a run touches

- The real hive state root: its capabilities file (doctor), the repo registry (`cargo hive repo add "$MAIN"`, written only when the main is not yet registered), the store, the store database and receipts, from bench's priming, `seed` and `gc`.
- The main checkout's target dir.
- The Cargo home: every build is offline, so nothing is downloaded, but Cargo may unpack cached crate files into its registry source dir.
- The main checkout's worktree metadata under its git dir. `git worktree prune` on the main also drops the stale metadata of any other worktree of that main whose directory is unmounted or gone.
- A scratch directory named `scorecard` under the temp dir.

Nothing else is touched. Untracked files the measured repo needs to build, such as an `.env`, are copied into each worktree from the local env file's list. A `kill -9` skips the cleanup traps and leaves those copied `.env` files under the scratch directory until they are removed by hand. Every removal is guarded: `bench_row` accepts only the rows R1 to R5, and `safe_rm` refuses any path whose resolved form is not strictly under `$TMPDIR/scorecard/`.

The Rerun blocks are zsh on macOS. Linux is out of scope.

## Rerun

### 0. Preflight

Run every Rerun block from the repo root. Source this block, then run `preflight`. It stops before any measurement, changes no cell, and names the step to fix. A dry run must be started through the dry-run harness, which refuses any state root or temp dir outside the scratch area. The traps clean up when the shell exits or is interrupted; in an interactive shell, call `cleanup_worktrees` as the last step of a run.

The local env file `.skills/verify/scorecard.env` is not committed. This is its template, placeholders only:

```text
MAIN=<absolute path of the measured repo's main checkout>
SCORECARD_N=4
P=<package name>
FILE=<path of a .rs file in P, relative to the repo root>
PROBE=<path of the file the proc macro reads without declaring it, relative to the repo root, or empty>
WORKSPACE_ENV="<VAR=value, a note of what the measured repo needs to build offline>"
export <VAR>=<value>
UNTRACKED_COPY="<relative path> <relative path>"
SCORECARD_NUMBERS_PUBLIC=
```

- `MAIN`: the measured repo's main checkout; every worktree is added from it.
- `SCORECARD_N`: samples per arm; 4 when unset.
- `P`: the package the by-hand rows and R5 build; letters, digits, `_` and `-` only.
- `FILE`: the `.rs` file in `P` that the R6 edit appends to; relative, no `..` segment.
- `PROBE`: the file the R9 probe changes, a `.sqlx/*.json` query file or else a `.env`; relative, no `..` segment; empty where the repo has none.
- `WORKSPACE_ENV`: a note of the variables the measured repo needs; no block reads it, the `export` line beside it is what takes effect (for an sqlx repo, `export SQLX_OFFLINE=true`).
- `UNTRACKED_COPY`: space-separated relative paths without spaces or `..` segments, such as `.env`, copied from the main into each worktree.
- `SCORECARD_NUMBERS_PUBLIC`: the operator's switch to let `record` write the measured values into this file. Unset by default; the operator sets it to `yes` only after agreeing that the measured repo's numbers may be committed.

`preflight` rejects a `P` that is not letters, digits, `_` and `-`, and a `FILE`, `PROBE` or `UNTRACKED_COPY` entry that is absolute or has a `..` segment.

The evidence directory is named for the date preflight runs on. A run that crosses midnight would start a second directory in a new shell. To keep one directory, finish before midnight, or set `RUN` to the earlier directory (`RUN=.skills/verify/<date>-scorecard`) before `preflight`: preflight keeps a `RUN` already set in the shell, so unset it for a new run. A second preflight overwrites `load-start.txt`, and `summarize` writes the date it runs on. When resuming, run only the pipelines still missing: the pipelines block moves an existing `pipe.jsonl` aside.

```zsh
source .skills/verify/scorecard.env
WTS=()

cleanup_worktrees() {
  local w
  for w in $WTS; do
    git -C $MAIN worktree remove --force $w 2>/dev/null || true
  done
  git -C $MAIN worktree prune || true
}
trap cleanup_worktrees EXIT
trap 'cleanup_worktrees; return 130' INT

rel_ok() {
  [[ $1 != /* && $1 != .. && $1 != ../* && $1 != */../* && $1 != */.. ]]
}

safe_rm() {
  local root=${${TMPDIR:A}%/}/scorecard
  [[ -n $TMPDIR && -n $1 && ${1:A} == $root/?* ]] || { print -u2 "refusing to remove $1: not strictly under $root/"; return 1 }
  rm -rf -- $1
}

preflight() {
  local flag v src vars help hv hbin prof
  : ${SCORECARD_N:=4}
  [[ -n $MAIN ]] || { print -u2 'MAIN is empty: set it in the env file'; return 1 }
  [[ -n $TMPDIR ]] || { print -u2 'TMPDIR is empty'; return 1 }
  : ${RUN:=.skills/verify/$(date +%F)-scorecard}
  mkdir -p $RUN
  rm -f $RUN/preflight-fail.txt
  [[ -z $P || $P =~ '^[A-Za-z0-9_-]+$' ]] || { print "preflight stopped: P must be letters, digits, _ and -" | tee $RUN/preflight-fail.txt >&2; return 1 }
  rel_ok "$FILE" && rel_ok "$PROBE" || { print "preflight stopped: FILE and PROBE must be relative paths without a .. segment" | tee $RUN/preflight-fail.txt >&2; return 1 }
  for v in ${=UNTRACKED_COPY}; do
    rel_ok $v || { print "preflight stopped: an UNTRACKED_COPY entry must be a relative path without a .. segment: $v" | tee $RUN/preflight-fail.txt >&2; return 1 }
  done
  sysctl -n vm.loadavg > $RUN/load-start.txt || { print "preflight stopped: could not read the load average" | tee $RUN/preflight-fail.txt >&2; return 1 }
  print "Quiet machine required: no other builds or heavy work running."

  local rustc_bin=$(cd $MAIN && env -u RUSTUP_TOOLCHAIN -u RUSTUP_TOOLCHAIN_SOURCE rustup which rustc)
  TC=
  [[ $rustc_bin == */toolchains/*/bin/rustc ]] && TC=${${rustc_bin:h:h}:t}
  hbin=$(command -v cargo-hive) || { print "preflight stopped: no cargo-hive on PATH" | tee $RUN/preflight-fail.txt >&2; return 1 }
  prof=unknown
  [[ $hbin == */target/debug/* ]] && prof=debug
  [[ $hbin == */target/release/* ]] && prof=release
  { print -r -- $hbin; cargo hive --version && print "build profile: $prof (from the binary's path)"; } > $RUN/hive.txt || { print "preflight stopped: cargo hive --version failed" | tee $RUN/preflight-fail.txt >&2; return 1 }

  local doctor_rc=0
  if [[ -n $TC ]]; then
    RUSTUP_TOOLCHAIN=$TC cargo hive doctor --json > $RUN/doctor.json || doctor_rc=$?
  else
    cargo hive doctor --json > $RUN/doctor.json || doctor_rc=$?
  fi
  if (( doctor_rc != 0 )) || ! jq -e '.toolchain != null' $RUN/doctor.json > /dev/null 2>&1; then
    { print "doctor failed (exit $doctor_rc) or the toolchain is unsupported:"
      jq -r '(.checks[]? | select(.verdict != "pass") | "check \(.id): \(.verdict)"), (.unmeasured[]? | "unmeasured: \(tojson)")' $RUN/doctor.json 2>&1
    } | tee $RUN/preflight-fail.txt >&2
    return 1
  fi

  if ! cargo hive repo list --json | jq -e --arg m ${MAIN:A} '.scope.repos | index($m)' > /dev/null; then
    cargo hive repo add "$MAIN" || { print "preflight stopped: could not register the main checkout" | tee $RUN/preflight-fail.txt >&2; return 1; }
  fi

  help=$(cargo hive bench --help)
  for flag in --cold --seeded --build-cmd --json; do
    if ! print -r -- $help | grep -q -e "$flag"; then
      print "preflight stopped: bench has no $flag; update the flag list in the preflight step and the bench row step in this commit" | tee $RUN/preflight-fail.txt >&2
      return 1
    fi
  done

  if ! (cd $MAIN && rustup component list --installed) | grep -q '^clippy'; then
    print "preflight stopped: the clippy component is not installed" | tee $RUN/preflight-fail.txt >&2
    return 1
  fi

  local cfg=${HIVE_HOME:-~/.hive}/config.json
  if [[ -f $cfg ]] && jq -e 'has("secondary_roots")' $cfg > /dev/null 2>&1; then
    if ! jq -e --arg t ${TMPDIR:A} '.secondary_roots | map(rtrimstr("/")) | any(. as $r | $t == $r or ($t | startswith($r + "/")))' $cfg > /dev/null; then
      print "preflight stopped: secondary_roots in $cfg does not cover $TMPDIR" | tee $RUN/preflight-fail.txt >&2
      return 1
    fi
  fi

  if [[ -n $(git -C $MAIN status --porcelain --untracked-files=no) ]]; then
    print "preflight stopped: the main's tracked tree is not clean" | tee $RUN/preflight-fail.txt >&2
    return 1
  fi

  hv=$(cargo hive --version | awk '{print $NF}')
  src=$(git rev-parse --show-toplevel 2>/dev/null)/crates/cargo/src/env.rs
  [[ -f $src ]] || src=(${CARGO_HOME:-~/.cargo}/registry/src/*/cargo-hive-cargo-$hv/src/env.rs(N[1]))
  if [[ ! -f $src ]]; then
    print "preflight stopped: no env.rs found for hive $hv to check the scrub list against" | tee $RUN/preflight-fail.txt >&2
    return 1
  fi
  vars=(CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR CARGO_BUILD_BUILD_DIR CARGO_MANIFEST_DIR OUT_DIR CARGO_BUILD_RUSTC CARGO_BUILD_RUSTC_WRAPPER CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER RUSTUP_TOOLCHAIN RUSTUP_TOOLCHAIN_SOURCE GIT_ CARGO_PKG_)
  for v in $vars; do
    if ! grep -q -F "\"$v" $src; then
      print "preflight stopped: hive's env.rs does not drop $v; update hive_env_cargo in this commit" | tee $RUN/preflight-fail.txt >&2
      return 1
    fi
  done
  print "preflight passed: toolchain ${TC:-default}, evidence under $RUN"
}
```

### 1. Bench rows

Run `preflight` first, in the same shell. `bench_row <row> <shape>` (`<row>` is one of R1 to R5) measures one first-command row: R1 `build`, R2 `check-all`, R3 `clippy-all`, R4 `test-no-run`, R5 `check-lib=$P`, in that order. `P` comes from the env file and stays the same from run to run; it must be a package name made of letters, digits, `_` and `-`. Each row builds the main checkout once with plain `cargo`, so the store holds that shape's units before bench seeds from it. The seeded value is the seed plus the build, and the seed and build times sit beside it. A row that cannot measure, whether the main build, a worktree, the copy of an untracked file or the bench run fails, keeps its previous values: copy the refusal or failure finding from `$RUN/<row>.refusal.json` into the run's evidence and change no cell. `bench_row` then prints `<row> kept previous values` and returns non-zero, so call it as `bench_row R2 check-all || true`: a non-zero status means this row did not measure, and the later rows still run. `UNTRACKED_COPY` is a space-separated list of paths without spaces in them.

```zsh
bench_refuse() {
  jq -n --arg r $2 '{findings: [{kind: "refusal", detail: $r}]}' > $RUN/$1.refusal.json
  cleanup_worktrees
  WTS=()
  print "$1 kept previous values"
}

bench_row() {
  local row=$1 shape=$2 i w f rc ret
  local -a cargs cold seeded
  : ${SCORECARD_N:=4}
  [[ $row =~ '^R[1-5]$' ]] || { print -u2 "unknown row '$row': expected R1 to R5"; return 1 }
  [[ -n $RUN && -n $TMPDIR && -n $MAIN ]] || { print -u2 'run preflight first'; return 1 }
  case $shape in
    build) cargs=(build) ;;
    check-all) cargs=(check --all-targets) ;;
    clippy-all) cargs=(clippy --all-targets) ;;
    test-no-run) cargs=(test --no-run) ;;
    check-lib=*) cargs=(check -p ${shape#check-lib=} --lib) ;;
    *) print -u2 "unknown shape $shape"; return 1 ;;
  esac

  rm -f $RUN/$row.refusal.json $RUN/$row.new.json
  (cd $MAIN && env -u RUSTC_WRAPPER -u RUSTC_WORKSPACE_WRAPPER cargo $cargs --offline --locked) || { bench_refuse $row 'the main build failed'; return 1 }
  if [[ $row == R1 ]]; then
    test -f "$(cargo metadata --format-version 1 --no-deps --manifest-path $MAIN/Cargo.toml | jq -r .target_directory)/.rustc_info.json" || { bench_refuse $row 'the main target dir has no .rustc_info.json'; return 1 }
  fi

  safe_rm $TMPDIR/scorecard/$row || { bench_refuse $row 'could not clear the scratch directory'; return 1 }
  git -C $MAIN worktree prune
  mkdir -p $TMPDIR/scorecard/$row
  for i in {1..$SCORECARD_N}; do
    for w in $TMPDIR/scorecard/$row/cold$i $TMPDIR/scorecard/$row/seeded$i; do
      git -C $MAIN worktree add --detach $w > /dev/null 2>&1 || { bench_refuse $row "could not add a worktree at $w"; return 1 }
      WTS+=($w)
      for f in ${=UNTRACKED_COPY}; do
        mkdir -p $w/${f:h} && cp -p $MAIN/$f $w/$f || { bench_refuse $row "could not copy $f into a worktree"; return 1 }
      done
    done
    cold+=($TMPDIR/scorecard/$row/cold$i)
    seeded+=($TMPDIR/scorecard/$row/seeded$i)
  done

  rc=0
  cargo hive bench --cold $cold --seeded $seeded --build-cmd $shape --json > $RUN/$row.new.json || rc=$?
  if (( rc == 0 )) && jq -e '[.counts.cold_build_ms_median, .counts.seeded_total_ms_median, .counts.seeded_seed_ms_median, .counts.seeded_build_ms_median] | all(type == "number")' $RUN/$row.new.json > /dev/null 2>&1; then
    mv $RUN/$row.new.json $RUN/$row.json
    jq -r --arg r $row '"\($r) cold=\(.counts.cold_build_ms_median) seeded=\(.counts.seeded_total_ms_median) seed=\(.counts.seeded_seed_ms_median) build=\(.counts.seeded_build_ms_median) toolchain=\(.toolchain)"' $RUN/$row.json
    ret=0
  else
    mv $RUN/$row.new.json $RUN/$row.refusal.json
    print "$row kept previous values"
    ret=1
  fi

  for w in $cold $seeded; do
    git -C $MAIN worktree remove --force $w 2> /dev/null
    WTS=(${WTS:#$w})
  done
  git -C $MAIN worktree prune
  return $ret
}
```

Run the rows in that order, after `preflight`: `bench_row R1 build`, `bench_row R2 check-all`, `bench_row R3 clippy-all`, `bench_row R4 test-no-run`, `bench_row R5 check-lib=$P`. R2 to R5 are measured after earlier shapes have filled the store and the main target dir. Copy each printed cell into its row of the Values table only when the row printed its values, and fill that row's Provenance from the run's evidence.

### 2. By-hand pipeline

Run `preflight` first, in the same shell, with `P` and `FILE` set in the env file: `P` is the workspace crate whose proc macro reads a file without declaring it, `FILE` is a `.rs` file in `P`, and `PROBE` is the file that macro reads without declaring it, a `.sqlx/*.json` query file or else a `.env`; both relative to the repo root. Leave `PROBE` empty where the repo has no such file. Build the main with `bench_row R1 build` before the first pipeline, so the seed has `build` units to clone. `pipeline <arm> <i>` measures one fresh worktree of its own, never one a bench run used. Per worktree it does, in order: add the worktree and resolve its toolchain; read free space; for the seeded arm only, seed it; run the first build; read free space again; apply the fixed edit; time the next build; run `cargo hive gc --checkout` on the worktree and rerun the build with `-v`; rebuild with `-v` once more as a control, change the probe file, rebuild with `-v` and restore both files; remove the worktree. R6 is that timed build; R8 is the rustc invocations of the rerun; R9 is whether the rebuild after the probe change left `P` untouched; R7 is the drop in free space across the first build, which for the seeded arm includes the seed.

`gc --checkout` checks the toolchain it is started under, not the checkout's own, so the pipeline starts it with `RUSTUP_TOOLCHAIN=$TC`, the release `resolve_toolchain` found for the worktree (left unset when `TC` is empty); `seed` and `bench` resolve the checkout's toolchain themselves.

Every by-hand `cargo` runs through `hive_env_cargo`: the toolchain's own `cargo` binary under the environment hive builds its children with. The main is never built that way; an empty `RUSTC_WRAPPER` stops Cargo writing `.rustc_info.json`, and the seed needs that file.

A void sample is still written to `$RUN/pipe.jsonl`, with its rows and reason in `void`, and the function returns non-zero, so call it as `pipeline cold 1 || true`. A seed that exits non-zero, clones no units, or reports a `source.none`, `toolchain.missing` or `store.tag` finding voids R6 to R9, because the worktree was not seeded. A failed first build voids R6 to R9. A failed R6 build voids R6 and every row after it; R7, read before it, stands. A gc or a rerun that exits non-zero, or a gc receipt without both deletion totals, voids R8 and R9; R6 and R7 stand. The line then carries `null` for the four R8 keys. A control rebuild that exits non-zero or compiles a unit, a probe change that does not apply, a probe rebuild that exits non-zero without recompiling `P`, or a file that does not restore voids R9 alone; R6 to R8 stand. The line's `r9` is `stale`, `fresh-recompiled`, `probe input absent` when `PROBE` is empty, or `null` when void. A seeded line carries `r7_clone_bytes_not_shared`, the seed's registry part at seed time, or `r7_unmeasured` with the receipt's reason; it never stands in for `r7_bytes`.

```zsh
zmodload zsh/datetime zsh/mathfunc

resolve_toolchain() {
  local wt=$1
  RUSTC_BIN=$(cd $wt && env -u RUSTUP_TOOLCHAIN -u RUSTUP_TOOLCHAIN_SOURCE rustup which rustc) || return 1
  CARGO_BIN=$(cd $wt && env -u RUSTUP_TOOLCHAIN -u RUSTUP_TOOLCHAIN_SOURCE rustup which cargo) || return 1
  TC=
  if [[ $RUSTC_BIN == */toolchains/*/bin/rustc ]]; then
    TC=${${RUSTC_BIN:h:h}:t}
  fi
}

hive_env_args() {
  local v
  local -a drop keep args
  drop=(CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR CARGO_BUILD_BUILD_DIR CARGO_MANIFEST_DIR OUT_DIR CARGO_BUILD_RUSTC CARGO_BUILD_RUSTC_WRAPPER CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER RUSTUP_TOOLCHAIN RUSTUP_TOOLCHAIN_SOURCE)
  for v in ${(M)${(k)parameters[(R)*export*]}:#(GIT_*|CARGO_PKG_*)}; do
    drop+=($v)
  done
  for v in $drop; do
    args+=(-u $v)
  done
  keep=(RUSTC_WRAPPER= RUSTC_WORKSPACE_WRAPPER= RUSTUP_AUTO_INSTALL=0 RUSTC=$RUSTC_BIN)
  if [[ -n $TC ]]; then
    keep+=(RUSTUP_TOOLCHAIN=$TC)
  fi
  HIVE_ENV_ARGS=($args $keep)
}

hive_env_cargo() {
  hive_env_args
  env $HIVE_ENV_ARGS $CARGO_BIN "$@"
}

r6_edit() {
  printf '\n/// Scorecard edit.\n#[allow(dead_code, unreachable_pub)]\npub const SCORECARD_EDIT: u32 = 1;\n' >> "$FILE"
}

r9_classify() {
  local code=$1 log=$2
  if grep -qE "Running \`.*--crate-name ${P//-/_} " $log 2> /dev/null; then
    print fresh-recompiled
  elif [[ $code == 0 ]] && grep -qF "Fresh $P v" $log 2> /dev/null; then
    print stale
  else
    print void
  fi
}

r9_change() {
  local tmp rc=0
  mkdir -p $TMPDIR/scorecard || return 1
  tmp=$(mktemp $TMPDIR/scorecard/r9.XXXXXX) || return 1
  {
    if [[ $PROBE == *.json ]]; then
      jq 'if (.describe.nullable[0] | type) == "boolean" then .describe.nullable[0] |= not else error("no nullable entry") end' "$PROBE" > $tmp || rc=1
    else
      awk -v q="'" '!d && /^[A-Za-z_][A-Za-z0-9_]*=/ {c = substr($0, length($0), 1); if (c == "\"" || c == q) {print substr($0, 1, length($0) - 1) "x" c} else {print $0 "x"}; d=1; next} {print} END {exit !d}' "$PROBE" > $tmp || rc=1
    fi
    (( rc == 0 )) && { cat $tmp > "$PROBE" || rc=1 }
  } always {
    rm -f $tmp
  }
  return $rc
}

r9_restore() {
  local f rc=0
  for f in "$@"; do
    if git ls-files --error-unmatch -- "$f" > /dev/null 2>&1; then
      git checkout -- "$f" || rc=1
    else
      cp -pf "$MAIN/$f" "$f" || rc=1
    fi
  done
  return $rc
}

pipeline_abort() {
  cleanup_worktrees
  WTS=()
  git -C $MAIN worktree prune
  print -u2 "pipeline $1 $2 stopped: $3"
}

pipeline() {
  local arm=$1 i=$2 wt=$TMPDIR/scorecard/pipe/$1$2 run=${RUN:A} f a b rc ms
  local r7=null clone=null unm=null r6=null void=null r8u=null r8f=null r8o=null r8v=null r9=null probe_void cls st
  local -a rows gc_env
  [[ $arm == (cold|seeded) && $i == <-> ]] || { print -u2 'usage: pipeline cold|seeded <i>'; return 1 }
  [[ -n $RUN && -n $TMPDIR && -n $MAIN && -n $P && -n $FILE ]] || { print -u2 'run preflight first, with P and FILE set'; return 1 }

  safe_rm $wt || { pipeline_abort $arm $i "could not clear $wt"; return 1 }
  git -C $MAIN worktree prune
  mkdir -p ${wt:h}
  git -C $MAIN worktree add --detach $wt > /dev/null 2>&1 || { pipeline_abort $arm $i "could not add a worktree at $wt"; return 1 }
  WTS+=($wt)
  for f in ${=UNTRACKED_COPY}; do
    mkdir -p $wt/${f:h} && cp -p $MAIN/$f $wt/$f || { pipeline_abort $arm $i "could not copy $f into the worktree"; return 1 }
  done
  resolve_toolchain $wt || { pipeline_abort $arm $i 'could not resolve the toolchain'; return 1 }

  sync
  a=$(/bin/df -k $wt | awk 'NR==2{print $4}')

  if [[ $arm == seeded ]]; then
    rc=0
    cargo hive seed $wt --json > $RUN/pipe-$arm$i-seed.json || rc=$?
    clone=$(jq -c '.counts.clone_bytes_not_shared // null' $RUN/pipe-$arm$i-seed.json 2> /dev/null) || clone=null
    [[ $clone == <-> ]] || clone=null
    if [[ $clone == null ]]; then
      unm=$(jq -c '[.unmeasured[]? | select(tojson | contains("clone_bytes_not_shared")) | (.reason // tojson)] | first // "no reason in the receipt"' $RUN/pipe-$arm$i-seed.json 2> /dev/null) || unm=
      [[ -n $unm ]] || unm='"no readable receipt"'
    fi
    if (( rc != 0 )); then
      void="{\"reason\":\"the seed exited $rc\"}"
    elif ! jq -e 'type == "object"' $RUN/pipe-$arm$i-seed.json > /dev/null 2>&1; then
      void='{"reason":"the seed receipt is unreadable"}'
    elif jq -e '(.counts.clone_units_cloned // 0) == 0' $RUN/pipe-$arm$i-seed.json > /dev/null 2>&1; then
      void='{"reason":"the seed cloned no units"}'
    elif jq -e 'any(.findings[]?; (.kind // "") | test("source\\.none|toolchain\\.missing|store\\.tag"))' $RUN/pipe-$arm$i-seed.json > /dev/null 2>&1; then
      void='{"reason":"the seed reported a source.none, toolchain.missing or store.tag finding"}'
    fi
    if [[ $void != null ]]; then
      void=$(jq -c '. + {rows: ["R6","R7","R8","R9"]}' <<< $void)
    fi
  fi

  if [[ $void == null ]]; then
    rc=0
    (cd $wt && hive_env_cargo build --offline --locked) > $RUN/pipe-$arm$i-first.log 2>&1 || rc=$?
    if (( rc != 0 )); then
      void='{"rows":["R6","R7","R8","R9"],"reason":"the first build failed"}'
    else
      sync
      b=$(/bin/df -k $wt | awk 'NR==2{print $4}')
      [[ $a == <-> && $b == <-> ]] || { pipeline_abort $arm $i 'could not read free space'; return 1 }
      r7=$(( (a - b) * 1024 ))
    fi
  fi

  if [[ $void == null ]]; then
    (cd $wt && r6_edit) || { pipeline_abort $arm $i "could not edit $FILE"; return 1 }
    rm -f $run/pipe-$arm$i-r6.txt
    (
      cd $wt
      hive_env_args
      t0=$EPOCHREALTIME
      rc=0
      env $HIVE_ENV_ARGS $CARGO_BIN build --offline --locked > $run/pipe-$arm$i-r6.log 2>&1 || rc=$?
      t1=$EPOCHREALTIME
      print -r -- $rc $(( int(rint((t1 - t0) * 1000)) )) > $run/pipe-$arm$i-r6.txt
    ) || true
    read rc ms < $run/pipe-$arm$i-r6.txt || { pipeline_abort $arm $i 'the R6 timing was not recorded'; return 1 }
    [[ $rc == <-> && $ms == <-> ]] || { pipeline_abort $arm $i 'the R6 timing was not readable'; return 1 }
    if (( rc != 0 )); then
      void='{"rows":["R6","R8","R9"],"reason":"the R6 build failed"}'
    else
      r6=$ms
    fi
  fi

  if [[ $void == null ]]; then
    rc=0
    gc_env=()
    if [[ -n $TC ]]; then gc_env=(RUSTUP_TOOLCHAIN=$TC); fi
    env $gc_env cargo hive gc --checkout $wt --json > $RUN/pipe-$arm$i-gc.json || rc=$?
    r8f=$(jq -c '.counts.unit_files_deleted // null' $RUN/pipe-$arm$i-gc.json 2> /dev/null) || r8f=null
    r8o=$(jq -c '.counts.objects_deleted // null' $RUN/pipe-$arm$i-gc.json 2> /dev/null) || r8o=null
    [[ $r8f == <-> ]] || r8f=null
    [[ $r8o == <-> ]] || r8o=null
    if (( rc != 0 )); then
      void="{\"rows\":[\"R8\",\"R9\"],\"reason\":\"the gc exited $rc\"}"
    elif [[ $r8f == null || $r8o == null ]]; then
      void='{"rows":["R8","R9"],"reason":"the gc receipt has no deletion totals"}'
    else
      rc=0
      (cd $wt && hive_env_cargo build --offline --locked -v) > /dev/null 2> $RUN/pipe-$arm$i-r8.log || rc=$?
      if (( rc != 0 )); then
        void="{\"rows\":[\"R8\",\"R9\"],\"reason\":\"the rebuild after the gc exited $rc\"}"
      else
        r8u=$(grep -c 'Running `.*--crate-name' $RUN/pipe-$arm$i-r8.log) || true
        [[ $r8u == <-> ]] || r8u=null
        if [[ $r8u == null ]]; then
          void='{"rows":["R8","R9"],"reason":"the rebuild log is unreadable"}'
        elif (( r8f == 0 && r8o == 0 )); then
          r8v=true
        else
          r8v=false
        fi
      fi
    fi
  fi
  if [[ $void != null ]] && jq -e '(.rows // []) | index("R8")' <<< $void > /dev/null; then
    r8u=null r8f=null r8o=null r8v=null
  fi

  if [[ $void == null ]]; then
    probe_void=
    if [[ -z $PROBE ]]; then
      r9='"probe input absent"'
      (cd $wt && git checkout -- "$FILE") || probe_void='the edit to the crate file was not restored'
    else
      rc=0
      [[ -f $wt/$PROBE ]] || rc=-1
      (( rc != 0 )) || (cd $wt && hive_env_cargo build --offline --locked -v) > /dev/null 2> $RUN/pipe-$arm$i-r9-control.log || rc=$?
      if (( rc == -1 )); then
        probe_void='probe input missing'
      elif (( rc != 0 )); then
        probe_void="the control rebuild exited $rc"
      elif grep -q 'Running `.*--crate-name' $RUN/pipe-$arm$i-r9-control.log; then
        probe_void='the control rebuild compiled a unit'
      elif ! (cd $wt && r9_change); then
        probe_void="the probe change did not apply to $PROBE"
      else
        rc=0
        (cd $wt && hive_env_cargo build --offline --locked -v) > /dev/null 2> $RUN/pipe-$arm$i-r9.log || rc=$?
        cls=$(r9_classify $rc $RUN/pipe-$arm$i-r9.log)
        r9=\"$cls\"
        [[ $cls != void ]] || probe_void="the probe rebuild exited $rc without recompiling $P"
      fi
      (cd $wt && r9_restore "$PROBE" "$FILE") || probe_void=${probe_void:-'the probe file was not restored'}
    fi
    if ! st=$(git -C $wt status --porcelain --untracked-files=no) || [[ -n $st ]]; then
      probe_void=${probe_void:-'the worktree was not clean after the probe'}
    elif [[ -n $PROBE ]] && ! git -C $wt ls-files --error-unmatch -- "$PROBE" > /dev/null 2>&1 && ! cmp -s "$MAIN/$PROBE" "$wt/$PROBE"; then
      probe_void=${probe_void:-'the probe file was not restored'}
    fi
    if [[ -n $probe_void ]]; then
      void=$(jq -nc --arg r "$probe_void" '{rows: ["R9"], reason: $r}')
      r9=null
    fi
  fi

  jq -nc --arg arm $arm --argjson i $i --argjson r7 "${r7:-null}" --argjson clone "${clone:-null}" --argjson unm "${unm:-null}" --argjson r6 "${r6:-null}" --argjson r8u "${r8u:-null}" --argjson r8f "${r8f:-null}" --argjson r8o "${r8o:-null}" --argjson r8v "${r8v:-null}" --argjson r9 "${r9:-null}" --argjson void "${void:-null}" '
    {arm: $arm, i: $i, r7_bytes: (if ($void.rows // []) | index("R7") then null else $r7 end), r6_ms: $r6, r8_units: $r8u, r8_unit_files_deleted: $r8f, r8_objects_deleted: $r8o, r8_vacuous: $r8v, r9: $r9, void: $void}
    + (if $arm == "seeded" then (if $clone != null then {r7_clone_bytes_not_shared: $clone} else {r7_unmeasured: $unm} end) else {} end)' >> $RUN/pipe.jsonl

  git -C $MAIN worktree remove --force $wt 2> /dev/null || true
  WTS=(${WTS:#$wt})
  git -C $MAIN worktree prune
  [[ $void == null ]]
}
```

Run the pipelines with the block below. For a `SCORECARD_N` of 4 the order is cold 1, seeded 1, seeded 2, cold 2, cold 3, seeded 3, seeded 4, cold 4: an odd `i` runs cold then seeded, an even `i` runs seeded then cold, so for any `SCORECARD_N` the arms swap places each round and drift falls on both. The block starts a fresh `pipe.jsonl` and keeps the old one under a timestamped name, so earlier samples and their void reasons stay in the evidence:

```text
if [[ -f $RUN/pipe.jsonl ]]; then mv $RUN/pipe.jsonl $RUN/pipe.$(date +%H%M%S).jsonl; fi
for i in {1..$SCORECARD_N}; do
  if (( i % 2 )); then arms=(cold seeded); else arms=(seeded cold); fi
  for a in $arms; do pipeline $a $i || true; done
done
```

### 3. Summarize

Run `summarize` in the same shell, after `preflight`, the bench rows and the pipelines. It reads `$RUN/R1.json` to `$RUN/R5.json` and `$RUN/pipe.jsonl`, prints one line per row ID, then the Provenance line, then min, max and spread per arm for each measured row, and writes the same text to `$RUN/summary.txt`. It also writes the cells the Record step copies to `$RUN/cells.tsv`. Call it as `summarize || true` in a script that has more to do.

A line reads `<row> cold n=<k> median=<v> | seeded n=<k> median=<v>`. For R8 and R9 the median field holds the cell text: R8 as `R rebuilt over k non-vacuous samples; m vacuous` or `all vacuous`, R9 as `s stale of n`. When either arm has fewer than 3 valid samples the line reads `<row> kept previous values (<arm>: <k> valid of <n>; void: <reasons>)` and the row has no line in `$RUN/cells.tsv`, so Record leaves it as it was. A bench row that did not measure reads the same way, with the refusal from `$RUN/<row>.refusal.json` as its reason. R10 is a listed cell and is not measured.

Rules `summarize` applies:

- Before reading the evidence it writes the end load average to `$RUN/load-end.txt`, and prints a one-line warning to stderr, not a failure and not part of `summary.txt`, when the 1-minute load average in `load-start.txt` is above half the logical core count (`sysctl -n hw.ncpu`, divided by 2) or `load-start.txt` is missing.
- A pipeline sample counts for a row unless its `void` names that row. A sample with a `probe input absent` result is not a valid R9 sample.
- The median of the by-hand rows follows bench's rule: sort the values, take the middle one for odd n, and take `floor((a+b)/2)` of the two middle ones for even n. The bench rows take bench's own median from the receipt, which uses the same rule.
- R1 to R5: the cold arm reads `cold_build_ms_median` and the seeded arm reads `seeded_total_ms_median`; n is the number of checkouts of that arm in the receipt.
- The Provenance line reads `measured=<date> commit=<sha> toolchain=<release> machine=<model, chip, cores> evidence=<RUN>`. The toolchain is the receipt's top-level `toolchain`. The commit is the commit of the tag `v<hive_version>` for a hive installed from the registry, or the source checkout's `HEAD` when the `cargo-hive` in use was built from it. The machine is `sysctl -n hw.model machdep.cpu.brand_string hw.ncpu`.

```zsh
median() {
  local -a v
  local n s
  v=(${(on)@})
  n=$#v
  (( n > 0 )) || return 1
  if (( n % 2 )); then
    print -r -- $v[$(( (n + 1) / 2 ))]
  else
    s=$(( v[n / 2] + v[n / 2 + 1] ))
    if (( s >= 0 )); then
      print -r -- $(( s / 2 ))
    else
      print -r -- $(( -((-s + 1) / 2) ))
    fi
  fi
}

pipe_sel() {
  jq -s -r --arg a $1 --arg x $2 "map(select(.arm == \$a and (.void == null or ((.void.rows // []) | index(\$x) | not)) and ($3))) | .[] | ($4)" $RUN/pipe.jsonl
}

pipe_reasons() {
  jq -s -r --arg a $1 --arg x $2 '[.[] | select(.arm == $a) | if (.void != null and ((.void.rows // []) | index($x))) then .void.reason elif ($x == "R9" and .r9 == "probe input absent") then "probe input absent" else empty end] | unique | join(", ")' $RUN/pipe.jsonl
}

sfail() {
  print -u2 'summarize: a command failed'
  exit 1
}

summarize() {
  local row arm m key f rcpt top date hv tc commit machine rs line l1 ncpu
  local -a vals lines units short ev rr
  local -A N K CN CM LO HI
  local rc=0
  setopt localoptions pipefail
  [[ -n $RUN && -n $MAIN ]] || { print -u2 'run preflight first'; return 1 }
  : ${SCORECARD_N:=4}
  ncpu=$(sysctl -n hw.ncpu) || return 1
  sysctl -n vm.loadavg > $RUN/load-end.txt || return 1
  l1=$(awk '{print $2}' $RUN/load-start.txt 2> /dev/null) || l1=
  if [[ $l1 != <->(.<->|) ]]; then
    print -u2 'warning: load-start.txt is missing or unreadable; the machine may not have been quiet'
  elif (( l1 > ncpu / 2.0 )); then
    print -u2 "warning: the 1-minute load average at the start was $l1, above half of the $ncpu logical cores; the machine was not quiet"
  fi
  [[ -f $RUN/pipe.jsonl ]] || : > $RUN/pipe.jsonl
  rm -f $RUN/cells.tsv

  date=$(date +%F)
  hv=
  tc=
  rcpt=($RUN/R[1-5].json(N[1]))
  if (( $#rcpt )); then
    hv=$(jq -r '.hive_version // empty' $rcpt[1]) || return 1
    tc=$(jq -r '.toolchain // empty' $rcpt[1]) || return 1
  fi
  [[ -n $hv ]] || hv=$(cargo hive --version | awk '{print $NF}')
  [[ -n $tc ]] || tc=$(jq -r '.toolchain // empty' $RUN/doctor.json 2> /dev/null) || tc=
  top=$(git rev-parse --show-toplevel)
  commit=
  if [[ $(head -1 $RUN/hive.txt 2> /dev/null) == $top/* ]]; then
    commit=$(git rev-parse HEAD)
  else
    commit=$(git rev-list -n1 v$hv 2> /dev/null) || commit=
  fi
  commit=${${commit:-unknown}[1,12]}
  machine=$(sysctl -n hw.model machdep.cpu.brand_string hw.ncpu) || return 1
  machine=${(j:, :)${(f)machine}}
  : > $RUN/cells.tsv

  {
    setopt errexit
    for row in R1 R2 R3 R4 R5 R6 R7 R8 R9; do
      N=() K=() CN=() CM=() LO=() HI=()
      rs=
      case $row in
        R1|R2|R3|R4|R5)
          f=$RUN/$row.json
          if [[ -f $RUN/$row.refusal.json || ! -f $f ]]; then
            rs=$(jq -r '[.findings[]? | .detail] | join(", ")' $RUN/$row.refusal.json 2> /dev/null) || rs=
            rs=${rs:-no receipt}
            for arm in cold seeded; do N[$arm]=$SCORECARD_N K[$arm]=0; done
          else
            for arm in cold seeded; do
              if [[ $arm == cold ]]; then m=cold_build_ms; else m=seeded_total_ms; fi
              N[$arm]=$(jq --arg a $arm '[.scope.checkouts[] | select((split("/") | last) | startswith($a))] | length' $f) || sfail
              CM[$arm]=$(jq -r --arg k ${m}_median '.counts[$k] | if type == "number" then . else empty end' $f) || sfail
              K[$arm]=0
              if [[ -n $CM[$arm] ]]; then K[$arm]=$N[$arm]; fi
              LO[$arm]=$(jq -r --arg k ${m}_min '.counts[$k] // empty' $f) || sfail
              HI[$arm]=$(jq -r --arg k ${m}_max '.counts[$k] // empty' $f) || sfail
            done
          fi
          ;;
        R6|R7)
          if [[ $row == R6 ]]; then key=r6_ms; else key=r7_bytes; fi
          for arm in cold seeded; do
            N[$arm]=$(jq -s --arg a $arm 'map(select(.arm == $a)) | length' $RUN/pipe.jsonl) || sfail
            vals=(${(f)"$(pipe_sel $arm $row ".$key != null" ".$key")"}) || sfail
            vals=(${vals:#})
            K[$arm]=$#vals
            if (( $#vals )); then
              CM[$arm]=$(median $vals) || sfail
              LO[$arm]=${${(on)vals}[1]}
              HI[$arm]=${${(on)vals}[-1]}
            fi
          done
          ;;
        R8)
          for arm in cold seeded; do
            N[$arm]=$(jq -s --arg a $arm 'map(select(.arm == $a)) | length' $RUN/pipe.jsonl) || sfail
            lines=(${(f)"$(pipe_sel $arm R8 '.r8_units != null and .r8_vacuous != null' '"\(.r8_vacuous) \(.r8_units)"')"}) || sfail
            lines=(${lines:#})
            K[$arm]=$#lines
            units=(${${(M)lines:#false *}#false })
            if (( $#units )); then
              CM[$arm]="$(median $units) rebuilt over $#units non-vacuous samples; $(( $#lines - $#units )) vacuous"
              LO[$arm]=${${(on)units}[1]}
              HI[$arm]=${${(on)units}[-1]}
            elif (( $#lines )); then
              CM[$arm]='all vacuous'
            fi
          done
          ;;
        R9)
          for arm in cold seeded; do
            N[$arm]=$(jq -s --arg a $arm 'map(select(.arm == $a)) | length' $RUN/pipe.jsonl) || sfail
            vals=(${(f)"$(pipe_sel $arm R9 '.r9 == "stale" or .r9 == "fresh-recompiled"' '.r9')"}) || sfail
            vals=(${vals:#})
            K[$arm]=$#vals
            units=(${(M)vals:#stale})
            CM[$arm]="$#units stale of $#vals"
          done
          ;;
      esac

      short=()
      for arm in cold seeded; do
        CN[$arm]=$K[$arm]
        (( K[$arm] >= 3 )) || short+=("$arm: $K[$arm] valid of $N[$arm]")
      done
      if (( $#short )); then
        if [[ $row == (R6|R7|R8|R9) ]]; then
          rr=(${(s:, :)"$(pipe_reasons cold $row)"} ${(s:, :)"$(pipe_reasons seeded $row)"}) || sfail
          rs=${(j:, :)${(u)rr}}
        fi
        print -r -- "$row kept previous values (${(j:; :)short}; void: ${rs:-none})"
      else
        print -r -- "$row cold n=$CN[cold] median=$CM[cold] | seeded n=$CN[seeded] median=$CM[seeded]"
        print -- "$row\t$CN[cold]\t$CM[cold]\t$CN[seeded]\t$CM[seeded]\t$date\t$commit\t$tc\t$machine\t$RUN" >> $RUN/cells.tsv
        if [[ -n $LO[cold] && -n $LO[seeded] ]]; then
          ev+=("evidence $row cold min=$LO[cold] max=$HI[cold] spread=$(( HI[cold] - LO[cold] )) | seeded min=$LO[seeded] max=$HI[seeded] spread=$(( HI[seeded] - LO[seeded] ))")
        fi
      fi
    done
    print 'R10 kept previous values (listed, not measured)'
    print -r -- "measured=$date commit=$commit toolchain=$tc machine=$machine evidence=$RUN"
    for line in $ev; do print -r -- $line; done
  } | tee $RUN/summary.txt || rc=$?
  if (( rc != 0 )) || [[ $(grep -cE '^R(10|[1-9]) ' $RUN/summary.txt) != 10 ]] || [[ -n $(cut -f1 $RUN/cells.tsv | grep -vxE 'R[1-9]') ]]; then
    rm -f $RUN/cells.tsv
    print -u2 'summarize failed: the summary or the cells file is incomplete; do not record it'
    return 1
  fi
}
```

### 4. Record

Run `record` in the same shell after `summarize`. It prints the cells it would write, and stops there unless `SCORECARD_NUMBERS_PUBLIC=yes` is set in the local env file. That variable is the operator's switch to let `record` write the measured values into this file; leave it unset until the operator has agreed that the measured repo may be named and its numbers committed. While it is unset, nothing is written to this file, and no commit names or measures the repo; the numbers stay in `$RUN`.

When it is set, `record` overwrites, for each row that has a line in `$RUN/cells.tsv`: the Cold n, Cold median, Seeded n and Seeded median cells of the Values table, and every cell of that row in the Provenance table, with `$RUN`'s relative path in the Evidence cell. It never adds a row, never touches R10, and never touches the Target (cold), Target (seeded) or Target set cells. A row that kept its previous values keeps every cell. Pass a file path to `record` to write to a copy instead of this file.

The Latest run paragraph under the Provenance table is not written by `record`: rewrite it by hand at each rerun. Before committing a rerun, re-check that paragraph (loads, spreads, commits, dates) against the new evidence.

Before the last block, re-check the setup footprint (the section after this one) and fix R10 in the same commit if it changed. The last block ends with `cleanup_worktrees` and a worktree list that must show only the main.

```zsh
record() {
  local file=${1:-docs/scorecard.md} tmp
  [[ -n $RUN && -f $RUN/cells.tsv ]] || { print -u2 'run summarize first'; return 1 }
  [[ -f $file ]] || { print -u2 "no such file: $file"; return 1 }
  [[ -s $RUN/cells.tsv ]] || { print 'no row measured; nothing to record'; return 0 }
  print 'cells to record:'
  cat $RUN/cells.tsv
  if [[ $SCORECARD_NUMBERS_PUBLIC != yes ]]; then
    print 'stopped after printing: nothing was written; the numbers stay in the run evidence until the operator sets SCORECARD_NUMBERS_PUBLIC=yes in the env file'
    return 0
  fi
  tmp=$(mktemp $file.XXXXXX) || return 1
  {
    cp -p $file $tmp && awk -F'\t' '
    FNR == NR { cell[$1] = $0; next }
    /^## / { sect = $0 }
    /^\| R(10|[1-9]) \|/ {
      n = split($0, c, "|")
      id = c[2]
      gsub(/^ +| +$/, "", id)
      if (id in cell) {
        split(cell[id], t, "\t")
        if (sect ~ /^## Values/) {
          c[5] = " " t[2] " "; c[6] = " " t[3] " "; c[7] = " " t[4] " "; c[8] = " " t[5] " "
        } else if (sect ~ /^## Provenance/) {
          c[3] = " " t[6] " "; c[4] = " " t[7] " "; c[5] = " " t[8] " "; c[6] = " " t[9] " "; c[7] = " " t[10] " "
        }
        out = c[1]
        for (j = 2; j <= n; j++) out = out "|" c[j]
        print out
        next
      }
    }
    { print }
  ' $RUN/cells.tsv $file > $tmp && mv $tmp $file
  } always {
    rm -f $tmp
  }
}
```

```text
summarize
record
git -C $MAIN worktree list
cleanup_worktrees
git -C $MAIN worktree list
```

### Re-check the setup footprint

Compare R10's Seeded cell with the install section of the README: each command a user runs, and the safety guarantees about what hive installs and writes. The README's source install (`cargo install --path crates/cargo-hive`) is the source form of `cargo install cargo-hive`. If a line changed, fix it in this rerun's own commit.
