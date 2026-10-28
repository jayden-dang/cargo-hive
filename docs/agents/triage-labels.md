# Triage label mapping (agent-facing)

Written by `configure-repo`. Maps canonical triage roles to this repo's actual labels.
All eleven labels exist on `github.com/jayden-dang/cargo-hive` (created 2026-10-28 where missing).

| Canonical role | Label in this repo |
|---|---|
| needs-triage | `needs-triage` |
| needs-info | `needs-info` |
| ready-for-agent | `ready-for-agent` |
| ready-for-human | `ready-for-human` |
| shaping | `status:shaping` |
| building | `status:building` |
| in-review | `status:in-review` |
| shipped | `status:shipped` |
| wontfix | `wontfix` |
| bug | `bug` |
| enhancement | `enhancement` |

Rule: every triaged issue carries exactly one category role (bug/enhancement)
and one state role. A skill that moves an issue to a new state removes the old state
role in the same edit. `shipped` marks a closed issue whose change is in a release.

The repo's other labels (`documentation`, `duplicate`, `invalid`, `question`,
`good first issue`, `help wanted`, `accessibility`) are not triage roles; skills leave them alone.
