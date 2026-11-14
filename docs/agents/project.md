# Project configuration (agent-facing)

Written by `configure-repo`. Skills read this file for repo-specific **machine config** —
commands, globs, paths — plus **posture** and **team** (below). Engineering standards
(merge gate, test layout, errors, code rules) live in `docs/standards/`, the single source
for them; there is no `docs/product/guidelines.md`.

## Project posture

The project's standing intent, lifecycle phase, and compatibility obligation. Skills read
this instead of re-asking: `frame-change` and `clarify-decisions` right-size migration /
compat / deprecation against **Compat obligation**; `interpret-session` reuses all three.
`clarify-decisions` production coverage needs Delivery **Production** + Lifecycle
**Cut Released / Scaling / Maintenance** + operate/launch surface (or an ops ask); MVP,
absent, and Early/Active stay off. Edit these lines when the project moves phase.

**SSOT:** the **Compat obligation** values and what each one means live only here. Skills
carry a one-clause copy of the derivation because a repo configured before the field existed
has no Compat obligation text in this file for them to read — change the meanings here, and
the derivation in `frame-change`, `clarify-decisions`, and `interpret-session` with it.

- **Delivery intent:** `Production` — the quality bar the output must meet. A bar, not a release state: **Production** says how well the code must be built, never that the project has shipped.
- **Lifecycle stage:** `Active development` — where the project is in its life. Move it to `Cut Released` when v0.1.0 is published.
- **Compat obligation** is not written, so it is derived from the lifecycle stage: Idea / Early development / Active development → **None**; Cut Released / Scaling / Maintenance → **External**. **None:** nothing outside this repo depends on any current schema, endpoint, or file format. **Internal:** other services or teams in the same org read them. **External:** users, customers, or third-party clients depend on them. Write a `Compat obligation` line only to override the derivation.
- **Default PR base:** `main`
  `land-branch` reads it as the third rung of its base-resolution ladder; keep the value alone on its line, since `refresh-lanes` reads the whole rest of the line as the branch name.
- **Default landing action:** `merge` — when a request names no outcome, `land-branch` merges locally after a green gate, pushes `main`, and closes the tickets it completed. This line is the standing permission for that push.

These are distinct from the product **Goals** in `docs/product/vision.md` (what success
looks like): posture is *how carefully to build right now*, not *what to build*.

## Team

Who works on this repo and how skills should package collaboration.
Skills that plan, review, or hand off read this section when present and
right-size **packaging** only (Solo / Small / Multi) — Iron Law gates never
change. Edit freely; re-run `/configure-repo` to re-draft from git/CODEOWNERS.
If this section is absent, skills do not invent a team.

**SSOT:** **band** derivation and the **packaging** matrix live only here.
Consumers **read** this section; they do not re-copy these rules into skill bodies.

### Roster

- Contributor — Jayden Đặng

Suggested roles (freeform allowed): Tech Lead, Backend Engineer, Frontend Engineer, Full-stack Engineer, Designer, Product Manager, QA, DevOps/SRE, Docs.

### Ownership notes (optional)

None — there is no CODEOWNERS file.

### Workflow band

- **Override (optional):** blank — skills derive the band.
- **Derive (when override blank):**
  1. Headcount from **Roster only**: each `Role — Name` = 1; each `N × Role` / `N Role(s)` adds N.
     Ignore Ownership notes and placeholders (`<…>`).
  2. Buckets: empty roster → **no band** (same packaging as Team absent); 1 → **Solo**;
     2–4 → **Small**; ≥5 → **Multi**.
  3. Specialty upgrade only: if **Small** and ≥3 distinct role titles (case-insensitive, trimmed),
     upgrade to **Multi**. Never downgrade Multi→Small.

With the roster above the band is **Solo**.

### Packaging matrix

