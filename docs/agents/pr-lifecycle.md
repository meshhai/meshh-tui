# PR Lifecycle

How contributors and coding agents should plan, publish, and verify `meshh-tui` work.

## Branches

- Work in a short-lived feature or fix branch.
- Branch from `main` unless the user explicitly asks for another base.
- Target PRs at `main` unless the user explicitly says otherwise.
- Do not commit directly to `main` unless the user explicitly asks for that exact action.
- Do not amend pushed commits unless the user explicitly asks.
- Push only when the user asks.

## Planning

- Keep private local notes out of public commits.
- Put reviewable implementation context in the PR body.
- Promote durable product language to `README.md` or `CONTEXT.md`.
- Promote durable architecture decisions to `docs/adr/`.
- Promote public workflow policy to `docs/agents/`.

## Implementation PRs

- Keep PRs narrow and independently reviewable.
- Prefer vertical slices that are demoable or verifiable on their own.
- Reference relevant ADRs or planning summaries in the PR body when useful.
- Do not invent new domain language silently. Update `CONTEXT.md` or call out the vocabulary gap.
- Keep open-source quality in mind: no secrets, no local-only paths, no unexplained dependencies.

## Verification

Run the narrow relevant check first, then broaden.

Recommended full local gate:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

For dependency changes, also inspect:

```sh
cargo tree
```

If verification cannot run because dependencies, network, credentials, a live Meshh server, or terminal capabilities are unavailable, state the exact command attempted and the blocker.

## Releases

- Cut release tags only from integrated `main`.
- Do not tag feature-branch commits for public releases.
- Before tagging, verify the target commit contains the expected release ancestry.
