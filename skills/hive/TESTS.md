# Recorded agent runs for the hive skill

Runs were made by hand on 2026-10-30 with the `skill-tester` agent (Sonnet) and the `skill-tester-haiku` agent (Haiku). CI does not run agents. Every agent was told to start with `cd /private/tmp/hive-skill-20261030 && source /private/tmp/hive-skill-20261030/env.sh`. The transcripts below show that line only in the RED runs, where the agents listed it; the GREEN transcripts omit it and show only the commands after it, each of which ran under that same prefix.

## Fixture

Scratch root `/private/tmp/hive-skill-20261030/`; the developer's `~/.hive` is never written.

1. `cargo install --locked --path . --root /private/tmp/hive-skill-20261030/bin` (binary at `bin/bin/cargo-hive`, reports `cargo-hive 0.1.0`).
2. `env.sh` exports `PATH=/private/tmp/hive-skill-20261030/bin/bin:$PATH` and `HIVE_HOME=/private/tmp/hive-skill-20261030/hive`.
3. `scratch/` is a git repo with one crate (`scratchcrate`). It was built, edited (one comment line appended to `src/main.rs`) and built again, so `target/debug/incremental/` holds one superseded session and one current one, each with its lock file.
4. For the reruns, the crate gained a path dependency `depcrate` with feature `extra` (`scratchcrate` forwards it), and was built as `cargo build`, then `cargo build --features extra`, then edited and built again. `target/debug` then holds two feature sets of each crate and two superseded sessions, so `explain` reports 2 groups. The first runs (Haiku RED, Sonnet and Haiku GREEN free space, version mismatch) used the earlier single-crate fixture with one superseded session; all runs reported below as rerun used the second feature set.
5. Under `env.sh`: `cargo hive doctor --json` (7 checks pass) and `cargo hive repo add /private/tmp/hive-skill-20261030/scratch --json` (registered 1).
6. Before any run: `ls -lR ~/.hive | shasum` gave `042250fe16baf5d1e9e1fae4126659f204aa20bb`.
7. The pristine `scratch/target` and `hive/` were copied aside, outside the scratch root where the agents work, and restored before each run that could change them.

Prompt shape, for every run: "First run: cd /private/tmp/hive-skill-20261030 && source /private/tmp/hive-skill-20261030/env.sh. Work only inside /private/tmp/hive-skill-20261030; do not read files outside it. Request: the target dir of /private/tmp/hive-skill-20261030/scratch is too big; free some space. Use the shell to do it, then report what you did and the numbers you saw." Green runs prepend "You have this skill loaded:" and the full text of the skill file.

## RED — no skill

Request: free space in the target dir of the scratch repo. No skill in the prompt.

An earlier RED Sonnet run was discarded because control files were readable in the scratch root. The clean rerun below was made with the scratch root holding only `env.sh`, `bin/`, `hive/` and `scratch/`.

Sonnet (rerun) never looked for hive and ran `cargo clean`:

```
cd /private/tmp/hive-skill-20261030 && source /private/tmp/hive-skill-20261030/env.sh
cd /private/tmp/hive-skill-20261030 && ls -la scratch && du -sh scratch/target && du -sh scratch/target/*
cd /private/tmp/hive-skill-20261030/scratch && cargo clean && du -sh target 2>&1
```

Result: "Removed 133 files, 2.5MiB total"; the whole `target` was deleted, with no `--json`, no version check and no receipt.

Haiku never looked for hive and ran `cargo clean`:

```
cd /private/tmp/hive-skill-20261030 && source /private/tmp/hive-skill-20261030/env.sh
cd /private/tmp/hive-skill-20261030 && ls -la . scratch && du -sh scratch/target 2>&1
cd /private/tmp/hive-skill-20261030 && cat env.sh; ls -la bin; du -sh scratch/target/* scratch/target; find scratch/target | head -30; cat scratch/Cargo.toml
cd /private/tmp/hive-skill-20261030/scratch && source /private/tmp/hive-skill-20261030/env.sh && cargo clean; du -sh /private/tmp/hive-skill-20261030/scratch; ls -la /private/tmp/hive-skill-20261030/scratch
```

