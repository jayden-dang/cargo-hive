# ADR-0008: Keep generated-input evidence in content-addressed manifests

Status: Accepted (2026-11-24)

A real workspace retains hundreds of named generated files per warm checkout.
Klynt retains 654 files and 31.3 MB, and per-file evidence for them does not
fit the 1 MiB slot state that every lease transition rewrites. Each producer's
evidence is therefore an immutable private manifest. The manifest is named by
its own SHA-256 and written durably before the slot record that references it.
The slot record keeps only that digest, so the state stays small. Every file is
still named and rechecked, as ADR-0006 requires.

Rejected: raising the slot state bound, which slows every transition, and one
digest per output directory, which drops ADR-0006's explicitly named files.
Consistent with ADR-0004: the manifest carries `layout` first. It is immutable,
written under the state lock, and referenced only after it is durable, which answers
the two-file atomicity objection for creation. A lost or restored manifest is not
repaired; it keeps the slot unavailable.
