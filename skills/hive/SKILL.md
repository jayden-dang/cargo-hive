---
name: hive
description: Reclaim disk space from Rust build output safely. Use when a target/ directory is large, a disk is filling up, you need to reclaim Rust build space, or the same crate is built in many copies across checkouts and worktrees. Use this instead of cargo clean or deleting target/.
metadata:
  hive-version: "0.1.0"
---

Placeholder body for the hive skill.

## Command lines

```
cargo hive doctor --json
cargo hive repo add /path/to/repo --json
cargo hive repo list --json
cargo hive status /path/to/repo --json
cargo hive gc /path/to/repo --dry-run --json
cargo hive gc /path/to/repo --json
cargo hive explain /path/to/repo --top 20 --json
```
