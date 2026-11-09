# Issue tracker configuration (agent-facing)

Written by `configure-repo`. Skills that read or write issues consult this file.

**Tracker:** github (`github.com/jayden-dang/cargo-hive`, through the `gh` CLI)

**PR surface:** no — external pull requests are not a request surface; `triage` works issues only.

## Publish and program sync

Filled by `configure-repo`. `plan-tasks` Step 5 and program writers read these fields.

| Field | Value |
|---|---|
| **Publish unit** | `feature` — one issue per approved plan; tasks stay in `tasks.md` |
| **Close linkage** | `Closes #N` in the feature PR body, or in the merge commit message when `land-branch` lands by `merge` |
| **Program sync** | `local` — MILE-N and ROAD-N live only in `docs/roadmap/INDEX.md`; a feature issue may cite them in its body |
| **Program write role** | `owner` |
| **Draft surface** | `repo` — milestone drafts live in `docs/roadmap/INDEX.md` only |

Slot vs CODE: one home in `plan-milestones` (**ROAD-N is a slot, not a feature**).

## Wayfinding operations

### github

- List open issues: `gh issue list --state open`
- Read issue: `gh issue view <n> --comments`
- Create issue: `gh issue create --title <t> --body-file <f> --label <l>`
- Blocking: `gh issue create --blocked-by <n>,<n>`, `gh issue edit <n> --add-blocked-by <n>`
- Sub-issues: `gh issue create --parent <n>` (not used by Publish unit `feature`, which opens one issue per plan with no task-to-task edges)
- Comment: `gh issue comment <n> --body-file <f>`
- Close: `gh issue close <n> --comment <text>`

## Issue format

Every writer follows this when creating an issue, with or without a skill loaded.

**Title:** what the reader gets, or for a bug what goes wrong, in plain words. No `Task N`, no `feat:`.

**Body of a bug:**

```markdown
## What happens
## Expected
## How to reproduce
Exact steps and a minimal reproduction, or "read from code, not reproduced".
## Evidence
Error text, versions, trace IDs.
```

**Body of anything else:**

```markdown
## Problem
Who hits what today, and why it matters now. The requester's own words stay verbatim.
## Outcome
What a person can do once this ships.
## Acceptance
- [ ] Observable, and checkable on its own
## Out of scope
- Adjacent work this must not touch, or "None"
```

In `shaping`, a section with nothing in it yet reads `Not decided yet`.

**Source paths, line numbers, function names, `Task N` items and implementation steps go in the spec.**
They stay out of the issue even when the request names them. Three of four baseline runs filed the
requester's "add a route, add a service method" as a "suggested approach". The first line of the
body is its first section: no line says who or what wrote it.

**Internal IDs follow reach.** `git check-ignore -q docs/specs/_probe/requirements.md` exiting 0
means specs are private, so no feature code, requirement ID or spec path appears anywhere on the
tracker. The same check on `docs/roadmap/INDEX.md` governs `ROAD-N` and `MILE-N`. Where specs are
shared, a plan's issue is titled `[CODE] <outcome>` and its last lines are `Requirements covered:`,
`Plan:` and `Roadmap:`. Where the roadmap is shared, `ROAD-N` leads the title of its claim. What
stays off the tracker goes in `.skills/<CODE>/tickets.md`, where `/board` and `/go` read it, one line
per ticket with the ID first: `<ticket id> covers: <requirement IDs> road: <ROAD-N>`.

**Fill all four fields, using the `Issue fields` table:**

- **Assignee:** whom the requester named, else the requester. Filing on someone's behalf still
  assigns them. Blank only when the requester said so, and say so in the reply.
- **Status:** one category role and one state role from `triage-labels.md`. The state is what the
  requester named, else `needs-triage`.
- **Priority:** the requester's word, else `## Focus` in the roadmap, else this table.
- **Estimate:** the requester's word, else this table.

| Priority | When |
|---|---|
| urgent | Production is down, data is being lost, or a security hole is open |
| high | It blocks users or other work, has a deadline this week, or is in `## Focus` |
| medium | The default, when nothing else applies |
| low | No time pressure; an improvement |

| Estimate | Complexity |
|---|---|
| 1 | One small change, approach known |
| 2 | A few places in one part of the system |
| 3 | Many places, with points to weigh |
| 5 | Several parts of the system, or open questions remain |
| 8 | Large with many unknowns; consider splitting |

**Blocking:** the tracker's relation when `Issue fields` names one, else one line in the body:
`Blocked by: #N, #M`.

**After creating:** read the issue back from the tracker. Reply with its ID, assignee, priority,
estimate and status, as the read shows them. A field that would not save: keep the issue, name the
field in the reply, and put nothing in the body in its place.

## Issue fields

Filled by `configure-repo`. `jayden-dang/cargo-hive` is a personal repo (`isInOrganization`
false), so priority and estimate are labels; all nine `priority:` and `size:` labels exist
(created 2026-11-09).

| Field | Write | Read |
|---|---|---|
| Assignee | `gh issue create --assignee <login>` (`jayden-dang`) | `gh issue view <n> --json assignees` |
| Status | `gh issue create --label <category> --label <state>` (roles from `triage-labels.md`) | `gh issue view <n> --json labels` |
| Priority | `gh issue create --label priority:<urgent\|high\|medium\|low>` | `gh issue view <n> --json labels` |
| Estimate | `gh issue create --label size:<1\|2\|3\|5\|8>` | `gh issue view <n> --json labels` |
| Blocking | `gh issue create --blocked-by <n>,<n>`, `gh issue edit <n> --add-blocked-by <n>` | `gh issue view <n> --json blockedBy` |
