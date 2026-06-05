# meshh-tui

`meshh-tui` is the Rust terminal client for MESHH route deliveries. The CLI binary is named `meshh`.

## Install

Install the latest macOS or Linux release:

```sh
curl -fsSL https://raw.githubusercontent.com/meshhai/meshh-tui/master/scripts/install.sh | sh
```

The installer downloads the matching GitHub Release archive, verifies its
`.sha256` checksum, and installs the `meshh` binary. Set `MESHH_INSTALL_DIR` to
choose the install directory, or `MESHH_VERSION=v0.1.0` to install a specific
release.

Install from source with Cargo:

```sh
cargo install --git https://github.com/meshhai/meshh-tui --bin meshh
```

Or download a release archive from GitHub Releases. Replace `0.1.0` with the
version you want to install:

```sh
# macOS arm64 example
curl -L https://github.com/meshhai/meshh-tui/releases/download/v0.1.0/meshh_0.1.0_aarch64-apple-darwin.tar.gz -o meshh.tar.gz
tar -xzf meshh.tar.gz
install -m 0755 meshh_0.1.0_aarch64-apple-darwin/meshh /usr/local/bin/meshh
```

For local development builds:

```sh
cargo install --path . --bin meshh
meshh --help
```

Release archives are named by version and target triple:

```text
meshh_0.1.0_aarch64-apple-darwin.tar.gz
meshh_0.1.0_x86_64-apple-darwin.tar.gz
meshh_0.1.0_x86_64-unknown-linux-gnu.tar.gz
meshh_0.1.0_x86_64-pc-windows-msvc.zip
```

Each archive is published with a matching `.sha256` checksum file.

## Commands

The first release is planned around two commands:

- `meshh login` authenticates the terminal with MESHH.
- `meshh tui` opens the route delivery terminal UI.

`meshh login` starts MESHH device authorization, prints the browser verification URL and user code, polls for approval, and stores the returned destination-scoped bearer token. The token is not printed.

`meshh tui` loads the stored token, fetches recent destination deliveries, and opens a dense route delivery view with published time, source, headline, and status columns. It keeps the delivery stream connected in the background, inserts new route deliveries at the top of the list, and reconnects with the latest stream cursor after network interruptions. Select rows with Up/Down or `j`/`k`, open detail with Enter or `o`, refresh with `r`, go back with `b`/Esc, and quit with `q` or Ctrl-C. Detail view shows the headline, status, published time, source context, source URL, matched routes, and summary/body text when provided. When older MESHH payloads do not include `published_at`, the TUI falls back to the source `detected_at` timestamp.

## Terminal Example

The example below uses sample delivery data.

```text
$ meshh login
Approve this terminal in MESHH:
Verification URL: https://meshh.example/device
User code: ABCD-EFGH
Waiting for approval...
Login approved. Token stored.

$ meshh tui
meshh-tui v0.1.0 | live | 3 rows | row 1/3
+------------------------------------------------------------------------------+
|Published    Source             Headline                              Status    |
|> Jun 04 19:09 FED            FOMC statement shifts risk balance    delivered |
|  Jun 04 18:58 RBA            Minutes point to inflation caution    delivered |
|  Jun 04 18:42 BOJ            Market operation notice published     delivered |
+------------------------------------------------------------------------------+
up/down select | enter open | r refresh | q quit

Enter

+------------------------------------------------------------------------------+
|FOMC statement shifts risk balance                                            |
|                                                                              |
|Status: delivered                                                             |
|Published: Jun 04 19:09                                                       |
|Source: FED                                                                   |
|Source URL: https://www.federalreserve.gov/monetarypolicy                     |
|Matched routes: Central Bank Watch                                            |
|                                                                              |
|A central bank route matched this delivery for market review.                 |
+------------------------------------------------------------------------------+
b back | r reload detail | q quit
```

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

MESHH stores local config under the platform config directory, such as
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
