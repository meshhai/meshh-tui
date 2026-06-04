use std::{error::Error, fmt, io, time::Duration};

use crossterm::{
    cursor::Show,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap},
};

use crate::{
    api::{
        ApiClient, ApiClientConfig, ApiError, DeliveryApi, DeliveryDetail, DeliveryListItem,
        DeliveryListPage, PublicDeliveryId, StreamCursor,
    },
    config::RuntimeConfig,
    credentials::{BearerToken, CredentialError, CredentialStore, FileCredentialStore},
};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// High-level screen currently shown by the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    DeliveryList,
    DeliveryDetail {
        public_delivery_id: PublicDeliveryId,
    },
}

/// List feed state visible to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedStatus {
    Loading,
    Ready,
    Empty,
    Error(AppError),
}

/// Detail panel state visible to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailStatus {
    Hidden,
    Loading,
    Ready,
    Error(AppError),
}

/// User-visible error category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppErrorKind {
    Authentication,
    Network,
    Api,
}

/// User-visible error message retained in app state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppError {
    kind: AppErrorKind,
    message: String,
}

impl AppError {
    /// Creates a user-visible error.
    pub fn new(kind: AppErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Converts an API error into a visible TUI error.
    pub fn from_api_error(error: &ApiError) -> Self {
        let kind = match error {
            ApiError::Authentication { .. } => AppErrorKind::Authentication,
            ApiError::Transport { .. } => AppErrorKind::Network,
            ApiError::HttpStatus { .. } | ApiError::InvalidResponse { .. } => AppErrorKind::Api,
        };

        Self::new(kind, error.to_string())
    }

    /// Returns the error category.
    pub fn kind(&self) -> AppErrorKind {
        self.kind
    }

    /// Returns the displayable error message.
    pub fn message(&self) -> &str {
        &self.message
    }

    fn heading(&self) -> &'static str {
        match self.kind {
            AppErrorKind::Authentication => "Authentication error",
            AppErrorKind::Network => "Network error",
            AppErrorKind::Api => "API error",
        }
    }
}

/// Keyboard action understood by the app state reducer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Up,
    Down,
    Open,
    Back,
    Refresh,
    Quit,
}

/// Side effect requested by the app state reducer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppCommand {
    None,
    LoadDeliveries,
    LoadDetail(PublicDeliveryId),
    Quit,
}

/// Testable application state for the terminal UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    screen: Screen,
    feed_status: FeedStatus,
    deliveries: Vec<DeliveryListItem>,
    selected_index: Option<usize>,
    next_cursor: Option<StreamCursor>,
    detail_status: DetailStatus,
    detail: Option<DeliveryDetail>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            screen: Screen::DeliveryList,
            feed_status: FeedStatus::Loading,
            deliveries: Vec::new(),
            selected_index: None,
            next_cursor: None,
            detail_status: DetailStatus::Hidden,
            detail: None,
        }
    }
}

impl AppState {
    /// Returns the active screen.
    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    /// Returns the current list feed status.
    pub fn feed_status(&self) -> &FeedStatus {
        &self.feed_status
    }

    /// Returns the visible detail status.
    pub fn detail_status(&self) -> &DetailStatus {
        &self.detail_status
    }

    /// Returns the loaded delivery rows.
    pub fn deliveries(&self) -> &[DeliveryListItem] {
        &self.deliveries
    }

    /// Returns the selected row index.
    pub fn selected_index(&self) -> Option<usize> {
        self.selected_index
    }

    /// Returns the selected row, when one is available.
    pub fn selected_delivery(&self) -> Option<&DeliveryListItem> {
        self.selected_index
            .and_then(|index| self.deliveries.get(index))
    }

    /// Returns the next history cursor from the last list response.
    pub fn next_cursor(&self) -> Option<&StreamCursor> {
        self.next_cursor.as_ref()
    }

    /// Returns the loaded detail record.
    pub fn detail(&self) -> Option<&DeliveryDetail> {
        self.detail.as_ref()
    }