| Band | Packaging |
|---|---|
| **Solo** | Lean multi-person ritual language; no invented peer reviewers/assignees; agent-as-pair; full gates |
| **Small** | Design-review checkpoints; ownership boundaries via optional freeform notes; name people when roster has names |
| **Multi** | CODEOWNERS-aware review language when ownership notes exist; explicit review responsibilities as prose; write-handoff/docs emphasis |
| **(no band)** | Team absent, or empty roster with blank override — pre-feature default; do not invent a team; do not hard-fail |

## verify commands

Run in this order; all must pass before any completion claim.

| Check | Command |
|---|---|
| Typecheck | `cargo check --workspace --all-targets` and `cargo check --workspace --target x86_64-unknown-linux-gnu` |
| Lint | `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` |
| Unit tests | `cargo test --workspace` |
| E2E / smoke | none — the integration tests in `crates/cargo-hive/tests/` already drive the real binary |

`cargo check --workspace --target x86_64-unknown-linux-gnu` needs that rustup target installed. On a
machine without it, record "non-macOS compile check not run: target not installed"; never
install it as a side effect.

Single test file: `cargo test -p <member> --test <file>` for a member's `tests/<file>.rs`, or
`cargo test -p <member> --lib <module>::` for the unit tests in that member's `src/<module>.rs`. Both build only
that test target.

Task gate (tests of the changed member and the members above it, then compile every other member):
"Above" means members that depend on the changed member (later in the layer order), not a lower layer number. Layer order: platform 0, core 1, state 2, cargo 3, doctor/seed/gc 4, cargo-hive 5. State does not depend on core. Feature crates do not depend on each other. Example for a change in `cargo-hive-doctor`: `cargo test -p cargo-hive-doctor && cargo test -p cargo-hive && cargo check -p cargo-hive-platform --all-targets && cargo check -p cargo-hive-core --all-targets && cargo check -p cargo-hive-state --all-targets && cargo check -p cargo-hive-cargo --all-targets && cargo check -p cargo-hive-seed --all-targets && cargo check -p cargo-hive-gc --all-targets`.

Wave gate (compile + lint every target after a parallel merge; `none` = typecheck + lint):
none.

Manual performance check, not part of the gate: `cargo test -p cargo-hive --test guards -- --ignored`
runs CORE-6.1's timing test against klynt on the proof machine.

The whole suite runs once, at close. Each task runs its task gate instead, escalating
to the whole suite when the change reaches what the build graph cannot see (schema,
migrations, shared fixtures, manifests, generated contracts, a root package).

The traceability check is not a command here — the `audit-trace` skill runs it as
`grep`/`git` over `docs/specs/` (and the architecture spine). It is **docs-only**
and does not grep application tests for requirement IDs. Requirement IDs never appear
in source, comments or test names (`docs/standards/code.md`); legacy annotations, if
any, are ignored, and no new ones are added.

Specs directory override: none — specs live in `docs/specs/`.

## Run locally (dev)

How to start the app for user-facing acceptance checks (read by `validate-feature`,
`validate-api` and `write-dogfood`). The full cold-start recipe is `docs/agents/verify.md`.

| Surface | Start command | Ready signal |
|---|---|---|
| CLI | `cargo build`, then run `target/debug/cargo-hive <sub> --json` with `HIVE_HOME` set to a scratch dir under `/private/tmp` | `target/debug/cargo-hive --version` prints `cargo-hive 0.1.0` |

There is no server and no web UI.

## Remote environments

None — not deployed. hive is a local CLI with no hosted component.

## release steps

