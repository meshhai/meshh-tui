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

`meshh login` starts Meshh device authorization, prints the browser verification URL and user code, polls for approval, and stores the returned destination-scoped bearer token. The token is not printed.

`meshh tui` loads the stored token, fetches recent destination deliveries, and opens a dense route-feed view with headline, source, status, and detected-time columns. It keeps the delivery stream connected in the background, inserts new route deliveries at the top of the list, and reconnects with the latest stream cursor after network interruptions. Select rows with Up/Down or `j`/`k`, open detail with Enter or `o`, refresh with `r`, go back with `b`/Esc, and quit with `q` or Ctrl-C. Detail view shows the headline, status, detected time, source context, source URL, matched routes, and summary/body text when provided.

## Configuration

The API base URL is resolved in this order:

1. `--api-base-url`
2. `MESHH_API_BASE_URL`
3. The platform config file
4. `https://api.meshh.ai`

The config file is JSON and supports `api_base_url`:

```json
{
  "api_base_url": "https://api.meshh.ai"
}
```

Meshh stores local config under the platform config directory, such as
`~/Library/Application Support/meshh/config.json` on macOS,
`~/.config/meshh/config.json` on Linux, and `%APPDATA%\meshh\config.json`
on Windows.

Destination-scoped bearer tokens are isolated behind the `CredentialStore`
trait. The current fallback store writes `credentials.json` under the same
config directory with owner-only file permissions on Unix platforms.

## Development

Run the local feedback loop before opening a pull request:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
```

The crate is split into modules for CLI parsing, configuration, API client setup, credential storage, and TUI app state so each slice can be tested without terminal IO where possible.

## Project Docs

- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)
- License: MIT OR Apache-2.0; see [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
