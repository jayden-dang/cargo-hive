# Verify / control — cold-start drive recipe

Written by `configure-repo` (Decision M). Read by `validate-feature`,
`validate-ui`, `validate-api`, and `write-dogfood`. A later agent that has
never seen this app follows this file instead of inventing commands.

**App:** `cargo-hive` (invoked by users as `cargo hive <sub>`)
**Primary surface:** `CLI`
**Other surfaces:** none (no server, no web UI, no MCP; agents use the CLI through the hive skill, ROAD-12)

## Launch

Start command (must match `docs/agents/project.md` **Run locally (dev)**), from the repo root:

```
cargo build
export HIVE="$PWD/target/debug/cargo-hive"
export SCRATCH="$(mktemp -d /private/tmp/hive-verify-XXXXXX)"
export HIVE_HOME="$SCRATCH/home"; mkdir -p "$HIVE_HOME"
```

`HIVE_HOME` keeps every drive away from the user's real `~/.hive`. Never point it there.

Ready when: `"$HIVE" --version` prints `cargo-hive 0.1.0`.
Teardown: nothing keeps running; Cleanup removes `$SCRATCH`.

## Doctor

One check: is this instance worth driving?

```
"$HIVE" doctor --json
```

Pass: exit 0, checks (a)–(f) each `pass` (or `unavailable` with a reason), and
`$HIVE_HOME/capabilities.json` records `toolchain_release`. Fail: do not Drive; fix
Launch or record the blocker. Doctor builds a throwaway crate offline, so it needs a
working Rust toolchain.

## Drive

Harness: a shell, with `--json` on every command. Stdout carries exactly one JSON
receipt; human text goes to stderr. Assert on receipt fields (`counts`, `findings[].kind`)
and on the files under `$HIVE_HOME`, never on prose.

Fixture used by the features below (a registered repo with one built crate):

```
mkdir -p "$SCRATCH/app/src" && cd "$SCRATCH/app" && git init -q
printf '[package]\nname = "fixture"\nversion = "0.1.0"\nedition = "2021"\n' > Cargo.toml
echo 'pub fn answer() -> u32 { 42 }' > src/lib.rs
git add -A && git -c user.email=v@v -c user.name=v commit -qm init
cargo build --offline --quiet && cd -
```

Isolation: two instances can run side by side, each with its own `$SCRATCH` and
`$HIVE_HOME`. Status also scans `/private/tmp` and `$TMPDIR` read-only as secondary roots.

## Evidence

Keep under `.skills/verify/<date>-<feature>/` (gitignored): the receipt JSON of each
command and the re-read state files. A proof captures **the action and the resulting
state**, including a **re-read** of the file the feature claims to persist. A final
receipt alone is not enough.

Cleanup never deletes evidence.

## Cleanup

`rm -rf "$SCRATCH"` — the scratch dir Launch created, with its `HIVE_HOME` and fixture.
Leave `.skills/verify/`.

## Features

| Feature | How a user reaches it | Observable end state |
|---|---|---|
| `doctor` | `cargo hive doctor` | Receipt checks (a)–(f); `capabilities.json` re-read names the toolchain release |
| `repo add` / `repo list` | `cargo hive repo add <path>`, `cargo hive repo list` | `counts.registered = 1`; `registry.json` re-read lists the main checkout and its root commit |
| `status` — checkouts | `cargo hive status <repo>` | `counts.checkouts`, `protected`, `secondary`; a `checkout.protected` finding for the main |
| `status` — target dirs | `cargo hive status <repo>` | `counts.targets = 1`, `owned = 1`, `bytes > 0`; a `target.owned` finding; `snapshot.json` written |
| `gc` | `cargo hive gc [<repo>…] [--dry-run]` | Receipt counts (`sessions_deleted`, `freed_bytes`, …); dry-run changes nothing; cancel exits 130 |
| Receipt log | any command | Each command appends one line to `$HIVE_HOME/receipts/<date>.jsonl` |

### doctor