1. Gate: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo check --workspace --target x86_64-unknown-linux-gnu`.
2. Version: the release after 0.1.0 is `0.1.1`. Set `[workspace.package] version` to that number (members inherit it; do not set a version inside each member), also set the same version in `plugin/.claude-plugin/plugin.json`, `plugin/.codex-plugin/plugin.json` and `plugin/skills/hive/SKILL.md` `metadata.hive-version` (tests/skill.rs fails if they differ), then `cargo check` so `Cargo.lock` follows. Commit that bump before the package check: `git commit -am "release: v0.1.1"`.
3. Package check: `cargo package --workspace --list --allow-dirty` shows, for every member, only `src/` files, `Cargo.toml`, `Cargo.toml.orig`, `.cargo_vcs_info.json`, `Cargo.lock`, `README.md` and the two license files (no `tests/` path). Then `CARGO_TARGET_DIR=target/package-verify cargo publish --workspace --dry-run --allow-dirty` and expect "aborting upload due to dry run" for each member in Cargo's dependency order: cargo-hive-platform, cargo-hive-core, cargo-hive-state, cargo-hive-cargo, cargo-hive-doctor, cargo-hive-gc, cargo-hive-seed, cargo-hive. Then probe every member:

   ```sh
   scripts/fresh-probe.sh \
     crates/platform/src/lib.rs cargo-hive-platform \
     crates/core/src/lib.rs cargo-hive-core \
     crates/state/src/lib.rs cargo-hive-state \
     crates/cargo/src/lib.rs cargo-hive-cargo \
     crates/doctor/src/lib.rs cargo-hive-doctor \
     crates/gc/src/lib.rs cargo-hive-gc \
     crates/seed/src/lib.rs cargo-hive-seed \
     crates/cargo-hive/src/lib.rs cargo-hive
   ```

   Each pair must print `GREEN`. The verify build must never use the repo's own `target/` (FIX-1).
4. Tag and push the release commit from step 2: `git tag v0.1.1`, `git push origin main v0.1.1`.

Crate names approved: 2026-11-14

5. Publish: stop unless the line `Crate names approved:` above holds a date. Then `cargo search <name> --limit 1` for every member and stop, naming the member, if an exact-name hit is a crate this project does not own. Then `CARGO_TARGET_DIR=target/package-verify cargo publish --workspace` (needs `cargo login`; a published version cannot be undone, only yanked). On a partial failure stop and report which members uploaded. Steps 1–4, including the dry-run, work on working names.
6. Smoke: `cargo install cargo-hive --version 0.1.1 --root /private/tmp/hive-release/inst`; `/private/tmp/hive-release/inst/bin/cargo-hive --version` prints `0.1.1`; `HIVE_HOME=/private/tmp/hive-release/home /private/tmp/hive-release/inst/bin/cargo-hive doctor --json` exits 0; the same `scripts/fresh-probe.sh` member pairs as step 3 print `GREEN`; then

   ```sh
   HIVE_NEW_BIN=/private/tmp/hive-release/inst/bin/cargo-hive \
   HIVE_V010_BIN=/private/tmp/hive-0.1.0/target/debug/cargo-hive \
     cargo test -p cargo-hive --test capture -- --ignored
   ```

   Stop on any difference. The capture test takes the new binary from `HIVE_NEW_BIN` when set, else `CARGO_BIN_EXE_cargo-hive`.

Each member's `Cargo.toml` carries an `include` list so the package holds only `src/`,
`Cargo.toml`, `README.md` and the two license files `LICENSE-MIT` and `LICENSE-APACHE`;
Cargo also lists `Cargo.lock`, `Cargo.toml.orig` and `.cargo_vcs_info.json` even when
`include` omits them. The plugin (`plugin/`, `.claude-plugin/`) and run records
(`skills/`) never ship in any crate.

## Paths

- Specs: `docs/specs/` (domain router `INDEX.md`, shard `catalog/hive.md`)
- ADRs: `docs/adr/`
- Glossary: `CONTEXT.md` (single context)
- Out-of-scope KB: `.out-of-scope/`
- Engineering standards: `docs/standards/`
- Product vision / architecture spine: `docs/product/vision.md`, `docs/architecture/INDEX.md`
- Roadmap: `docs/roadmap/INDEX.md`

Library docs: Context7 MCP (preferred); fall back to the crate's official docs or source.