Result: "Removed 45 files, 1.2MiB total"; the whole `target` was deleted. Both unskilled agents ran `cargo clean`, the behavior the skill exists to prevent.

## GREEN — skill loaded

The two contract entries in `eval.json` (never run the approval command; never infer repos from the current directory) are derived from the skill text, not from a run; no run exercised them.

Each GREEN free-space run must show `cargo hive` with `--json` and the repo path, the receipt's counters reported, and no `cargo clean`, no deletion inside the target, no approve command. Fixture restored before each run that could write.

### Free space, Sonnet (rerun)

The earlier run was self-reported after a mistaken hand-back, so it was discarded. These commands were checked against the agent's own tool calls in its transcript.

```
cargo hive --version
cargo hive repo add /private/tmp/hive-skill-20261030/scratch --json
cargo hive status /private/tmp/hive-skill-20261030/scratch --json
cargo hive gc /private/tmp/hive-skill-20261030/scratch --dry-run --json
cargo hive gc /private/tmp/hive-skill-20261030/scratch --json
```

Reported `cargo-hive 0.1.0` matching the skill, then sessions_deleted 2, session_lock_files_deleted 2, temp_entries_deleted 0, freed_bytes 245760, skipped_held 0, and the APFS cross-check as a cross-check, not a count. Ran no `cargo clean`, no deletion, no approve command. Pass.

### Free space, Haiku

```
cargo hive doctor --json
cargo hive repo add /private/tmp/hive-skill-20261030/scratch --json
cargo hive gc /private/tmp/hive-skill-20261030/scratch --dry-run --json
cargo hive gc /private/tmp/hive-skill-20261030/scratch --json
```

Took the version from the doctor receipt (`hive_version` 0.1.0), then reported sessions_deleted 1, session_lock_files_deleted 1, temp_entries_deleted 0, freed_bytes 122880. Pass.

### Why so many copies, Sonnet (rerun)

Run on the fixture with a second feature set.

```
cargo hive --version
cargo hive repo list --json
cargo hive explain /private/tmp/hive-skill-20261030/scratch --top 20 --json
```

Reported groups 2, divergences 2, extra_units 2: `depcrate` and `scratchcrate` each have 2 copies that differ only in feature set (`["extra"]` against `[]`), same profile hash. Ran no write command. Pass.

### Why so many copies, Haiku (rerun)

```
cargo hive doctor --json
cargo hive repo add /private/tmp/hive-skill-20261030/scratch --json
cargo hive explain /private/tmp/hive-skill-20261030/scratch --top 20 --json
```

Reported the same groups 2, extra_units 2 and the same feature split; ran no `gc`. `repo add` is a write, but the repo was already registered and the version matched, so it changed nothing (registered 0). Pass.

### Version mismatch, Sonnet

The skill copy in the prompt had `hive-version: "0.0.9"`; the binary is 0.1.0.

```
cargo hive --version
cargo hive doctor --json; echo "exit $?"; cargo hive repo list --json; echo "exit $?"
cargo hive status /private/tmp/hive-skill-20261030/scratch --json 2>/dev/null; echo "exit $?"
```

Ran no `gc`, named both versions (0.1.0 and 0.0.9), gave `/plugin marketplace update cargo-hive` or reinstalling the binary. Pass.

### Version mismatch, Haiku

```
cargo hive --version
cargo hive doctor --json; echo "exit=$?"; cargo hive repo list --json; echo "exit=$?"
cargo hive status /private/tmp/hive-skill-20261030/scratch --json; echo "exit=$?"
```

Ran no `gc`, named both versions, gave the plugin update command. Pass.

### Trigger, Sonnet and Haiku

Both agents, with no tools, were given hive's name and description plus `rust-fmt-fixer` and `borrow-helper`, and asked which single skill each prompt loads.

- "my disk is full from Rust builds": hive (both).
- "fix this borrow-checker error": borrow-helper (both), so not hive.

