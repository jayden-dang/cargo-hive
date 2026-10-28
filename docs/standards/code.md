# Code standards

Status: Approved
Date: 2026-10-28

## Purpose and boundary

How cargo-hive source and docs are written, beyond tests and error handling. There is no pack template for this file; it follows the shape of the other standards files.

## Rules

| Area | Rule |
|---|---|
| Bounds | Every directory walk, file read and child process has an explicit bound or deadline. Hitting a bound is an error, never a silent skip |
| `unsafe` | Only at FFI sites, and each site carries a `// SAFETY:` comment |
| Dependencies | A new dependency needs the owner's approval |
| Requirement IDs | Never in source code, comments or test names; they live in requirements, design and task footers |
| Module comments | Each module opens with a `//!` comment giving its purpose and contract |
| Comments | Describe the code, not the plan that produced it |
| Measured claims | A measured number in docs cites dated evidence: a spike section, a log or a bench run |

## Not ARCH-N

Standards docs do **not** redefine greppable `ARCH-N` architecture invariants.
