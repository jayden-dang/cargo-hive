# ADR-0003: Seed receives the liveness verdict as a function the binary passes in

Status: Proposed (2026-11-13, unattended ROAD-15 run; the operator decides at the spec sheet)

When hive is split into crates (ROAD-15), seed needs the closure-liveness verdict: it keeps only live units
(seed/source.rs:183, :327). But ROAD-15 also requires that changing how gc decides a dead unit rebuilds the gc
crate and the binary, not seed (ROAD-15.md:8). So the result types (`Liveness`, `UnitRef`, `DeadUnit`,
`Uncertain`, `LivenessFn`) go in `cargo-hive-core`, the algorithm stays in `cargo-hive-gc`, and seed's context
carries a `LivenessFn` that the binary fills with `cargo_hive_gc::liveness`. The rejected alternative put liveness
in core or its own crate under both gc and seed, which is simpler but rebuilds seed on every liveness edit.
Accepted cost: one function field to wire at each seed entry point, and seed tests that reach the verdict live in
the binary's tests. Editing the shared fingerprint helpers in core (`embedded_hashes`, `unit_timestamp`) still
rebuilds seed; that is legitimate sharing, not the dead-unit decision.
Revisit if seed ever needs liveness internals beyond the verdict, or if ROAD-15's example 1 is dropped.