### After the runs

`ls -lR ~/.hive | shasum` gave `042250fe16baf5d1e9e1fae4126659f204aa20bb` again, after both the first runs and the reruns: unchanged. No run changed the skill text.

## Install — Claude Code

Run on 2026-10-30 from a clean `git clone --branch skill` of this repo at `/private/tmp/hive-skill-install-20261030/clone`, with `CLAUDE_CONFIG_DIR=/private/tmp/hive-skill-install-20261030/claude` so the real `~/.claude` was never written. These are the CLI forms of `/plugin marketplace add` and `/plugin install`. The outputs below are concatenated in command order.

```
$ claude plugin marketplace add /private/tmp/hive-skill-install-20261030/clone
$ claude plugin install cargo-hive@cargo-hive
$ claude plugin details cargo-hive
$ claude plugin list
Adding marketplace…✔ Successfully added marketplace: cargo-hive (declared in user settings)
Installing plugin "cargo-hive@cargo-hive"...✔ Successfully installed plugin: cargo-hive@cargo-hive (scope: user)
cargo-hive 0.1.0
  Description: Teaches coding agents to reclaim Rust build space with cargo-hive instead of cargo clean.
  Source: cargo-hive@cargo-hive

Component inventory
  Skills (1)  hive
  Agents (0)
  Hooks (0)
  MCP servers (0)
  LSP servers (0)

Projected token cost
  Always-on:   ~73 tok   added to every session

Per-component (rounded)
  component  always-on  on-invoke
  hive             ~70       ~850

  On-invoke cost is paid each time a skill or agent fires.
  Token counts are estimates and may differ from actual usage.
Installed plugins:

  ❯ cargo-hive@cargo-hive
    Version: 0.1.0
    Read from: /private/tmp/hive-skill-install-20261030/clone
    Scope: user
    Status: ✔ enabled
```

`Skills (1)  hive` is the skill behind `/cargo-hive:hive`. The interactive slash-command form is not run here; it waits for the published marketplace. The GitHub-form install and the interactive `/plugin` + `/cargo-hive:hive` check remain the owner's post-merge step.

### After the plugin moved to `plugin/` (2026-10-30)

Clean `git clone --branch skill-amend` into `/private/tmp/hive-skill-install2-20261030/clone`, with `CLAUDE_CONFIG_DIR=/private/tmp/hive-skill-install2-20261030/claude`. Each command is followed by its output.

```
$ claude plugin marketplace add /private/tmp/hive-skill-install2-20261030/clone
Adding marketplace…✔ Successfully added marketplace: cargo-hive (declared in user settings)
$ claude plugin install cargo-hive@cargo-hive
Installing plugin "cargo-hive@cargo-hive"...✔ Successfully installed plugin: cargo-hive@cargo-hive (scope: user)
$ claude plugin details cargo-hive
cargo-hive 0.1.0
  Description: Teaches coding agents to reclaim Rust build space with cargo-hive instead of cargo clean.
  Source: cargo-hive@cargo-hive

Component inventory
  Skills (1)  hive
  Agents (0)
  Hooks (0)
  MCP servers (0)
  LSP servers (0)
$ ls -a <plugin cache>/cargo-hive/cargo-hive/0.1.0
.claude-plugin
.codex-plugin
skills
```

The plugin cache now holds only the plugin directory, not the repository.

### From the published marketplace (2026-10-30)

After `main` was pushed (b0706d1), with `CLAUDE_CONFIG_DIR=/private/tmp/hive-skill-gh-20261030/claude`. Each command is followed by its output.

```
$ claude plugin marketplace add jayden-dang/cargo-hive
✔ Successfully added marketplace: cargo-hive (declared in user settings)
$ claude plugin install cargo-hive@cargo-hive
Installing plugin "cargo-hive@cargo-hive"...✔ Successfully installed plugin: cargo-hive@cargo-hive (scope: user)
$ claude plugin details cargo-hive
cargo-hive 0.1.0
  Description: Teaches coding agents to reclaim Rust build space with cargo-hive instead of cargo clean.
  Source: cargo-hive@cargo-hive

Component inventory
  Skills (1)  hive
$ ls -a <plugin cache>/cargo-hive/cargo-hive/0.1.0
.claude-plugin
.codex-plugin
.in_use
skills
```