- **Sub-features:** checks (a) clone sharing, (b) Applesauce compression, (c) clone id, (d) exclusive lock proof, (e) owner-auth availability, (f) fingerprint format.
- **How to get to it (user POV):** run `cargo hive doctor` after install, and again after a toolchain change.
- **Driving it with the shell:** `"$HIVE" doctor --json > doctor.json`; assert exit 0 and `[c["id"], c["verdict"]]` for each of `doctor.json["checks"]`.
- **Gotchas:** below macOS 27 it still runs every check and reports the floor; check (e) never shows a dialog.
- **Persist check:** re-read `$HIVE_HOME/capabilities.json`: `toolchain_release` matches `rustc -V` at `$HIVE_HOME`, and `disabled` lists no capability when every check passed.

### repo add / repo list

- **Sub-features:** a path outside any git checkout is refused (`refusal` finding "the path is not inside a git checkout"); adding an already registered repo is a no-op (`counts.registered == 0`, no findings, exit 0).
- **How to get to it (user POV):** `cargo hive repo add ~/code/my-repo`, then `cargo hive repo list`.
- **Driving it with the shell:** `"$HIVE" repo add "$SCRATCH/app" --json` → `counts.registered == 1`, no findings; `"$HIVE" repo list --json`.
- **Gotchas:** registration resolves to the main checkout containing the path.
- **Persist check:** re-read `$HIVE_HOME/registry.json`: `repos[0].main` is `$SCRATCH/app` and `root_commits` has one entry; a second `repo list` in a new shell shows the same repo.

### status — checkouts

- **Sub-features:** protected, secondary and missing classification (C1).
- **How to get to it (user POV):** `cargo hive status <repo>`.
- **Driving it with the shell:** `"$HIVE" status "$SCRATCH/app" --json` → `counts.checkouts == 1`, `counts.protected == 1`, and a `checkout.protected` finding for `$SCRATCH/app`.
- **Gotchas:** status refuses until doctor results exist for the active toolchain; a fresh run may also list `walk.unreadable` findings for system entries under `/private/tmp`, which are expected.
- **Persist check:** presentational for checkouts; the receipt line is the persisted record (see Receipt log).

### status — target dirs

- **Sub-features:** owner or orphan candidate, sizes (skipped with `--no-sizes`), fingerprint units and edges.
- **How to get to it (user POV):** `cargo hive status <repo>`, optionally `--no-sizes`.
- **Driving it with the shell:** the same command → `counts.targets == 1`, `counts.owned == 1`, `counts.bytes > 0`, and a `target.owned` finding for `$SCRATCH/app/target`.
- **Gotchas:** `cargo metadata` runs offline and locked; a second run within the same stamp is a cache hit (`counts.metadata_cache_hits`).
- **Persist check:** re-read `$HIVE_HOME/snapshot.json` (present after the run); a second `status` reports `metadata_cache_hits >= 1` and `metadata_calls == 0`.

### gc

- **Sub-features:** delete superseded incremental sessions and rustc temp dirs; a session with a lock file is deleted under that lock, and a session with none is deleted without one; `--dry-run` reports the same counts without deleting; cancel (SIGINT or SIGTERM) exits 130 with a receipt.
- **How to get to it (user POV):** `cargo hive gc <repo>` after a build has left old sessions; optionally `--dry-run`.
- **Driving it with the shell:** `"$HIVE" gc "$SCRATCH/app" --json > gc.json`; assert exit 0 and counts such as `sessions_deleted` / `freed_bytes`. A dry run (`--dry-run`) leaves the target tree unchanged. Cancel returns exit 130 and still appends a receipt whose `command` is `gc`.
- **Gotchas:** gc refuses until doctor results for the active toolchain allow both deletion and fingerprint reading, which includes the current lock check; held session or build locks are skipped (`skipped_held`).
- **Persist check:** presentational for deletions; the receipt line is the persisted record (see Receipt log). A dry run must leave `$SCRATCH/app/target` byte-identical to before.

### Receipt log

- **How to get to it (user POV):** every command, CLI or `--json`.
- **Driving it with the shell:** run any two commands above.
- **Persist check:** `wc -l "$HIVE_HOME"/receipts/*.jsonl` grows by one per command, and each line parses as one JSON receipt naming its `command`.
