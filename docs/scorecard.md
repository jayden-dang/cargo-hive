# Worktree scorecard

What an agent worktree costs, cold and seeded, on the measured workspace. A cold checkout is one hive did not seed. A seeded checkout is one that `cargo hive seed` seeded just before its first build. Every durable value below is an integer: durations in milliseconds, sizes in bytes. A cell reading `not yet measured` has no run behind it yet.

## Values

| Row | Metric | Source | Cold n | Cold median | Seeded n | Seeded median | Target (cold) | Target (seeded) | Target set |
|---|---|---|---|---|---|---|---|---|---|
| R1 | First-command time for `build` (ms) | `cold_build_ms_median` / `seeded_total_ms_median` | not yet measured | not yet measured | not yet measured | not yet measured | open — operator | open — operator |  |
| R2 | First-command time for `check --all-targets` (ms) | `cold_build_ms_median` / `seeded_total_ms_median` | not yet measured | not yet measured | not yet measured | not yet measured | open — operator | open — operator |  |
| R3 | First-command time for `clippy --all-targets` (ms) | `cold_build_ms_median` / `seeded_total_ms_median` | not yet measured | not yet measured | not yet measured | not yet measured | open — operator | open — operator |  |
| R4 | First-command time for `test --no-run` (ms) | `cold_build_ms_median` / `seeded_total_ms_median` | not yet measured | not yet measured | not yet measured | not yet measured | open — operator | open — operator |  |
| R5 | First-command time for `check -p <package> --lib` (ms) | `cold_build_ms_median` / `seeded_total_ms_median` | not yet measured | not yet measured | not yet measured | not yet measured | open — operator | open — operator |  |
| R6 | Next build after a one-file edit in a workspace crate (ms) | by hand (outside bench) | not yet measured | not yet measured | not yet measured | not yet measured | open — operator | open — operator |  |
| R7 | Private bytes each new worktree adds (bytes) | by hand (outside bench): free space | not yet measured | not yet measured | not yet measured | not yet measured | open — operator | open — operator |  |
| R8 | Units rebuilt because of a hive deletion | by hand (outside bench): cargo -v after gc --checkout | not yet measured | not yet measured | not yet measured | not yet measured | open — operator | open — operator |  |
| R9 | Stale results on a probe that changes a file a proc macro reads without declaring it | by hand (outside bench): cargo -v after probe | not yet measured | not yet measured | not yet measured | not yet measured | open — operator | open — operator |  |
| R10 | Setup footprint | listed | — | nothing | — | `cargo install cargo-hive`, `cargo hive doctor`, `cargo hive repo add <main>`; Cargo config files 0, environment variables 0, rustc wrappers 0, cargo shims 0; files outside the state root: 0, beyond clones into target dirs | open — operator | open — operator |  |

R8 cells read `R rebuilt over k non-vacuous samples; m vacuous`, or `all vacuous`. R9 cells read `s stale of n`.

## Provenance

| Row | Measured on | Hive commit | Toolchain | Machine | Evidence |
|---|---|---|---|---|---|
| R1 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |
| R2 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |
| R3 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |
| R4 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |
| R5 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |
| R6 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |
| R7 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |
| R8 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |
| R9 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |
| R10 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |

## Targets

The operator sets a target by editing that row's Target cell and its Target set date in a plain commit. A rerun never touches either.

## Notes per row

- R1 to R5: bench times one first build per fresh checkout. Rows measured later in a run see a store and a main target dir that earlier shapes already filled, so R2 to R5 are noted as measured after earlier shapes.
- R6: by hand (outside bench); environment matches bench's, but bench did not time it. The environment is the same variable scrub that hive applies to the builds it runs. Only the cargo exec is timed, after a fixed one-file edit to a workspace crate. The median follows bench's rule: the middle value for odd n, and the floor of the mean of the two middle values for even n.
- R7: the bytes the volume lost, read as the drop in free space from before a worktree's first build to after it, never bytes that clones share. The reading also carries the seed's store writes, any Cargo source unpacks, and any other writer on the same APFS container, so the quiet-machine rule and the median over four samples are the only guards. On the seeded arm the seed receipt's `clone_bytes_not_shared` is recorded beside the value as the seed's registry part at seed time; it is 0 on a fully shared clone and never stands in for the drop.
- R8: a sample is vacuous when the deletion removed no unit files and no objects, because then nothing could be rebuilt because of it. A vacuous sample is valid and counted. A sample is void when the deletion or the rebuild fails.

