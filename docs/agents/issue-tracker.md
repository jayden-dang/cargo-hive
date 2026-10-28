# Issue tracker configuration (agent-facing)

Written by `configure-repo`. Skills that read or write issues consult this file.

**Tracker:** github (`github.com/jayden-dang/cargo-hive`, through the `gh` CLI)

**PR surface:** no — external pull requests are not a request surface; `triage` works issues only.

## Publish and program sync

Filled by `configure-repo`. `plan-tasks` Step 5 and program writers read these fields.

| Field | Value |
|---|---|
| **Publish unit** | `feature` — one issue per approved plan; tasks stay in `tasks.md` |
| **Close linkage** | `Closes #N` in the feature PR body |
| **Program sync** | `local` — MILE-N and ROAD-N live only in `docs/roadmap/INDEX.md`; a feature issue may cite them in its body |
| **Program write role** | `owner` |
| **Draft surface** | `repo` — milestone drafts live in `docs/roadmap/INDEX.md` only |

Slot vs CODE: one home in `plan-milestones` (**ROAD-N is a slot, not a feature**).

## Wayfinding operations

### github

- List open issues: `gh issue list --state open`
- Read issue: `gh issue view <n> --comments`
- Create issue: `gh issue create --title <t> --body-file <f> --label <l>`
- Sub-issues / blocking: not used — Publish unit `feature` opens one issue per plan, with no task-to-task edges
- Comment: `gh issue comment <n> --body-file <f>`
