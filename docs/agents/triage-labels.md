# Triage Labels

Agent workflows speak in terms of five canonical triage roles. In this repository, these are status strings in `.scratch/<feature>/issues.md`, not GitHub labels.

| Canonical role | Status in `.scratch/<feature>/issues.md` | Meaning |
| --- | --- | --- |
| `needs-triage` | `needs-triage` | Maintainer needs to evaluate this issue |
| `needs-info` | `needs-info` | Waiting on reporter or user clarification |
| `ready-for-agent` | `ready-for-agent` | Fully specified and ready for an agent to implement |
| `ready-for-human` | `ready-for-human` | Requires human implementation or judgment |
| `wontfix` | `wontfix` | Will not be actioned |

When a skill mentions a triage role, use the corresponding status string from this table.

If GitHub Issues are introduced later, map these canonical roles to real GitHub labels instead of inventing new names.