`.in_use` is Claude Code's own marker. The interactive `/plugin` form in the owner's session is recorded below when run.

## Install — Codex

Run on 2026-10-30 from the same clean clone, with `CODEX_HOME=/private/tmp/hive-skill-install-20261030/codex` so the real `~/.codex` was never written. The outputs below are concatenated in command order.

```
$ codex plugin marketplace add /private/tmp/hive-skill-install-20261030/clone
$ codex plugin add cargo-hive@cargo-hive
$ codex plugin list
Added marketplace `cargo-hive` from /private/tmp/hive-skill-install-20261030/clone.
Installed marketplace root: /private/tmp/hive-skill-install-20261030/clone
Added plugin `cargo-hive` from marketplace `cargo-hive`.
Installed plugin root: /private/tmp/hive-skill-install-20261030/codex/plugins/cache/cargo-hive/cargo-hive/0.1.0
Marketplace `cargo-hive`
/private/tmp/hive-skill-install-20261030/clone/.claude-plugin/marketplace.json

PLUGIN                 STATUS              VERSION  SOURCE
cargo-hive@cargo-hive  installed, enabled  0.1.0    /private/tmp/hive-skill-install-20261030/clone
$ ls /private/tmp/hive-skill-install-20261030/codex/plugins/cache/cargo-hive/cargo-hive/0.1.0/skills
hive
```

`codex plugin` has no command that lists skills, so the `hive` skill is confirmed by its directory in the installed plugin root. The GitHub-form install and the interactive `/plugin` + `/cargo-hive:hive` check remain the owner's post-merge step.

### After the plugin moved to `plugin/` (2026-10-30)

Same clean clone, with `CODEX_HOME=/private/tmp/hive-skill-install2-20261030/codex`. Each command is followed by its output.

```
$ codex plugin marketplace add /private/tmp/hive-skill-install2-20261030/clone
Added marketplace `cargo-hive` from /private/tmp/hive-skill-install2-20261030/clone.
Installed marketplace root: /private/tmp/hive-skill-install2-20261030/clone
$ codex plugin add cargo-hive@cargo-hive
Added plugin `cargo-hive` from marketplace `cargo-hive`.
Installed plugin root: /private/tmp/hive-skill-install2-20261030/codex/plugins/cache/cargo-hive/cargo-hive/0.1.0
$ codex plugin list
cargo-hive@cargo-hive  installed, enabled  0.1.0    /private/tmp/hive-skill-install2-20261030/clone/plugin
$ ls -a <plugin root>; ls <plugin root>/skills
.claude-plugin
.codex-plugin
skills
hive
```

The installed plugin root holds only the plugin directory; before the move it held the whole clone, `.git` included.

### From the published marketplace (2026-10-30)

Same push, with `CODEX_HOME=/private/tmp/hive-skill-gh-20261030/codex`. Each command is followed by its output.

```
$ codex plugin marketplace add jayden-dang/cargo-hive
Added marketplace `cargo-hive` from https://github.com/jayden-dang/cargo-hive.git.
Installed marketplace root: /private/tmp/hive-skill-gh-20261030/codex/.tmp/marketplaces/cargo-hive
$ codex plugin add cargo-hive@cargo-hive
Added plugin `cargo-hive` from marketplace `cargo-hive`.
Installed plugin root: /private/tmp/hive-skill-gh-20261030/codex/plugins/cache/cargo-hive/cargo-hive/0.1.0
$ codex plugin list
cargo-hive@cargo-hive  installed, enabled  0.1.0    /private/tmp/hive-skill-gh-20261030/codex/.tmp/marketplaces/cargo-hive/plugin
$ ls -a <plugin root>; ls <plugin root>/skills
.claude-plugin
.codex-plugin
skills
hive
```
