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
| GCPLAN | Plan-tier GC: `cargo hive gc --plan` stores a one-use plan of dead-unit files per checkout class, and `apply` rechecks liveness, ownership and locks before deleting, for every class, with no owner step; an orphan plan removes that target directory | ../2026-11-01-gc-plan-apply/ | Implemented |  | ROAD-6 | plan, apply, gc --plan, plan id, one-use, stale, identity fence, outcome, protected, secondary, orphan, window, cargo-lock, skipped_held, undecided | `src/` |
| SEED | Store and seed: a compressed, hive-owned store of registry units, and `cargo hive seed`, which fills the store from live targets and clones into a new secondary checkout, with per-target counts of seeded units in `status` | ../2026-11-02-store-seed/ | Implemented |  | ROAD-8 | store, seed, fill, clone, clonefile, compress, Applesauce, registry unit, toolchain table, seed record, reused, CACHEDIR.TAG | `src/` |
| BENCH | Bench: `cargo hive bench`, repeated cold-versus-seeded first builds of fresh secondary checkouts the caller made, reporting per sample and per arm the measured build and seed milliseconds (median, p90, spread) with load average, in the existing receipt | ../2026-11-09-bench/ | In-progress |  | ROAD-11 | bench, cold sample, seeded sample, prime, median, p90, spread, load average, timing, milliseconds, ABBA, wrapper | `src/` |
