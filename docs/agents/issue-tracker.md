# Issue Tracker

Implementation planning for agent-driven work is feature-scoped local markdown, not GitHub Issues.

## Canonical Files

Each feature uses this local layout:

```text
.scratch/<feature>/
├── prd.md
├── issues.md
└── progress.txt
```

- `.scratch/<feature>/prd.md` contains product requirements, scope, user stories, and acceptance criteria.
- `.scratch/<feature>/issues.md` contains implementation slices, task status, and optional top-of-file `# Review notes`.
- `.scratch/<feature>/progress.txt` is an append-only iteration log.
- `.scratch/<feature>/PRD.md` may be accepted by local tooling as a compatibility alias for `prd.md`.

## Scratch State

- `.scratch/` is local planning scratch and should stay out of public commits by default.
- `.agents/` is local executable skill configuration and should stay out of public commits by default.
- Do not treat scratch output as durable project truth. Promote durable vocabulary to `CONTEXT.md`, durable decisions to `docs/adr/`, and public workflow policy to `docs/agents/`.

## GitHub

Use GitHub for pull requests and code review, not as the canonical implementation task queue.

- Target PRs at `main` unless the user explicitly says otherwise.
- Include the relevant local feature slug or planning summary in the PR body when helpful.
- Do not push branches unless the user asks.

## When A Skill Says "Issue Tracker"

For agent-driven implementation work, read and update `.scratch/<feature>/issues.md`. Use the sibling `prd.md` and `progress.txt` for context.

For PR publication or review coordination, use GitHub PRs.