## What a run touches

- The real hive state root: its capabilities file (doctor), the store, the store database and receipts, from bench's priming, `seed` and `gc`.
- The main checkout's target dir.
- The Cargo home: every build is offline, so nothing is downloaded, but Cargo may unpack cached crate files into its registry source dir.
- The main checkout's worktree metadata under its git dir.
- A scratch dir named `scorecard` under the temp dir.

Nothing else is touched. Untracked files the workspace needs to build, such as an `.env`, are copied into each worktree from the local env file's list.

The Rerun blocks are zsh on macOS. Linux is out of scope.

## Rerun

### 0. Preflight

Run every Rerun block from the repo root. Source this block, then run `preflight`. It stops before any measurement, changes no cell, and names the step to fix. A dry run must be started through the dry-run harness, which refuses any state root or temp dir outside the scratch area. The traps clean up when the shell exits or is interrupted; in an interactive shell, call `cleanup_worktrees` as the last step of a run.

```zsh
source .skills/verify/scorecard.env
WTS=()

cleanup_worktrees() {
  local w
  for w in $WTS; do
    git -C $MAIN worktree remove --force $w 2>/dev/null
  done
  git -C $MAIN worktree prune
}
trap cleanup_worktrees EXIT
trap 'cleanup_worktrees; return 130' INT

preflight() {
  local flag v src vars help hv
  : ${SCORECARD_N:=4}
  [[ -n $MAIN ]] || { print -u2 'MAIN is empty: set it in the env file'; return 1 }
  [[ -n $TMPDIR ]] || { print -u2 'TMPDIR is empty'; return 1 }
  RUN=.skills/verify/$(date +%F)-scorecard
  mkdir -p $RUN
  rm -f $RUN/preflight-fail.txt
  sysctl -n vm.loadavg > $RUN/load-start.txt
  print "Quiet machine required: no other builds or heavy work running."

  local rustc_bin=$(cd $MAIN && env -u RUSTUP_TOOLCHAIN -u RUSTUP_TOOLCHAIN_SOURCE rustup which rustc)
  TC=
  [[ $rustc_bin == */toolchains/*/bin/rustc ]] && TC=${${rustc_bin:h:h}:t}
  { command -v cargo-hive; cargo hive --version; } > $RUN/hive.txt

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

Run `preflight` first, in the same shell. `bench_row <row> <shape>` measures one first-command row: R1 `build`, R2 `check-all`, R3 `clippy-all`, R4 `test-no-run`, R5 `check-lib=$P`, in that order. `P` comes from the env file and stays the same from run to run; it must be a package name made of letters, digits, `_` and `-`. Each row builds the main checkout once with plain `cargo`, so the store holds that shape's units before bench seeds from it. The seeded value is the seed plus the build, and the seed and build times sit beside it. A row that cannot measure, whether the main build, a worktree, the copy of an untracked file or the bench run fails, keeps its previous values: copy the refusal or failure finding from `$RUN/<row>.refusal.json` into the run's evidence and change no cell. `bench_row` then prints `<row> kept previous values` and returns non-zero, so call it as `bench_row R2 check-all || true`: a non-zero status means this row did not measure, and the later rows still run. `UNTRACKED_COPY` is a space-separated list of paths without spaces in them.

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

  rm -rf $TMPDIR/scorecard/$row
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

Run `preflight` first, in the same shell, with `P` and `FILE` set in the env file: `P` is the workspace crate whose proc macro reads a file without declaring it, and `FILE` is a `.rs` file in `P`, relative to the repo root. Build the main with `bench_row R1 build` before the first pipeline, so the seed has `build` units to clone. `pipeline <arm> <i>` measures one fresh worktree of its own, never one a bench run used. Per worktree it does, in order: add the worktree and resolve its toolchain; read free space; for the seeded arm only, seed it; run the first build; read free space again; apply the fixed edit; time the next build; remove the worktree. R6 is that timed build; R7 is the drop in free space across the first build, which for the seeded arm includes the seed.

Every by-hand `cargo` runs through `hive_env_cargo`: the toolchain's own `cargo` binary under the environment hive builds its children with. The main is never built that way; an empty `RUSTC_WRAPPER` stops Cargo writing `.rustc_info.json`, and the seed needs that file.

A void sample is still written to `$RUN/pipe.jsonl`, with its rows and reason in `void`, and the function returns non-zero, so call it as `pipeline cold 1 || true`. A seed that exits non-zero, clones no units, or reports a `source.none`, `toolchain.missing` or `store.tag` finding voids R6 to R9, because the worktree was not seeded. A failed first build voids R6 to R9. A failed R6 build voids R6 and every row after it; R7, read before it, stands. A seeded line carries `r7_clone_bytes_not_shared`, the seed's registry part at seed time, or `r7_unmeasured` with the receipt's reason; it never stands in for `r7_bytes`.

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

hive_env_cargo() {
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
  env $args $keep $CARGO_BIN "$@"
}

r6_edit() {
  printf '\n/// Scorecard edit.\n#[allow(dead_code, unreachable_pub)]\npub const SCORECARD_EDIT: u32 = 1;\n' >> "$FILE"
}

pipeline_abort() {
  cleanup_worktrees
  WTS=()
  git -C $MAIN worktree prune
  print -u2 "pipeline $1 $2 stopped: $3"
}

pipeline() {
  local arm=$1 i=$2 wt=$TMPDIR/scorecard/pipe/$1$2 run=${RUN:A} f a b rc ms
  local r7=null clone=null unm=null r6=null void=null
  local -a rows
  [[ $arm == (cold|seeded) && $i == <-> ]] || { print -u2 'usage: pipeline cold|seeded <i>'; return 1 }
  [[ -n $RUN && -n $TMPDIR && -n $MAIN && -n $P && -n $FILE ]] || { print -u2 'run preflight first, with P and FILE set'; return 1 }

  rm -rf $wt
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
    if [[ $clone == null ]]; then
      unm=$(jq -c '[.unmeasured[]? | select(tojson | contains("clone_bytes_not_shared")) | (.reason // tojson)] | first // "no reason in the receipt"' $RUN/pipe-$arm$i-seed.json 2> /dev/null) || unm='"no readable receipt"'
    fi
    if (( rc != 0 )); then
      void="{\"reason\":\"the seed exited $rc\"}"
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
    (cd $wt && r6_edit)
    (
      cd $wt
      t0=$EPOCHREALTIME
      rc=0
      hive_env_cargo build --offline --locked > $run/pipe-$arm$i-r6.log 2>&1 || rc=$?
      t1=$EPOCHREALTIME
      print -r -- $rc $(( int(rint((t1 - t0) * 1000)) )) > $run/pipe-$arm$i-r6.txt
    )
    read rc ms < $RUN/pipe-$arm$i-r6.txt
    if (( rc != 0 )); then
      void='{"rows":["R6","R8","R9"],"reason":"the R6 build failed"}'
    else
      r6=$ms
    fi
  fi

  jq -nc --arg arm $arm --argjson i $i --argjson r7 $r7 --argjson clone $clone --argjson unm $unm --argjson r6 $r6 --argjson void $void '
    {arm: $arm, i: $i, r7_bytes: (if ($void.rows // []) | index("R7") then null else $r7 end), r6_ms: $r6, void: $void}
    + (if $arm == "seeded" then (if $clone != null then {r7_clone_bytes_not_shared: $clone} else {r7_unmeasured: $unm} end) else {} end)' >> $RUN/pipe.jsonl

  git -C $MAIN worktree remove --force $wt 2> /dev/null || true
  WTS=(${WTS:#$wt})
  git -C $MAIN worktree prune
  [[ $void == null ]]
}
```

Run the pipelines in the order cold 1, seeded 1, seeded 2, cold 2, cold 3, seeded 3, seeded 4, cold 4, so that drift falls on both arms:

```text
for i in {1..$SCORECARD_N}; do
  if (( i % 2 )); then arms=(cold seeded); else arms=(seeded cold); fi
  for a in $arms; do pipeline $a $i || true; done
done
```

### Re-check the setup footprint

Compare R10's Seeded cell with the install section of the README: each command a user runs, and the safety guarantees about what hive installs and writes. The README's source install (`cargo install --path crates/cargo-hive`) is the source form of `cargo install cargo-hive`. If a line changed, fix it in this rerun's own commit.
