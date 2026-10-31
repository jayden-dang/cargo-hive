# Hive catalog

Recognized feature cards for this domain. Add a row **before** writing
`requirements.md` for a new feature. Status is one of
`Draft | Approved | In-progress | Implemented | Shipped | Recognized`.
`Owner` is the team or person a PR that changes this feature's spec or surface should
notify (`@org/team`). A blank cell falls back to the `Owner:` line below; no owner means
nobody is notified.

| Code | Feature | Spec | Status | Owner | Roadmap item | Match terms | Surface roots |
|---|---|---|---|---|---|---|---|
| CORE | Foundation: repo registry, checkout discovery and classification, Cargo target identification, fingerprint reader, receipts, doctor | ../2026-10-25-foundation/ | Implemented |  | ROAD-1 | registry, repo add, checkout, worktree, classify, protected, secondary, target dir, CACHEDIR.TAG, fingerprint, receipt, doctor | `src/` |
| AUTOGC | Auto-tier GC: superseded incremental sessions and rustc temp entries, deleted under their owners' locks, with per-inode freed bytes | ../2026-10-28-auto-gc/ | Implemented |  | ROAD-2 | gc, reclaim, incremental session, rustc temp, rmeta, try-lock, flock, dry-run, freed bytes | `src/` |
| EXPLAIN | Explain: why a crate has N copies in a target dir (copy groups by feature set, mode and profile hash), feature divergence within one mode and target kind, and failed units | ../2026-10-29-explain/ | Implemented |  | ROAD-3 | explain, copies, copy group, variant, feature set, features, declared_features, divergence, mode, failed unit, output-*, aborting due to | `src/` |
| SKILL | Agent skill: the hive skill shipped as a plugin from this repo's marketplace, teaching agents to drive MILE-1 commands through the CLI with `--json`, kept in step with the binary's version and flags | ../2026-10-29-agent-skill/ | Implemented |  | ROAD-12 | skill, SKILL.md, plugin, marketplace, agent, hive-version, eval, TESTS.md | `plugin/`, `skills/hive/` |
| LIVE | Closure liveness: which units in a Cargo target dir are live (reachable from units compiled inside the GC window over the fingerprint graph of the whole target dir) and which dead units' hash-keyed files may be collected, failing closed as Undecided; proven by a deletion trial on a copy of a real target | ../2026-10-30-closure-liveness/ | Implemented |  | ROAD-5 | liveness, live, dead unit, closure, roots, window, invoked.timestamp, undecided, cross-profile, vacuous, prove | `src/` |
