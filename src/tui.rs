use std::{collections::HashSet, error::Error, fmt, io, sync::mpsc, time::Duration};

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
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap},
};

use crate::{
    api::{
        ApiClient, ApiClientConfig, ApiError, DeliveryApi, DeliveryDetail, DeliveryListItem,
        DeliveryListPage, DeliveryStreamFrame, PublicDeliveryId, StreamCursor,
    },
    config::RuntimeConfig,
    credentials::{BearerToken, CredentialError, CredentialStore, FileCredentialStore},
};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const STREAM_RECONNECT_INTERVAL: Duration = Duration::from_secs(1);

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

/// Live delivery stream state visible to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamStatus {
    Disconnected,
    Connecting,
    Live,
    Reconnecting(AppError),
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
    stream_status: StreamStatus,
    deliveries: Vec<DeliveryListItem>,
    selected_index: Option<usize>,
    next_cursor: Option<StreamCursor>,
    stream_resume_cursor: Option<StreamCursor>,
    seen_stream_cursors: HashSet<StreamCursor>,
    detail_status: DetailStatus,
    detail: Option<DeliveryDetail>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            screen: Screen::DeliveryList,
            feed_status: FeedStatus::Loading,
            stream_status: StreamStatus::Disconnected,
            deliveries: Vec::new(),
            selected_index: None,
            next_cursor: None,
            stream_resume_cursor: None,
            seen_stream_cursors: HashSet::new(),
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

    /// Returns the live delivery stream status.
    pub fn stream_status(&self) -> &StreamStatus {
        &self.stream_status
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

    /// Returns the cursor that should be sent as the next stream `after` value.
    pub fn stream_resume_cursor(&self) -> Option<&StreamCursor> {
        self.stream_resume_cursor.as_ref()
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
        self.track_page_cursors();
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

    /// Moves the stream into a connecting state.
    pub fn start_stream(&mut self) {
        self.stream_status = StreamStatus::Connecting;
    }

    /// Applies stream frames, prepending new deliveries and suppressing cursor replays.
    pub fn receive_stream_frames(&mut self, frames: Vec<DeliveryStreamFrame>) {
        for frame in frames {
            match frame {
                DeliveryStreamFrame::Connected => {}
                DeliveryStreamFrame::Cursor(cursor) => self.note_stream_cursor(cursor),
                DeliveryStreamFrame::Delivery { cursor, item } => {
                    let cursor = cursor.or_else(|| item.cursor().cloned());
                    if let Some(cursor) = cursor {
                        if !self.seen_stream_cursors.insert(cursor.clone()) {
                            continue;
                        }

                        self.stream_resume_cursor = Some(cursor);
                    }

                    self.insert_or_replace_stream_delivery(item);
                }
            }
        }

        self.feed_status = if self.deliveries.is_empty() {
            FeedStatus::Empty
        } else {
            FeedStatus::Ready
        };
        self.stream_status = StreamStatus::Live;
    }

    /// Applies a stream error while keeping the loaded list visible.
    pub fn receive_stream_error(&mut self, error: &ApiError) {
        let app_error = AppError::from_api_error(error);

        self.stream_status = if app_error.kind() == AppErrorKind::Authentication {
            StreamStatus::Error(app_error)
        } else {
            StreamStatus::Reconnecting(app_error)
        };
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

    fn track_page_cursors(&mut self) {
        let latest_page_cursor = self
            .deliveries
            .iter()
            .find_map(|item| item.cursor().cloned());

        for cursor in self.deliveries.iter().filter_map(DeliveryListItem::cursor) {
            self.seen_stream_cursors.insert(cursor.clone());
        }

        if self.stream_resume_cursor.is_none() {
            self.stream_resume_cursor = latest_page_cursor;
        }
    }

    fn note_stream_cursor(&mut self, cursor: StreamCursor) {
        if self.seen_stream_cursors.insert(cursor.clone()) {
            self.stream_resume_cursor = Some(cursor);
        }
    }

    fn insert_or_replace_stream_delivery(&mut self, item: DeliveryListItem) {
        let selected_id = self
            .selected_delivery()
            .map(|delivery| delivery.public_delivery_id().clone());
        let incoming_id = item.public_delivery_id().clone();

        self.deliveries
            .retain(|delivery| delivery.public_delivery_id() != &incoming_id);
        self.deliveries.insert(0, item);

        self.selected_index = selected_id
            .and_then(|selected_id| {
                self.deliveries
                    .iter()
                    .position(|delivery| delivery.public_delivery_id() == &selected_id)
            })
            .or(Some(0));
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
    A: DeliveryApi + Clone + Send + Sync + 'static,
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
    A: DeliveryApi + Clone + Send + Sync + 'static,
{
    let _guard = TerminalModeGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))
        .map_err(|source| TuiError::Terminal { source })?;
    let mut state = AppState::default();

    draw_state(&mut terminal, &state)?;
    load_deliveries(&mut state, api, token).await;
    state.start_stream();
    let stream_events = spawn_stream_worker(
        api.clone(),
        token.clone(),
        state.stream_resume_cursor().cloned(),
    );
    run_event_loop(&mut terminal, &mut state, api, token, &stream_events).await
}

async fn run_event_loop<B, A>(
    terminal: &mut Terminal<B>,
    state: &mut AppState,
    api: &A,
    token: &BearerToken,
    stream_events: &mpsc::Receiver<StreamWorkerMessage>,
) -> Result<(), TuiError>
where
    B: Backend<Error = io::Error>,
    A: DeliveryApi,
{
    loop {
        drain_stream_events(state, stream_events);
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

#[derive(Debug)]
enum StreamWorkerMessage {
    Frames(Vec<DeliveryStreamFrame>),
    Error(ApiError),
}

fn spawn_stream_worker<A>(
    api: A,
    token: BearerToken,
    initial_after: Option<StreamCursor>,
) -> mpsc::Receiver<StreamWorkerMessage>
where
    A: DeliveryApi + Send + Sync + 'static,
{
    let (sender, receiver) = mpsc::channel();

    tokio::spawn(async move {
        let mut after = initial_after;

        loop {
            let frame_sender = sender.clone();
            let mut next_after = after.clone();
            let mut received_frame = false;
            let stream_after = after.clone();
            let result = api
                .stream_deliveries(&token, stream_after.as_ref(), |frame| {
                    if let Some(cursor) = frame_cursor(&frame) {
                        next_after = Some(cursor);
                    }
                    received_frame = true;

                    frame_sender
                        .send(StreamWorkerMessage::Frames(vec![frame]))
                        .map_err(|_| ApiError::InvalidResponse {
                            message: "delivery stream receiver closed".to_owned(),
                        })
                })
                .await;
            after = next_after;

            match result {
                Ok(()) => {
                    if !received_frame
                        && sender
                            .send(StreamWorkerMessage::Frames(Vec::new()))
                            .is_err()
                    {
                        break;
                    }

                    tokio::time::sleep(STREAM_RECONNECT_INTERVAL).await;
                }
                Err(error) => {
                    let is_authentication_error = matches!(error, ApiError::Authentication { .. });

                    if sender.send(StreamWorkerMessage::Error(error)).is_err() {
                        break;
                    }

                    if is_authentication_error {
                        break;
                    }

                    tokio::time::sleep(STREAM_RECONNECT_INTERVAL).await;
                }
            }
        }
    });

    receiver
}

fn drain_stream_events(state: &mut AppState, stream_events: &mpsc::Receiver<StreamWorkerMessage>) {
    while let Ok(message) = stream_events.try_recv() {
        match message {
            StreamWorkerMessage::Frames(frames) => state.receive_stream_frames(frames),
            StreamWorkerMessage::Error(error) => state.receive_stream_error(&error),
        }
    }
}

fn frame_cursor(frame: &DeliveryStreamFrame) -> Option<StreamCursor> {
    match frame {
        DeliveryStreamFrame::Connected => None,
        DeliveryStreamFrame::Cursor(cursor) => Some(cursor.clone()),
        DeliveryStreamFrame::Delivery { cursor, item } => {
            cursor.clone().or_else(|| item.cursor().cloned())
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
            Constraint::Length(1),
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
    let activity = header_activity(state);
    let row_count = state.deliveries().len();
    let rows = match row_count {
        1 => "1 row".to_owned(),
        count => format!("{count} rows"),
    };
    let selected = match (state.selected_index(), row_count) {
        (Some(index), count) if count > 0 => format!("row {}/{}", index + 1, count),
        _other => "no row".to_owned(),
    };
    let line = Line::from(vec![
        Span::styled(
            format!("meshh-tui v{}", env!("CARGO_PKG_VERSION")),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | "),
        Span::raw(activity),
        Span::raw(" | "),
        Span::raw(rows),
        Span::raw(" | "),
        Span::raw(selected),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

fn header_activity(state: &AppState) -> String {
    match state.feed_status() {
        FeedStatus::Loading => "loading".to_owned(),
        FeedStatus::Error(error) => error.heading().to_owned(),
        FeedStatus::Empty => "empty".to_owned(),
        FeedStatus::Ready => match state.stream_status() {
            StreamStatus::Disconnected => "offline".to_owned(),
            StreamStatus::Connecting => "connecting".to_owned(),
            StreamStatus::Live => "live".to_owned(),
            StreamStatus::Reconnecting(error) => format!("reconnecting: {}", error.heading()),
            StreamStatus::Error(error) => error.heading().to_owned(),
        },
    }
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
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::DarkGray)),
                )
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let rows = state.deliveries().iter().map(|item| {
        Row::new(vec![
            Cell::from(format_feed_timestamp(item.display_timestamp())),
            Cell::from(item.source_context().unwrap_or("-").to_owned()),
            Cell::from(item.headline().to_owned()),
            Cell::from(item.status().as_str().to_owned()),
        ])
        .style(Style::default().fg(Color::White))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(18),
            Constraint::Min(24),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new(vec!["Published", "Source", "Headline", "Status"]).style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    )
    .column_spacing(1)
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("> ");
    let mut table_state = TableState::default().with_selected(state.selected_index());

    frame.render_stateful_widget(table, area, &mut table_state);
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
            .block(Block::default().borders(Borders::ALL))
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
        Line::from(format!(
            "Published: {}",
            format_feed_timestamp(detail.display_timestamp())
        )),
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

fn format_feed_timestamp(timestamp: Option<&str>) -> String {
    let Some(timestamp) = timestamp else {
        return "-".to_owned();
    };

    match compact_iso_timestamp(timestamp) {
        Some(compact) => compact,
        None => timestamp.to_owned(),
    }
}

fn compact_iso_timestamp(timestamp: &str) -> Option<String> {
    let parts = timestamp
        .split_once('T')
        .or_else(|| timestamp.split_once(' '))?;
    if parts.0.len() != 10 || parts.1.len() < 5 || !parts.0.is_ascii() || !parts.1.is_ascii() {
        return None;
    }

    let month = match &parts.0[5..7] {
        "01" => "Jan",
        "02" => "Feb",
        "03" => "Mar",
        "04" => "Apr",
        "05" => "May",
        "06" => "Jun",
        "07" => "Jul",
        "08" => "Aug",
        "09" => "Sep",
        "10" => "Oct",
        "11" => "Nov",
        "12" => "Dec",
        _ => return None,
    };
    let day = &parts.0[8..10];
    let time = &parts.1[..5];

    if !day.bytes().all(|byte| byte.is_ascii_digit())
        || !time.as_bytes()[0].is_ascii_digit()
        || !time.as_bytes()[1].is_ascii_digit()
        || time.as_bytes()[2] != b':'
        || !time.as_bytes()[3].is_ascii_digit()
        || !time.as_bytes()[4].is_ascii_digit()
    {
        return None;
    }

    Some(format!("{month} {day} {time}"))
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
    use super::{
        AppCommand, AppErrorKind, AppState, DetailStatus, FeedStatus, KeyAction, Screen,
        StreamStatus,
    };
    use crate::api::{
        ApiError, DeliveryDetail, DeliveryListItem, DeliveryListPage, DeliveryStatus,
        DeliveryStreamFrame, MatchedRoute, PublicDeliveryId, StreamCursor, TransportError,
    };
    use crate::credentials::BearerToken;
    use ratatui::{Terminal, backend::TestBackend};
    use std::time::Duration;

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
    fn stream_frames_insert_rows_deduplicate_replays_track_resume_and_show_auth_errors() {
        let mut state = AppState::default();
        state.receive_delivery_page(DeliveryListPage::new(
            vec![delivery_with_cursor(
                "del_pub_01",
                "CPU alert routed to ops",
                Some("Datadog"),
                Some("cur_01"),
            )],
            Some(StreamCursor::new("cur_history").unwrap()),
        ));

        assert_eq!(
            state.stream_resume_cursor().map(StreamCursor::as_str),
            Some("cur_01")
        );

        state.start_stream();
        assert_eq!(state.stream_status(), &StreamStatus::Connecting);

        let replayed_delivery = delivery("del_pub_02", "Deploy complete", Some("GitHub"));
        state.receive_stream_frames(vec![
            DeliveryStreamFrame::Delivery {
                cursor: Some(StreamCursor::new("cur_02").unwrap()),
                item: replayed_delivery.clone(),
            },
            DeliveryStreamFrame::Delivery {
                cursor: Some(StreamCursor::new("cur_02").unwrap()),
                item: replayed_delivery,
            },
            DeliveryStreamFrame::Cursor(StreamCursor::new("cur_03").unwrap()),
        ]);

        assert_eq!(state.stream_status(), &StreamStatus::Live);
        assert_eq!(state.deliveries().len(), 2);
        assert_eq!(
            state.deliveries()[0].public_delivery_id().as_str(),
            "del_pub_02"
        );
        assert_eq!(
            state.stream_resume_cursor().map(StreamCursor::as_str),
            Some("cur_03")
        );

        state.receive_stream_error(&ApiError::Authentication {
            operation: "streaming deliveries",
            status: 401,
            body: "token_expired".to_owned(),
        });

        let StreamStatus::Error(error) = state.stream_status() else {
            panic!("expected visible stream auth error");
        };
        assert_eq!(error.kind(), AppErrorKind::Authentication);
        assert!(error.message().contains("stored token"));

        let rendered = render_text(&state);
        assert!(rendered.contains("meshh-tui v0.1.0"));
        assert!(rendered.contains("Authentication error"));
        assert!(rendered.contains("2 rows"));
        assert!(!rendered.contains("resume:"));
        assert!(!rendered.contains("cur_03"));
    }

    #[test]
    fn connected_stream_frame_marks_empty_feed_live() {
        let mut state = AppState::default();
        state.receive_delivery_page(DeliveryListPage::new(Vec::new(), None));
        state.start_stream();

        state.receive_stream_frames(vec![DeliveryStreamFrame::Connected]);

        assert_eq!(state.feed_status(), &FeedStatus::Empty);
        assert_eq!(state.stream_status(), &StreamStatus::Live);
        assert!(state.deliveries().is_empty());
    }

    #[tokio::test]
    async fn stream_worker_reconnects_with_latest_seen_cursor() {
        let api = ScriptedStreamApi::new(vec![
            StreamResponse::Frames(vec![DeliveryStreamFrame::Delivery {
                cursor: Some(StreamCursor::new("cur_02").unwrap()),
                item: delivery("del_pub_02", "Deploy complete", Some("GitHub")),
            }]),
            StreamResponse::AuthError,
        ]);
        let receiver = super::spawn_stream_worker(
            api.clone(),
            BearerToken::new("destination-token").unwrap(),
            Some(StreamCursor::new("cur_01").unwrap()),
        );

        match recv_stream_message(&receiver).await {
            super::StreamWorkerMessage::Frames(frames) => assert_eq!(frames.len(), 1),
            super::StreamWorkerMessage::Error(error) => {
                panic!("expected stream frames, got {error}")
            }
        }

        match recv_stream_message(&receiver).await {
            super::StreamWorkerMessage::Error(ApiError::Authentication { status: 401, .. }) => {}
            super::StreamWorkerMessage::Error(error) => {
                panic!("expected stream auth error, got {error}")
            }
            super::StreamWorkerMessage::Frames(frames) => {
                panic!("expected stream auth error, got {frames:?}")
            }
        }

        assert_eq!(
            api.observed_after_values(),
            vec![Some("cur_01".to_owned()), Some("cur_02".to_owned())]
        );
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
        assert!(list_text.contains("meshh-tui v0.1.0 | offline | 1 row | row 1/1"));
        assert!(!list_text.contains("resume:"));
        assert!(!list_text.contains("history:"));
        assert!(list_text.contains("CPU alert routed to ops"));
        assert!(list_text.contains("Datadog"));
        assert!(list_text.contains("delivered"));
        assert!(list_text.contains("Published"));
        assert!(list_text.contains("Jun 04 19:09"));

        state.open_selected_detail().unwrap();
        state.receive_detail(detail("del_pub_01", "CPU alert routed to ops"));

        let detail_text = render_text(&state);
        assert!(detail_text.contains("CPU alert routed to ops"));
        assert!(detail_text.contains("A production route matched this delivery."));
        assert!(detail_text.contains("https://alerts.example/del_pub_01"));
        assert!(detail_text.contains("Ops Escalation"));
        assert!(detail_text.contains("Published: Jun 04 19:09"));
    }

    #[test]
    fn timestamp_formatting_falls_back_for_non_ascii_malformed_input() {
        let timestamp = "2026é6-04T19:09:00Z";

        assert_eq!(super::format_feed_timestamp(Some(timestamp)), timestamp);
    }

    #[test]
    fn render_keeps_selected_row_visible_in_long_lists() {
        let mut state = AppState::default();
        let deliveries = (0..30)
            .map(|index| {
                let public_delivery_id = format!("del_pub_{index:02}");
                let headline = format!("Delivery {index:02}");

                delivery(&public_delivery_id, &headline, None)
            })
            .collect();
        state.receive_delivery_page(DeliveryListPage::new(deliveries, None));

        for _ in 0..18 {
            state.select_next();
        }

        assert_eq!(state.selected_index(), Some(18));
        let list_text = render_text_with_size(&state, 120, 12);
        assert!(list_text.contains("Delivery 18"), "rendered:\n{list_text}");
    }

    fn delivery(
        public_delivery_id: &str,
        headline: &str,
        source_context: Option<&str>,
    ) -> DeliveryListItem {
        delivery_with_cursor(public_delivery_id, headline, source_context, None)
    }

    fn delivery_with_cursor(
        public_delivery_id: &str,
        headline: &str,
        source_context: Option<&str>,
        cursor: Option<&str>,
    ) -> DeliveryListItem {
        DeliveryListItem::new(
            PublicDeliveryId::new(public_delivery_id).unwrap(),
            headline,
            source_context.map(str::to_owned),
            DeliveryStatus::new("delivered").unwrap(),
            Some("2026-06-04T19:09:10Z".to_owned()),
            Some("2026-06-04T02:03:04Z".to_owned()),
            cursor.map(|cursor| StreamCursor::new(cursor).unwrap()),
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
            Some("2026-06-04T19:09:10Z".to_owned()),
            Some("2026-06-04T02:03:04Z".to_owned()),
            vec![MatchedRoute::new("Ops Escalation").unwrap()],
            None,
        )
        .unwrap()
    }

    fn render_text(state: &AppState) -> String {
        render_text_with_size(state, 120, 24)
    }

    fn render_text_with_size(state: &AppState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
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

    async fn recv_stream_message(
        receiver: &std::sync::mpsc::Receiver<super::StreamWorkerMessage>,
    ) -> super::StreamWorkerMessage {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match receiver.try_recv() {
                    Ok(message) => return message,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        panic!("stream worker disconnected before sending a message")
                    }
                }
            }
        })
        .await
        .expect("stream worker message")
    }

    #[derive(Debug, Clone)]
    struct ScriptedStreamApi {
        responses: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<StreamResponse>>>,
        observed_after_values: std::sync::Arc<std::sync::Mutex<Vec<Option<String>>>>,
    }

    impl ScriptedStreamApi {
        fn new(responses: Vec<StreamResponse>) -> Self {
            Self {
                responses: std::sync::Arc::new(std::sync::Mutex::new(responses.into())),
                observed_after_values: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }

        fn observed_after_values(&self) -> Vec<Option<String>> {
            self.observed_after_values.lock().unwrap().clone()
        }
    }

    #[derive(Debug)]
    enum StreamResponse {
        Frames(Vec<DeliveryStreamFrame>),
        AuthError,
    }

    impl super::DeliveryApi for ScriptedStreamApi {
        async fn list_deliveries(
            &self,
            _token: &BearerToken,
        ) -> Result<DeliveryListPage, ApiError> {
            panic!("stream worker should not list deliveries")
        }

        async fn get_delivery(
            &self,
            _token: &BearerToken,
            _public_delivery_id: &PublicDeliveryId,
        ) -> Result<DeliveryDetail, ApiError> {
            panic!("stream worker should not load delivery detail")
        }

        async fn stream_deliveries(
            &self,
            _token: &BearerToken,
            after: Option<&StreamCursor>,
            mut on_frame: impl FnMut(DeliveryStreamFrame) -> Result<(), ApiError> + Send,
        ) -> Result<(), ApiError> {
            self.observed_after_values
                .lock()
                .unwrap()
                .push(after.map(StreamCursor::as_str).map(str::to_owned));

            match self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted stream response")
            {
                StreamResponse::Frames(frames) => {
                    for frame in frames {
                        on_frame(frame)?;
                    }

                    Ok(())
                }
                StreamResponse::AuthError => Err(ApiError::Authentication {
                    operation: "streaming deliveries",
                    status: 401,
                    body: "token_expired".to_owned(),
                }),
            }
        }
    }
}
