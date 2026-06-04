# Meshh TUI Issues

## 1. Project Foundation

Status: completed  
Type: AFK  
Blocked by: None

### What to build

Set up the Rust project as a clean open-source CLI foundation with formatting, linting, errors, module layout, and basic command parsing.

### Acceptance criteria

- [x] `meshh --help` shows top-level help and the `login` and `tui` commands.
- [x] The repo has a concise README with install, development, and configuration basics.
- [x] The codebase has clear modules for CLI, config, API, credentials, and TUI app state.
- [x] `cargo fmt`, `cargo clippy`, and `cargo test` pass.

## 2. Configuration And Credential Storage

Status: completed  
Type: AFK  
Blocked by: None

### What to build

Implement config loading for API base URL and local credential storage for the destination-scoped token.

### Acceptance criteria

- [x] API base URL precedence is CLI flag, environment variable, config file, default.
- [x] Token read/write/delete is isolated behind a small credential-store interface.
- [x] Stored token is not printed in normal logs or UI.
- [x] Unit tests cover config precedence and credential-store behavior.

## 3. Device Login Flow

Status: completed  
Type: AFK  
Blocked by: None

### What to build

Implement `meshh login` against the Meshh device authorization API.

### Acceptance criteria

- [x] `meshh login` creates a device authorization and displays the verification URL and user code.
- [x] The command polls until approved, denied, expired, interrupted, or timed out.
- [x] Approved login stores the returned bearer token.
- [x] Denied, expired, invalid, and network errors produce clear terminal messages.
- [x] Tests cover success, pending, denied, expired, and invalid-device-code responses.

## 4. Delivery API Client

Status: completed
Type: AFK  
Blocked by: None

### What to build

Implement typed API calls for delivery list, delivery detail, and stream frames.

### Acceptance criteria

- [x] List endpoint decodes public delivery items.
- [x] Detail endpoint decodes a selected public delivery item.
- [x] Stream endpoint parses server-sent delivery events and cursors.
- [x] Auth failures are represented distinctly from network and parse failures.
- [x] Tests use product-safe payload fixtures and assert no internal-ID fields are required.

## 5. TUI List And Detail Experience

Status: completed  
Type: AFK  
Blocked by: Issue 4

### What to build

Implement `meshh tui` with a list view and detail view for route deliveries.

### Acceptance criteria

- [x] Startup loads recent deliveries and renders headline rows.
- [x] Rows show headline, source context, status, and detected time when available.
- [x] Keyboard navigation supports up/down, open detail, back, refresh, and quit.
- [x] Detail view shows headline, summary/body, source URL, matched routes, and status.
- [x] Empty, loading, auth error, and network error states are visible and non-crashing.
- [x] App state transitions are unit tested without terminal IO.

## 6. Live Stream And Resume

Status: ready-for-agent  
Type: AFK  
Blocked by: Issues 4, 5

### What to build

Connect the TUI to the stream endpoint so new deliveries appear while the app is open and reconnects resume from the latest cursor.

### Acceptance criteria

- [ ] Streamed delivery events update the list without duplicating exact cursor replays.
- [ ] The client tracks the latest cursor seen in the current session.
- [ ] Reconnect uses the latest cursor as the `after` parameter.
- [ ] Stream errors move the app into a visible reconnecting/error state.
- [ ] Tests cover stream insert, exact duplicate suppression, reconnect resume, and auth failure.

## 7. Open-Source Release Hardening

Status: ready-for-agent  
Type: AFK  
Blocked by: Issues 4-6

### What to build

Prepare the repo for a first alpha release.

### Acceptance criteria

- [ ] README includes screenshots or an asciinema-style terminal example.
- [ ] License, contribution notes, and security/reporting guidance are present.
- [ ] CI runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.
- [ ] Release build works locally.
- [ ] No secrets, internal Meshh IDs, or local-only paths are committed.
