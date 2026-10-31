# cargo-hive glossary

Terms settled during the 2026-10-25 interview and the CORE design. One canonical term per concept.

**hive**:
The product (crate `cargo-hive`). Invoked as `cargo hive <sub>`, or as `cargo-hive <sub>` with the same arguments, the form agents that cannot call Cargo subcommands use.
_Avoid_: `hive <cmd>` written as a command; no `hive` binary exists

**Checkout**:
A working tree of a registered repo: the main checkout, a linked worktree, or a clone.
_Avoid_: workspace (that is Cargo's term), copy

**Protected checkout**:
A checkout where hive only deletes (auto tier or approved plan) and reads. A registered main is always protected, and so is anything unclassifiable.
_Avoid_: main-only, locked checkout

**Secondary checkout**:
A linked worktree inside its own main, or a checkout under a secondary root. Agents may apply plans to it, and hive may seed it.
_Avoid_: scratch checkout, temp worktree

**Missing**:
A flag on a protected checkout whose directory is gone, not a third checkout class.
_Avoid_: missing class, deleted checkout

**Secondary root**:
A configured directory whose checkouts are secondary. The defaults are `/private/tmp` and `$TMPDIR`.
_Avoid_: temp root (the default is only one example)

**Foreign mode**:
The only v1 mode: every checkout builds at its own path with stock Cargo, and only registry units are seeded.
_Avoid_: unmanaged mode

**Slot mode**:
Deferred: a fixed pool of worktree paths that would allow sharing workspace-crate incremental state.

**Cargo target dir**:
A directory whose `CACHEDIR.TAG` says "created by cargo" and that holds a profile dir with fingerprints.
_Avoid_: target folder, build folder

**Profile dir**:
A Cargo target dir's per-profile subdirectory (`debug/`, `release/`, `<triple>/<profile>/`) that holds fingerprints and artifacts.
_Avoid_: profile folder, build profile

**Build directory**:
Where Cargo keeps intermediates and fingerprints (`build_directory` in `cargo metadata`); it can differ from the target directory.
_Avoid_: build-dir cache, intermediate dir

**Unit**:
One Cargo compilation unit: a directory under `.fingerprint/` holding exactly one fingerprint JSON, identified by `<name>-<metadata hash>` and its fingerprint hash.
_Avoid_: crate (a crate has many units), artifact

**Fingerprint format signature**:
The exact shape hive accepts for a unit: the 11 JSON keys with fixed types, `deps` as `[u64, string, bool, u64]`, and a 16-hex hash file. Any other shape is unsupported.
_Avoid_: fingerprint version

**Orphan candidate**:
The status name for a target no checkout owns when resolution succeeded. It is a Cargo target dir that is neither the target directory nor the build directory Cargo resolves for any known checkout.
_Avoid_: stale target, dead target

**Store**:
Hive's content store of compressed registry units under `~/.hive/store/<rustc>/`. It is the only place that gets compressed and the only source of clones.
_Avoid_: cache, shared target

**Seed**:
Cloning store units into a secondary checkout's target with APFS clonefile.
_Avoid_: warm, prefill, restore

**Fill**:
Copying new registry units from a live checkout into the store, under a try-lock on that checkout's target, then compressing them there.
_Avoid_: harvest

**Roots**:
Units whose `invoked.timestamp` mtime falls inside the GC window, or lies later than now.

**Closure liveness**:
A unit is live if it is reachable from the roots over the fingerprint graph of its whole target dir, edges resolving across every profile dir in it.
_Avoid_: reachability GC, mark phase

**Dead unit**:
A unit in a Decided target dir that is not live, and whose hash-keyed `deps/` and `examples/` files plus fingerprint dir may be collected. Build-script, run-build-script, doc and `other`-kind units are never dead units.
_Avoid_: garbage unit, stale unit, orphan unit

**Unit hash**:
The 16 lowercase hex characters after the last `-` of a unit's directory name under `.fingerprint/`; it names the unit's files in `deps/` and `examples/`, and is not the fingerprint stored in its hash file.
_Avoid_: fingerprint, metadata hash

**Undecided**:
The liveness verdict for a target dir whose graph cannot prove reachability (a not-current unit inside the window, an unresolved edge from a live unit, or an unsupported profile); it has no dead units, and each reason names what clears it.
_Avoid_: unknown, partial

**Vacuous**:
The outcome of a liveness proof window that could not have caught a fault (an Undecided result, no dead file to delete, a warm-up or baseline that failed or rebuilt units, a failed or silent replay, or a clone that differs from its source); never counted as a pass.
_Avoid_: skipped, trivially passing

**Auto tier**:
GC that runs without a plan: `rustc*`/`rmeta*` temp entries and superseded incremental sessions, always after taking the owner's lock.

**Plan tier**:
GC through `cargo hive gc --plan` then `apply`: closure deletion and whole orphan targets.

**Receipt**:
The deterministic counters a hive command returns. It never contains durations or estimated time saved.
_Avoid_: report, summary

**needs-human**:
The status `apply` returns for a protected plan. Only `cargo hive approve <id>` opens the owner-authentication dialog.
