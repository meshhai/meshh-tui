# Domain Docs

How contributors and coding agents should consume `meshh-tui` product and architecture documentation.

## Before Exploring

Read these first when they exist:

- `AGENTS.md` for current coding-agent and repository workflow rules.
- `README.md` for public installation, usage, and development guidance.
- `CONTEXT.md` for stable Meshh TUI vocabulary and architecture rules.
- `docs/adr/` for accepted architecture decisions that touch the area being changed.
- `.scratch/<feature>/prd.md` and `.scratch/<feature>/issues.md` for local feature planning.

If a future `CONTEXT-MAP.md` exists, follow it as the source of context boundaries.

## Layout

`meshh-tui` is a Rust CLI/TUI project. Keep the layout boring and easy to navigate:

```text
/
├── Cargo.toml
├── README.md
├── docs/
│   ├── adr/
│   └── agents/
├── src/
└── tests/
```

## Use Project Vocabulary

Use the Meshh TUI terms consistently:

- device authorization
- user code
- destination-scoped token
- delivery item
- public delivery ID
- cursor
- route delivery
- stream
- resume
- list view
- detail view
- credential store

If a concept is missing from the glossary, either avoid inventing new language or call out the gap for a docs/design pass.

## Respect Boundaries

- CLI parsing owns command names, flags, and process exit behavior.
- Config owns API base URL and local settings precedence.
- Credential storage owns token persistence and redaction.
- API client code owns Meshh HTTP contracts and DTOs.
- App state owns selection, loading, errors, cursor tracking, and view mode.
- TUI rendering reads app state and emits UI events. It should not perform HTTP requests directly.
- Future agent interface work is out of scope unless explicitly requested.

## ADR Conflicts

If a proposed change contradicts an existing ADR, surface the conflict explicitly instead of silently overriding it.
