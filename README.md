# meshh-tui

`meshh-tui` is the Rust terminal client for Meshh route deliveries. The CLI binary is named `meshh`.

## Install

Build the current development binary with Cargo:

```sh
cargo build
```

Run the top-level help:

```sh
cargo run -- --help
```

## Commands

The first release is planned around two commands:

- `meshh login` authenticates the terminal with Meshh.
- `meshh tui` opens the route delivery terminal UI.

Both commands are present in the CLI foundation. Their API behavior is implemented in later slices.

## Configuration

The API base URL can be passed per command with `--api-base-url` or through `MESHH_API_BASE_URL`.
Full config-file loading and credential storage are implemented in later slices.

## Development

Run the local feedback loop before opening a pull request:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
```

The crate is split into modules for CLI parsing, configuration, API client setup, credential storage, and TUI app state so each slice can be tested without terminal IO where possible.
