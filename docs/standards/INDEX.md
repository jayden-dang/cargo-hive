# Engineering standards index

Status: Approved
Date: 2026-10-28

## Purpose and boundary

How code, tests and docs in cargo-hive are written and gated. These rules were ratified in the define-project interview on 2026-10-28 from conventions the foundation (CORE) already follows. Plans cite these files in their Global Constraints instead of copying rules into each plan. Architecture invariants live in `docs/architecture/INDEX.md`, product scope in `docs/product/vision.md`.

## Standards catalog

| Document | Path | Scope |
|---|---|---|
| Testing | `docs/standards/testing.md` | Merge gate, test layout and style, what tests may touch |
| Errors and logging | `docs/standards/errors-logging.md` | Error types, refusals and exit codes |
| Code | `docs/standards/code.md` | Bounds and deadlines, `unsafe`, dependencies, requirement IDs, comments, measured claims |

## Source of truth rule

Canonical engineering standards live under `docs/standards/`. `docs/product/guidelines.md` is a **pointer only** after migration (legacy fallback only while unmigrated content remains). Never maintain parallel SSOTs.

## Not ARCH-N

Standards docs do **not** redefine greppable `ARCH-N` architecture invariants.
