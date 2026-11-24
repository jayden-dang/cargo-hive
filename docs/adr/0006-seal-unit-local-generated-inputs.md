# ADR-0006: Verify unit-local Cargo-generated inputs

Status: Accepted (2026-11-24)

The Klynt proof uses generated Rust files and native archives as compiler inputs.
Warm reuse permits explicitly named inputs in their producing Cargo unit's output
directory when the reviewed producer, consumers, contents and custody through
scoped GC can be verified. This preserves the guard against undeclared retained
inputs while admitting the generated files required by the reviewed commands.
