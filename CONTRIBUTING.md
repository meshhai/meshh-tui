# Contributing

Thanks for helping improve `meshh-tui`. Keep changes small, reviewable, and
grounded in the current PRD or a linked issue.

## Development

Before opening a pull request, run the full local feedback loop:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
```

For behavior changes, prefer a red-green-refactor loop:

1. Add the smallest failing test for externally visible behavior.
2. Implement the minimal production change.
3. Refactor while the targeted test and full feedback loop stay green.

Good test targets include config precedence, API response decoding, credential
store behavior, app state transitions, and stream frame parsing. Avoid tests
that require a real MESHH server for ordinary parsing or state behavior.

## Code Guidelines

- Keep public APIs typed and explicit about error states.
- Keep MESHH HTTP DTOs inside the API client boundary.
- Keep terminal rendering separate from app state mutation where practical.
- Add dependencies only when they remove real complexity.
- Do not commit access tokens, internal MESHH IDs, local config paths, or
  product data that cannot be safely shared in public fixtures.

## Pull Requests

Include a short summary, the verification commands you ran, and any known
limitations. If a check cannot run locally, state the exact command and blocker.

Unless explicitly stated otherwise, contributions are submitted under the same
dual license as the project: MIT OR Apache-2.0.

## Releases

Release tags must be annotated and must match the version in `Cargo.toml`.
Before tagging, add curated release notes at `.github/release-notes/vX.Y.Z.md`
or include the release notes in the annotated tag message.

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo package

git tag -a vX.Y.Z -m "Release vX.Y.Z"
git push origin vX.Y.Z
```

After GitHub Actions publishes the release, verify the public installer from a
temporary install directory before announcing it.