    /// Moves the list feed into a loading state.
    pub fn start_loading(&mut self) {
        self.screen = Screen::DeliveryList;
        self.feed_status = FeedStatus::Loading;
        self.detail_status = DetailStatus::Hidden;
        self.detail = None;
    }

    /// Applies a successful delivery list response.
    pub fn receive_delivery_page(&mut self, page: DeliveryListPage) {
        self.deliveries = page.items().to_vec();
        self.selected_index = if self.deliveries.is_empty() {
            None
        } else {
            Some(0)
        };
        self.next_cursor = page.next_cursor().cloned();
        self.feed_status = if self.deliveries.is_empty() {
            FeedStatus::Empty
        } else {
            FeedStatus::Ready
        };
    }

    /// Applies a failed delivery list response without panicking or printing secrets.
    pub fn receive_list_error(&mut self, error: &ApiError) {
        self.feed_status = FeedStatus::Error(AppError::from_api_error(error));
        self.clamp_selection();
    }

    /// Moves selection one row up.
    pub fn select_previous(&mut self) {
        if let Some(index) = self.selected_index {
            self.selected_index = Some(index.saturating_sub(1));
        }
    }

    /// Moves selection one row down.
    pub fn select_next(&mut self) {
        if let Some(index) = self.selected_index {
            self.selected_index = Some((index + 1).min(self.deliveries.len().saturating_sub(1)));
        }
    }

    /// Opens the selected delivery detail screen and returns the ID to load.
    pub fn open_selected_detail(&mut self) -> Option<PublicDeliveryId> {
        let public_delivery_id = self.selected_delivery()?.public_delivery_id().clone();

        self.screen = Screen::DeliveryDetail {
            public_delivery_id: public_delivery_id.clone(),
        };
        self.detail_status = DetailStatus::Loading;
        self.detail = None;

        Some(public_delivery_id)
    }

    /// Applies a successful detail response.
    pub fn receive_detail(&mut self, detail: DeliveryDetail) {
        let public_delivery_id = detail.public_delivery_id().clone();

        self.screen = Screen::DeliveryDetail { public_delivery_id };
        self.detail_status = DetailStatus::Ready;
        self.detail = Some(detail);
    }

    /// Applies a failed detail response while keeping the detail screen visible.
    pub fn receive_detail_error(&mut self, error: &ApiError) {
        self.detail_status = DetailStatus::Error(AppError::from_api_error(error));
    }

    /// Returns from detail to the delivery list.
    pub fn close_detail(&mut self) {
        self.screen = Screen::DeliveryList;
        self.detail_status = DetailStatus::Hidden;
        self.detail = None;
    }

    /// Applies a keyboard action and returns any side effect the caller must perform.
    pub fn handle_key_action(&mut self, action: KeyAction) -> AppCommand {
        match action {
            KeyAction::Quit => AppCommand::Quit,
            KeyAction::Up => {
                if matches!(self.screen, Screen::DeliveryList) {
                    self.select_previous();
                }

                AppCommand::None
            }
            KeyAction::Down => {
                if matches!(self.screen, Screen::DeliveryList) {
                    self.select_next();
                }

                AppCommand::None
            }
            KeyAction::Open => {
                if matches!(self.screen, Screen::DeliveryList) {
                    self.open_selected_detail()
                        .map(AppCommand::LoadDetail)
                        .unwrap_or(AppCommand::None)
                } else {
                    AppCommand::None
                }
            }
            KeyAction::Back => {
                if matches!(self.screen, Screen::DeliveryDetail { .. }) {
                    self.close_detail();
                }

                AppCommand::None
            }
            KeyAction::Refresh => match self.screen() {
                Screen::DeliveryList => {
                    self.start_loading();
                    AppCommand::LoadDeliveries
                }
                Screen::DeliveryDetail { public_delivery_id } => {
                    let public_delivery_id = public_delivery_id.clone();
                    self.detail_status = DetailStatus::Loading;
                    self.detail = None;
                    AppCommand::LoadDetail(public_delivery_id)
                }
            },
        }
    }

    fn clamp_selection(&mut self) {
        self.selected_index = match (self.selected_index, self.deliveries.len()) {
            (_, 0) => None,
            (Some(index), len) if index >= len => Some(len - 1),
            (index, _) => index,
        };
    }
}

