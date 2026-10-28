# Errors and logging standards

Status: Approved
Date: 2026-10-28

## Purpose and boundary

How cargo-hive represents failures and refusals, and how a command ends. Receipt content is an architecture rule (ARCH-10), not repeated here.

## Error handling

| Context | Convention |
|---|---|
| Error types | Typed enums with a hand-written `Display` and `std::error::Error` impl; no `thiserror` or `anyhow` |
| Error messages | Say what is wrong and what to do next |
| Expected refusals | Data, not errors: `Result<Result<Outcome, Refusal>, Error>` keeps a refusal apart from an infrastructure failure |
| Ending a command | Commands return an exit code: 0 ok, 1 refusal or failure, 2 unsupported OS. Nothing calls `process::exit` outside `main.rs` |

## Logging

None — hive has no log facility. Each command's receipt is its record, appended to `~/.hive/receipts/<date>.jsonl`; the 2026-10-28 interview ratified no logging convention beyond that.

## Sensitive data

None — the 2026-10-28 interview ratified no rule.

## Not ops observability map

Conventions only. Dashboards/alerts live in ops observability docs when present.
