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
- **Default PR base:** `main` — `land-branch` reads it as the third rung of its base-resolution ladder.

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
| Typecheck | `cargo check --all-targets` and `cargo check --target x86_64-unknown-linux-gnu` |
| Lint | `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` |
| Unit tests | `cargo test` |
| E2E / smoke | none — the integration tests in `tests/` already drive the real binary |

`cargo check --target x86_64-unknown-linux-gnu` needs that rustup target installed. On a
machine without it, record "non-macOS compile check not run: target not installed"; never
install it as a side effect.

Single test file: `cargo test --test <file>` for `tests/<file>.rs`, or
`cargo test --lib <module>::` for the unit tests in `src/<module>.rs`. Both build only
that test target.

Task gate (changed units + their dependents; `none` when the whole suite is fast):
none — one crate, and the whole suite takes about a minute.

Wave gate (compile + lint every target after a parallel merge; `none` = typecheck + lint):
none.

Manual performance check, not part of the gate: `cargo test --test guards -- --ignored`
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

1. Gate: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo check --target x86_64-unknown-linux-gnu`.
2. Version: set `version` in `Cargo.toml` to `X.Y.Z`, also set the same version in `plugin/.claude-plugin/plugin.json`, `plugin/.codex-plugin/plugin.json` and `plugin/skills/hive/SKILL.md` `metadata.hive-version` (tests/skill.rs fails if they differ), then `cargo check` so `Cargo.lock` follows.
3. Package check: `cargo package --list` shows only `src/`, `Cargo.toml`, `Cargo.lock`, `README.md` and license files; then `cargo publish --dry-run`.
4. Commit and tag: `git commit -am "release: vX.Y.Z"`, `git tag vX.Y.Z`, `git push origin main vX.Y.Z`.
5. Publish: `cargo publish` (needs `cargo login`; a published version cannot be undone, only yanked).
6. Smoke: `cargo install cargo-hive --version X.Y.Z --root <tmp>`; `<tmp>/bin/cargo-hive --version` prints `X.Y.Z`; `HIVE_HOME=<tmp>/home <tmp>/bin/cargo-hive doctor --json` exits 0.

`Cargo.toml` carries an `include` list (2026-10-30), so the package holds only `src/`,
`Cargo.toml`, `Cargo.lock` and `README.md`; the plugin (`plugin/`, `.claude-plugin/`) and
run records (`skills/`) never ship in the crate. Before the first release, add the two
license files `LICENSE-MIT` and `LICENSE-APACHE` for the declared `MIT OR Apache-2.0`;
the `include` list already names them.

## Paths

- Specs: `docs/specs/` (domain router `INDEX.md`, shard `catalog/hive.md`)
- ADRs: `docs/adr/`
- Glossary: `CONTEXT.md` (single context)
- Out-of-scope KB: `.out-of-scope/`
- Engineering standards: `docs/standards/`
- Product vision / architecture spine: `docs/product/vision.md`, `docs/architecture/INDEX.md`
- Roadmap: `docs/roadmap/INDEX.md`

Library docs: Context7 MCP (preferred); fall back to the crate's official docs or source.