/// Runs `meshh tui` using the default API client and credential store.
pub async fn run(config: &RuntimeConfig) -> Result<(), TuiError> {
    let api = ApiClient::new(ApiClientConfig::from_runtime(config));
    let credentials =
        FileCredentialStore::new_default().map_err(|source| TuiError::Credential { source })?;

    run_with_dependencies(&api, &credentials).await
}

/// Runs the delivery TUI with injectable dependencies.
pub async fn run_with_dependencies<A, C>(api: &A, credentials: &C) -> Result<(), TuiError>
where
    A: DeliveryApi,
    C: CredentialStore,
{
    let token = credentials
        .load_token()
        .map_err(|source| TuiError::Credential { source })?
        .ok_or(TuiError::MissingToken)?;

    run_authenticated(api, &token).await
}

async fn run_authenticated<A>(api: &A, token: &BearerToken) -> Result<(), TuiError>
where
    A: DeliveryApi,
{
    let _guard = TerminalModeGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))
        .map_err(|source| TuiError::Terminal { source })?;
    let mut state = AppState::default();

    draw_state(&mut terminal, &state)?;
    load_deliveries(&mut state, api, token).await;
    run_event_loop(&mut terminal, &mut state, api, token).await
}

async fn run_event_loop<B, A>(
    terminal: &mut Terminal<B>,
    state: &mut AppState,
    api: &A,
    token: &BearerToken,
) -> Result<(), TuiError>
where
    B: Backend<Error = io::Error>,
    A: DeliveryApi,
{
    loop {
        draw_state(terminal, state)?;

        if !event::poll(EVENT_POLL_INTERVAL).map_err(|source| TuiError::Terminal { source })? {
            continue;
        }

        let Event::Key(key) = event::read().map_err(|source| TuiError::Terminal { source })? else {
            continue;
        };
        let Some(action) = key_action(key) else {
            continue;
        };

        match state.handle_key_action(action) {
            AppCommand::None => {}
            AppCommand::Quit => return Ok(()),
            AppCommand::LoadDeliveries => {
                draw_state(terminal, state)?;
                load_deliveries(state, api, token).await;
            }
            AppCommand::LoadDetail(public_delivery_id) => {
                draw_state(terminal, state)?;
                load_detail(state, api, token, &public_delivery_id).await;
            }
        }
    }
}

fn draw_state<B>(terminal: &mut Terminal<B>, state: &AppState) -> Result<(), TuiError>
where
    B: Backend<Error = io::Error>,
{
    terminal
        .draw(|frame| render(frame, state))
        .map(|_| ())
        .map_err(|source| TuiError::Terminal { source })
}

async fn load_deliveries<A>(state: &mut AppState, api: &A, token: &BearerToken)
where
    A: DeliveryApi,
{
    state.start_loading();

    match api.list_deliveries(token).await {
        Ok(page) => state.receive_delivery_page(page),
        Err(error) => state.receive_list_error(&error),
    }
}

async fn load_detail<A>(
    state: &mut AppState,
    api: &A,
    token: &BearerToken,
    public_delivery_id: &PublicDeliveryId,
) where
    A: DeliveryApi,
{
    match api.get_delivery(token, public_delivery_id).await {
        Ok(detail) => state.receive_detail(detail),
        Err(error) => state.receive_detail_error(&error),
    }
}

struct TerminalModeGuard;

impl TerminalModeGuard {
    fn enter() -> Result<Self, TuiError> {
        enable_raw_mode().map_err(|source| TuiError::Terminal { source })?;

        let mut stdout = io::stdout();
        if let Err(source) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(TuiError::Terminal { source });
        }

        Ok(Self)
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen, Show);
    }
}

fn key_action(key: KeyEvent) -> Option<KeyAction> {
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(KeyAction::Quit)
        }
        KeyCode::Char('q') => Some(KeyAction::Quit),
        KeyCode::Up | KeyCode::Char('k') => Some(KeyAction::Up),
        KeyCode::Down | KeyCode::Char('j') => Some(KeyAction::Down),
        KeyCode::Enter | KeyCode::Char('o') => Some(KeyAction::Open),
        KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('b') => Some(KeyAction::Back),
        KeyCode::Char('r') => Some(KeyAction::Refresh),
        _ => None,
    }
}

