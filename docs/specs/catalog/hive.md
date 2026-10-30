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
| SKILL | Agent skill: the hive skill shipped as a plugin from this repo's marketplace, teaching agents to drive MILE-1 commands through the CLI with `--json`, kept in step with the binary's version and flags | ../2026-10-29-agent-skill/ | In-progress |  | ROAD-12 | skill, SKILL.md, plugin, marketplace, agent, hive-version, eval, TESTS.md | `skills/hive/` |