/// Renders the TUI from immutable app state.
pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(frame.area());

    render_header(frame, layout[0], state);

    match state.screen() {
        Screen::DeliveryList => render_list(frame, layout[1], state),
        Screen::DeliveryDetail { .. } => render_detail(frame, layout[1], state),
    }

    render_footer(frame, layout[2], state);
}

fn render_header(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &AppState) {
    let status = match state.feed_status() {
        FeedStatus::Loading => "loading",
        FeedStatus::Ready => "live",
        FeedStatus::Empty => "empty",
        FeedStatus::Error(error) => error.heading(),
    };
    let selected = state
        .selected_index()
        .map(|index| format!("row {}", index + 1))
        .unwrap_or_else(|| "no row".to_owned());
    let cursor = state
        .next_cursor()
        .map(|cursor| cursor.as_str().to_owned())
        .unwrap_or_else(|| "-".to_owned());
    let line = Line::from(vec![
        Span::styled(
            "MESHH ROUTE FEED",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::raw(format!("status: {status}")),
        Span::raw("  "),
        Span::raw(format!("selected: {selected}")),
        Span::raw("  "),
        Span::raw(format!("cursor: {cursor}")),
    ]);

    frame.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn render_list(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &AppState) {
    if state.deliveries().is_empty() {
        let message = match state.feed_status() {
            FeedStatus::Loading => "Loading route deliveries...",
            FeedStatus::Empty => "No route deliveries yet.",
            FeedStatus::Error(error) => error.message(),
            FeedStatus::Ready => "No route deliveries yet.",
        };

        frame.render_widget(
            Paragraph::new(message)
                .block(Block::default().borders(Borders::ALL).title("Deliveries"))
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let rows = state.deliveries().iter().enumerate().map(|(index, item)| {
        let style = if state.selected_index() == Some(index) {
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        Row::new(vec![
            Cell::from(item.headline().to_owned()),
            Cell::from(item.source_context().unwrap_or("-").to_owned()),
            Cell::from(item.status().as_str().to_owned()),
            Cell::from(item.detected_at().unwrap_or("-").to_owned()),
        ])
        .style(style)
    });
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(44),
            Constraint::Percentage(22),
            Constraint::Percentage(14),
            Constraint::Percentage(20),
        ],
    )
    .header(
        Row::new(vec!["Headline", "Source", "Status", "Detected"]).style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(Block::default().borders(Borders::ALL).title("Deliveries"))
    .column_spacing(1);

    frame.render_widget(table, area);
}

fn render_detail(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &AppState) {
    let text = match state.detail_status() {
        DetailStatus::Hidden => Text::from("No delivery selected."),
        DetailStatus::Loading => Text::from("Loading delivery detail..."),
        DetailStatus::Error(error) => Text::from(vec![
            Line::from(error.heading()),
            Line::from(""),
            Line::from(error.message().to_owned()),
        ]),
        DetailStatus::Ready => detail_text(state.detail()),
    };

    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Delivery Detail"),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn detail_text(detail: Option<&DeliveryDetail>) -> Text<'static> {
    let Some(detail) = detail else {
        return Text::from("No delivery selected.");
    };
    let routes = if detail.matched_routes().is_empty() {
        "-".to_owned()
    } else {
        detail
            .matched_routes()
            .iter()
            .map(|route| route.name())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let summary_or_body = detail
        .summary()
        .or_else(|| detail.body())
        .unwrap_or("No summary provided.");

    Text::from(vec![
        Line::from(Span::styled(
            detail.headline().to_owned(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("Status: {}", detail.status().as_str())),
        Line::from(format!("Detected: {}", detail.detected_at().unwrap_or("-"))),
        Line::from(format!(
            "Source: {}",
            detail.source_context().unwrap_or("-")
        )),
        Line::from(format!(
            "Source URL: {}",
            detail.source_url().unwrap_or("-")
        )),
        Line::from(format!("Matched routes: {routes}")),
        Line::from(""),
        Line::from(summary_or_body.to_owned()),
    ])
}

fn render_footer(frame: &mut Frame<'_>, area: ratatui::layout::Rect, state: &AppState) {
    let message = match state.screen() {
        Screen::DeliveryList => "up/down select | enter open | r refresh | q quit",
        Screen::DeliveryDetail { .. } => "b back | r reload detail | q quit",
    };

    frame.render_widget(Paragraph::new(message), area);
}

/// Errors produced before or during the terminal UI session.
#[derive(Debug)]
pub enum TuiError {
    Credential { source: CredentialError },
    MissingToken,
    Terminal { source: io::Error },
}

impl fmt::Display for TuiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credential { .. } => formatter.write_str("could not load Meshh credentials"),
            Self::MissingToken => {
                formatter.write_str("no Meshh token found; run `meshh login` first")
            }
            Self::Terminal { .. } => formatter.write_str("terminal UI failed"),
        }
    }
}

impl Error for TuiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Credential { source } => Some(source),
            Self::Terminal { source } => Some(source),
            Self::MissingToken => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AppCommand, AppErrorKind, AppState, DetailStatus, FeedStatus, KeyAction, Screen};
    use crate::api::{
        ApiError, DeliveryDetail, DeliveryListItem, DeliveryListPage, DeliveryStatus, MatchedRoute,
        PublicDeliveryId, StreamCursor, TransportError,
    };
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn loading_deliveries_populates_rows_and_selects_the_first_item() {
        let mut state = AppState::default();

        state.start_loading();
        assert_eq!(state.feed_status(), &FeedStatus::Loading);

        state.receive_delivery_page(DeliveryListPage::new(
            vec![
                delivery("del_pub_01", "CPU alert routed to ops", Some("Datadog")),
                delivery("del_pub_02", "Deploy complete", None),
            ],
            Some(StreamCursor::new("cur_next").unwrap()),
        ));

        assert_eq!(state.feed_status(), &FeedStatus::Ready);
        assert_eq!(state.deliveries().len(), 2);
        assert_eq!(state.deliveries()[0].headline(), "CPU alert routed to ops");
        assert_eq!(state.deliveries()[0].source_context(), Some("Datadog"));
        assert_eq!(state.selected_index(), Some(0));
        assert_eq!(
            state.next_cursor().map(StreamCursor::as_str),
            Some("cur_next")
        );
    }

    #[test]
    fn keyboard_navigation_opens_loads_and_closes_detail() {
        let mut state = AppState::default();
        state.receive_delivery_page(DeliveryListPage::new(
            vec![
                delivery("del_pub_01", "CPU alert routed to ops", Some("Datadog")),
                delivery("del_pub_02", "Deploy complete", Some("GitHub")),
            ],
            None,
        ));

        state.handle_key_action(KeyAction::Down);
        assert_eq!(state.selected_index(), Some(1));

        let command = state.handle_key_action(KeyAction::Open);
        assert_eq!(
            command,
            AppCommand::LoadDetail(PublicDeliveryId::new("del_pub_02").unwrap())
        );
        assert_eq!(state.detail_status(), &DetailStatus::Loading);
        assert!(matches!(
            state.screen(),
            Screen::DeliveryDetail { public_delivery_id }
                if public_delivery_id.as_str() == "del_pub_02"
        ));

        state.receive_detail(detail("del_pub_02", "Deploy complete"));

        assert_eq!(state.detail_status(), &DetailStatus::Ready);
        assert_eq!(state.detail().unwrap().headline(), "Deploy complete");
        assert_eq!(
            state.detail().unwrap().source_url(),
            Some("https://alerts.example/del_pub_02")
        );
        assert_eq!(
            state.detail().unwrap().matched_routes()[0].name(),
            "Ops Escalation"
        );

        assert_eq!(state.handle_key_action(KeyAction::Back), AppCommand::None);
        assert_eq!(state.screen(), &Screen::DeliveryList);
        assert_eq!(state.detail_status(), &DetailStatus::Hidden);
        assert!(state.detail().is_none());
    }

    #[test]
    fn keyboard_refresh_and_quit_commands_are_explicit() {
        let mut state = AppState::default();
        state.receive_delivery_page(DeliveryListPage::new(
            vec![delivery("del_pub_01", "CPU alert routed to ops", None)],
            None,
        ));

        assert_eq!(
            state.handle_key_action(KeyAction::Refresh),
            AppCommand::LoadDeliveries
        );
        assert_eq!(state.feed_status(), &FeedStatus::Loading);
        assert_eq!(state.handle_key_action(KeyAction::Quit), AppCommand::Quit);
    }

    #[test]
    fn empty_auth_and_network_states_are_visible_without_terminal_io() {
        let mut state = AppState::default();

        state.receive_delivery_page(DeliveryListPage::new(Vec::new(), None));
        assert_eq!(state.feed_status(), &FeedStatus::Empty);
        assert_eq!(state.selected_index(), None);

        state.receive_list_error(&ApiError::Authentication {
            operation: "listing deliveries",
            status: 401,
            body: "token_expired".to_owned(),
        });

        let FeedStatus::Error(error) = state.feed_status() else {
            panic!("expected auth list error");
        };
        assert_eq!(error.kind(), AppErrorKind::Authentication);
        assert!(error.message().contains("stored token"));

        state.receive_delivery_page(DeliveryListPage::new(
            vec![delivery("del_pub_01", "CPU alert routed to ops", None)],
            None,
        ));
        state.open_selected_detail().unwrap();
        state.receive_detail_error(&ApiError::Transport {
            source: TransportError::new("connection refused"),
        });

        let DetailStatus::Error(error) = state.detail_status() else {
            panic!("expected network detail error");
        };
        assert_eq!(error.kind(), AppErrorKind::Network);
        assert!(error.message().contains("network error"));
    }

    #[test]
    fn render_outputs_list_columns_and_detail_fields() {
        let mut state = AppState::default();
        state.receive_delivery_page(DeliveryListPage::new(
            vec![delivery(
                "del_pub_01",
                "CPU alert routed to ops",
                Some("Datadog"),
            )],
            Some(StreamCursor::new("cur_next").unwrap()),
        ));

        let list_text = render_text(&state);
        assert!(list_text.contains("CPU alert routed to ops"));
        assert!(list_text.contains("Datadog"));
        assert!(list_text.contains("delivered"));
        assert!(list_text.contains("2026-06-04T02:03:04Z"));

        state.open_selected_detail().unwrap();
        state.receive_detail(detail("del_pub_01", "CPU alert routed to ops"));

        let detail_text = render_text(&state);
        assert!(detail_text.contains("CPU alert routed to ops"));
        assert!(detail_text.contains("A production route matched this delivery."));
        assert!(detail_text.contains("https://alerts.example/del_pub_01"));
        assert!(detail_text.contains("Ops Escalation"));
    }

    fn delivery(
        public_delivery_id: &str,
        headline: &str,
        source_context: Option<&str>,
    ) -> DeliveryListItem {
        DeliveryListItem::new(
            PublicDeliveryId::new(public_delivery_id).unwrap(),
            headline,
            source_context.map(str::to_owned),
            DeliveryStatus::new("delivered").unwrap(),
            Some("2026-06-04T02:03:04Z".to_owned()),
            None,
        )
        .unwrap()
    }

    fn detail(public_delivery_id: &str, headline: &str) -> DeliveryDetail {
        DeliveryDetail::new(
            PublicDeliveryId::new(public_delivery_id).unwrap(),
            headline,
            Some("A production route matched this delivery.".to_owned()),
            Some("Full routed notification body.".to_owned()),
            Some(format!("https://alerts.example/{public_delivery_id}")),
            Some("GitHub".to_owned()),
            DeliveryStatus::new("delivered").unwrap(),
            Some("2026-06-04T02:03:04Z".to_owned()),
            vec![MatchedRoute::new("Ops Escalation").unwrap()],
            None,
        )
        .unwrap()
    }

    fn render_text(state: &AppState) -> String {
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| super::render(frame, state)).unwrap();

        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }
}
