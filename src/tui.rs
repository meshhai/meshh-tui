use std::{error::Error, fmt, io, sync::mpsc, time::Duration};

use crossterm::{
    cursor::Show,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::{Backend, CrosstermBackend},
};

mod render;
mod state;
mod stream_session;
mod time_display;

pub use render::render;
pub use state::{
    AppCommand, AppError, AppErrorKind, AppState, DeliveryListStatus, DetailStatus, KeyAction,
    Screen, StreamStatus,
};

use crate::{
    api::{
        ApiClient, ApiClientConfig, ApiError, DeliveryApi, DeliveryDetail, DeliveryListPage,
        DeliveryStreamFrame, PublicDeliveryId, StreamCursor,
    },
    config::RuntimeConfig,
    credentials::{BearerToken, CredentialError, CredentialStore, FileCredentialStore},
};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const STREAM_RECONNECT_INTERVAL: Duration = Duration::from_secs(1);

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
    let (load_sender, load_events) = mpsc::channel();
    let mut stream_events = None;

    draw_state(&mut terminal, &state)?;
    spawn_delivery_page_load(
        api.clone(),
        token.clone(),
        state.list_request_generation,
        load_sender.clone(),
    );

    run_event_loop(
        &mut terminal,
        &mut state,
        api,
        token,
        &load_sender,
        &load_events,
        &mut stream_events,
    )
    .await
}

async fn run_event_loop<B, A>(
    terminal: &mut Terminal<B>,
    state: &mut AppState,
    api: &A,
    token: &BearerToken,
    load_sender: &mpsc::Sender<LoadWorkerMessage>,
    load_events: &mpsc::Receiver<LoadWorkerMessage>,
    stream_events: &mut Option<mpsc::Receiver<StreamWorkerMessage>>,
) -> Result<(), TuiError>
where
    B: Backend<Error = io::Error>,
    A: DeliveryApi + Clone + Send + Sync + 'static,
{
    loop {
        drain_load_events(state, load_events);
        drain_stream_events(state, stream_events);
        ensure_stream_started(state, api, token, stream_events);
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
            AppCommand::LoadDeliveries { generation } => {
                draw_state(terminal, state)?;
                spawn_delivery_page_load(
                    api.clone(),
                    token.clone(),
                    generation,
                    load_sender.clone(),
                );
            }
            AppCommand::LoadDetail {
                public_delivery_id,
                generation,
            } => {
                draw_state(terminal, state)?;
                spawn_detail_load(
                    api.clone(),
                    token.clone(),
                    public_delivery_id,
                    generation,
                    load_sender.clone(),
                );
            }
        }
    }
}

#[derive(Debug)]
enum LoadWorkerMessage {
    DeliveryPage {
        generation: u64,
        result: Result<DeliveryListPage, ApiError>,
    },
    Detail {
        public_delivery_id: PublicDeliveryId,
        generation: u64,
        result: Result<Box<DeliveryDetail>, ApiError>,
    },
}

#[derive(Debug)]
enum StreamWorkerMessage {
    Frames(Vec<DeliveryStreamFrame>),
    Error(ApiError),
}

fn spawn_delivery_page_load<A>(
    api: A,
    token: BearerToken,
    generation: u64,
    sender: mpsc::Sender<LoadWorkerMessage>,
) where
    A: DeliveryApi + Send + Sync + 'static,
{
    tokio::spawn(async move {
        let _ = sender.send(LoadWorkerMessage::DeliveryPage {
            generation,
            result: api.list_deliveries(&token).await,
        });
    });
}

fn spawn_detail_load<A>(
    api: A,
    token: BearerToken,
    public_delivery_id: PublicDeliveryId,
    generation: u64,
    sender: mpsc::Sender<LoadWorkerMessage>,
) where
    A: DeliveryApi + Send + Sync + 'static,
{
    tokio::spawn(async move {
        let result = api
            .get_delivery(&token, &public_delivery_id)
            .await
            .map(Box::new);

        let _ = sender.send(LoadWorkerMessage::Detail {
            public_delivery_id,
            generation,
            result,
        });
    });
}

fn spawn_stream_worker<A>(
    api: A,
    token: BearerToken,
    initial_after: Option<StreamCursor>,
) -> mpsc::Receiver<StreamWorkerMessage>
where
    A: DeliveryApi + Send + Sync + 'static,
{
    spawn_stream_worker_with_reconnect_interval(
        api,
        token,
        initial_after,
        STREAM_RECONNECT_INTERVAL,
    )
}

fn spawn_stream_worker_with_reconnect_interval<A>(
    api: A,
    token: BearerToken,
    initial_after: Option<StreamCursor>,
    reconnect_interval: Duration,
) -> mpsc::Receiver<StreamWorkerMessage>
where
    A: DeliveryApi + Send + Sync + 'static,
{
    let (sender, receiver) = mpsc::channel();

    tokio::spawn(async move {
        let mut reconnect = stream_session::DeliveryStreamReconnect::new(initial_after);

        loop {
            let frame_sender = sender.clone();
            let stream_after = reconnect.begin_attempt();
            let result = api
                .stream_deliveries(&token, stream_after.as_ref(), |frame| {
                    reconnect.accept_frame(&frame);

                    frame_sender
                        .send(StreamWorkerMessage::Frames(vec![frame]))
                        .map_err(|_| ApiError::InvalidResponse {
                            message: "delivery stream receiver closed".to_owned(),
                        })
                })
                .await;

            match result {
                Ok(()) => {
                    if !reconnect.received_frame()
                        && sender
                            .send(StreamWorkerMessage::Frames(Vec::new()))
                            .is_err()
                    {
                        break;
                    }

                    tokio::time::sleep(reconnect_interval).await;
                }
                Err(error) => {
                    let is_authentication_error = matches!(error, ApiError::Authentication { .. });

                    if sender.send(StreamWorkerMessage::Error(error)).is_err() {
                        break;
                    }

                    if is_authentication_error {
                        break;
                    }

                    tokio::time::sleep(reconnect_interval).await;
                }
            }
        }
    });

    receiver
}

fn drain_load_events(state: &mut AppState, load_events: &mpsc::Receiver<LoadWorkerMessage>) {
    while let Ok(message) = load_events.try_recv() {
        match message {
            LoadWorkerMessage::DeliveryPage {
                generation,
                result: Ok(page),
            } => state.receive_delivery_page(generation, page),
            LoadWorkerMessage::DeliveryPage {
                generation,
                result: Err(error),
            } => state.receive_list_error(generation, &error),
            LoadWorkerMessage::Detail {
                public_delivery_id,
                generation,
                result: Ok(detail),
            } => {
                if state.detail_request_matches(generation, &public_delivery_id) {
                    state.receive_detail(generation, *detail);
                }
            }
            LoadWorkerMessage::Detail {
                public_delivery_id,
                generation,
                result: Err(error),
            } => {
                if state.detail_request_matches(generation, &public_delivery_id) {
                    state.receive_detail_error(generation, &error);
                }
            }
        }
    }
}

fn drain_stream_events(
    state: &mut AppState,
    stream_events: &Option<mpsc::Receiver<StreamWorkerMessage>>,
) {
    let Some(stream_events) = stream_events else {
        return;
    };

    while let Ok(message) = stream_events.try_recv() {
        match message {
            StreamWorkerMessage::Frames(frames) => state.receive_stream_frames(frames),
            StreamWorkerMessage::Error(error) => state.receive_stream_error(&error),
        }
    }
}

fn ensure_stream_started<A>(
    state: &mut AppState,
    api: &A,
    token: &BearerToken,
    stream_events: &mut Option<mpsc::Receiver<StreamWorkerMessage>>,
) where
    A: DeliveryApi + Clone + Send + Sync + 'static,
{
    if stream_events.is_none()
        && !matches!(state.delivery_list_status(), DeliveryListStatus::Loading)
    {
        state.start_stream();
        *stream_events = Some(spawn_stream_worker(
            api.clone(),
            token.clone(),
            state.stream_resume_cursor().cloned(),
        ));
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
            Self::Credential { .. } => formatter.write_str("could not load MESHH credentials"),
            Self::MissingToken => {
                formatter.write_str("no MESHH token found; run `meshh login` first")
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
        AppCommand, AppErrorKind, AppState, DeliveryListStatus, DetailStatus, KeyAction, Screen,
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
        assert_eq!(state.delivery_list_status(), &DeliveryListStatus::Loading);

        apply_page(
            &mut state,
            DeliveryListPage::new(
                vec![
                    delivery("del_pub_01", "CPU alert routed to ops", Some("Datadog")),
                    delivery("del_pub_02", "Deploy complete", None),
                ],
                Some(StreamCursor::new("cur_next").unwrap()),
            ),
        );

        assert_eq!(state.delivery_list_status(), &DeliveryListStatus::Ready);
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
        apply_page(
            &mut state,
            DeliveryListPage::new(
                vec![
                    delivery("del_pub_01", "CPU alert routed to ops", Some("Datadog")),
                    delivery("del_pub_02", "Deploy complete", Some("GitHub")),
                ],
                None,
            ),
        );

        state.handle_key_action(KeyAction::Down);
        assert_eq!(state.selected_index(), Some(1));

        let command = state.handle_key_action(KeyAction::Open);
        assert_eq!(
            command,
            AppCommand::LoadDetail {
                public_delivery_id: PublicDeliveryId::new("del_pub_02").unwrap(),
                generation: state.detail_request_generation
            }
        );
        assert_eq!(state.detail_status(), &DetailStatus::Loading);
        assert!(matches!(
            state.screen(),
            Screen::DeliveryDetail { public_delivery_id }
                if public_delivery_id.as_str() == "del_pub_02"
        ));

        apply_detail(&mut state, detail("del_pub_02", "Deploy complete"));

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
        apply_page(
            &mut state,
            DeliveryListPage::new(
                vec![delivery("del_pub_01", "CPU alert routed to ops", None)],
                None,
            ),
        );

        assert_eq!(
            state.handle_key_action(KeyAction::Refresh),
            AppCommand::LoadDeliveries {
                generation: state.list_request_generation
            }
        );
        assert_eq!(state.delivery_list_status(), &DeliveryListStatus::Loading);
        assert_eq!(state.handle_key_action(KeyAction::Quit), AppCommand::Quit);
    }

    #[test]
    fn stale_list_load_results_do_not_replace_newer_state() {
        let mut state = AppState::default();
        state.start_loading();
        let stale_generation = state.list_request_generation;
        state.start_loading();
        let active_generation = state.list_request_generation;
        let (sender, receiver) = std::sync::mpsc::channel();

        sender
            .send(super::LoadWorkerMessage::DeliveryPage {
                generation: stale_generation,
                result: Ok(DeliveryListPage::new(
                    vec![delivery("del_pub_old", "Old result", None)],
                    None,
                )),
            })
            .unwrap();
        super::drain_load_events(&mut state, &receiver);

        assert_eq!(state.delivery_list_status(), &DeliveryListStatus::Loading);
        assert!(state.deliveries().is_empty());

        sender
            .send(super::LoadWorkerMessage::DeliveryPage {
                generation: active_generation,
                result: Ok(DeliveryListPage::new(
                    vec![delivery("del_pub_new", "New result", None)],
                    None,
                )),
            })
            .unwrap();
        super::drain_load_events(&mut state, &receiver);

        assert_eq!(state.delivery_list_status(), &DeliveryListStatus::Ready);
        assert_eq!(state.deliveries()[0].headline(), "New result");
    }

    #[test]
    fn stale_detail_load_results_do_not_reopen_closed_or_replaced_detail() {
        let mut state = AppState::default();
        apply_page(
            &mut state,
            DeliveryListPage::new(
                vec![
                    delivery("del_pub_01", "First row", None),
                    delivery("del_pub_02", "Second row", None),
                ],
                None,
            ),
        );
        let (sender, receiver) = std::sync::mpsc::channel();

        let first_public_delivery_id = state.open_selected_detail().unwrap();
        let first_generation = state.detail_request_generation;
        state.close_detail();

        sender
            .send(super::LoadWorkerMessage::Detail {
                public_delivery_id: first_public_delivery_id,
                generation: first_generation,
                result: Ok(Box::new(detail("del_pub_01", "First row"))),
            })
            .unwrap();
        super::drain_load_events(&mut state, &receiver);

        assert_eq!(state.screen(), &Screen::DeliveryList);
        assert_eq!(state.detail_status(), &DetailStatus::Hidden);
        assert!(state.detail().is_none());

        state.select_next();
        let second_public_delivery_id = state.open_selected_detail().unwrap();
        let second_generation = state.detail_request_generation;
        sender
            .send(super::LoadWorkerMessage::Detail {
                public_delivery_id: second_public_delivery_id,
                generation: second_generation,
                result: Ok(Box::new(detail("del_pub_02", "Second row"))),
            })
            .unwrap();
        super::drain_load_events(&mut state, &receiver);

        assert_eq!(state.detail_status(), &DetailStatus::Ready);
        assert_eq!(state.detail().unwrap().headline(), "Second row");
    }

    #[test]
    fn stream_frames_insert_rows_deduplicate_replays_track_resume_and_show_auth_errors() {
        let mut state = AppState::default();
        apply_page(
            &mut state,
            DeliveryListPage::new(
                vec![delivery_with_cursor(
                    "del_pub_01",
                    "CPU alert routed to ops",
                    Some("Datadog"),
                    Some("cur_01"),
                )],
                Some(StreamCursor::new("cur_history").unwrap()),
            ),
        );

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
    fn cursor_frame_does_not_dedupe_following_delivery_with_same_cursor() {
        let mut state = AppState::default();
        apply_page(&mut state, DeliveryListPage::new(Vec::new(), None));

        state.receive_stream_frames(vec![
            DeliveryStreamFrame::Cursor(StreamCursor::new("cur_01").unwrap()),
            DeliveryStreamFrame::Delivery {
                cursor: Some(StreamCursor::new("cur_01").unwrap()),
                item: delivery("del_pub_01", "Deploy complete", Some("GitHub")),
            },
        ]);

        assert_eq!(state.deliveries().len(), 1);
        assert_eq!(
            state.deliveries()[0].public_delivery_id().as_str(),
            "del_pub_01"
        );
        assert_eq!(
            state.stream_resume_cursor().map(StreamCursor::as_str),
            Some("cur_01")
        );
    }

    #[test]
    fn connected_stream_frame_marks_empty_delivery_list_live() {
        let mut state = AppState::default();
        apply_page(&mut state, DeliveryListPage::new(Vec::new(), None));
        state.start_stream();

        state.receive_stream_frames(vec![DeliveryStreamFrame::Connected]);

        assert_eq!(state.delivery_list_status(), &DeliveryListStatus::Empty);
        assert_eq!(state.stream_status(), &StreamStatus::Live);
        assert!(state.deliveries().is_empty());
    }

    #[test]
    fn stream_metadata_frames_do_not_hide_delivery_list_load_errors() {
        let mut state = AppState::default();
        apply_list_error(
            &mut state,
            &ApiError::Transport {
                source: TransportError::new("connection refused"),
            },
        );
        state.start_stream();

        state.receive_stream_frames(vec![
            DeliveryStreamFrame::Connected,
            DeliveryStreamFrame::Cursor(StreamCursor::new("cur_01").unwrap()),
        ]);

        let DeliveryListStatus::Error(error) = state.delivery_list_status() else {
            panic!("expected delivery list error to remain visible");
        };
        assert_eq!(error.kind(), AppErrorKind::Network);
        assert_eq!(state.stream_status(), &StreamStatus::Live);
        assert!(state.deliveries().is_empty());
    }

    #[test]
    fn empty_delivery_list_surfaces_stream_errors() {
        let mut state = AppState::default();
        apply_page(&mut state, DeliveryListPage::new(Vec::new(), None));

        state.receive_stream_error(&ApiError::Transport {
            source: TransportError::new("connection refused"),
        });

        let rendered = render_text(&state);

        assert!(rendered.contains("meshh-tui v0.1.0 | reconnecting: Network error"));
        assert!(rendered.contains("network error"));
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
        let receiver = super::spawn_stream_worker_with_reconnect_interval(
            api.clone(),
            BearerToken::new("destination-token").unwrap(),
            Some(StreamCursor::new("cur_01").unwrap()),
            Duration::from_millis(1),
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

    #[tokio::test]
    async fn stream_worker_resumes_after_standalone_cursor_frames() {
        let api = ScriptedStreamApi::new(vec![
            StreamResponse::Frames(vec![DeliveryStreamFrame::Cursor(
                StreamCursor::new("cur_02").unwrap(),
            )]),
            StreamResponse::AuthError,
        ]);
        let receiver = super::spawn_stream_worker_with_reconnect_interval(
            api.clone(),
            BearerToken::new("destination-token").unwrap(),
            Some(StreamCursor::new("cur_01").unwrap()),
            Duration::from_millis(1),
        );

        match recv_stream_message(&receiver).await {
            super::StreamWorkerMessage::Frames(frames) => assert_eq!(frames.len(), 1),
            super::StreamWorkerMessage::Error(error) => {
                panic!("expected stream cursor frame, got {error}")
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

        apply_page(&mut state, DeliveryListPage::new(Vec::new(), None));
        assert_eq!(state.delivery_list_status(), &DeliveryListStatus::Empty);
        assert_eq!(state.selected_index(), None);

        apply_list_error(
            &mut state,
            &ApiError::Authentication {
                operation: "listing deliveries",
                status: 401,
                body: "token_expired".to_owned(),
            },
        );

        let DeliveryListStatus::Error(error) = state.delivery_list_status() else {
            panic!("expected auth list error");
        };
        assert_eq!(error.kind(), AppErrorKind::Authentication);
        assert!(error.message().contains("stored token"));

        apply_page(
            &mut state,
            DeliveryListPage::new(
                vec![delivery("del_pub_01", "CPU alert routed to ops", None)],
                None,
            ),
        );
        state.open_selected_detail().unwrap();
        apply_detail_error(
            &mut state,
            &ApiError::Transport {
                source: TransportError::new("connection refused"),
            },
        );

        let DetailStatus::Error(error) = state.detail_status() else {
            panic!("expected network detail error");
        };
        assert_eq!(error.kind(), AppErrorKind::Network);
        assert!(error.message().contains("network error"));
    }

    #[test]
    fn render_outputs_list_columns_and_detail_fields() {
        let mut state = AppState::default();
        apply_page(
            &mut state,
            DeliveryListPage::new(
                vec![delivery(
                    "del_pub_01",
                    "CPU alert routed to ops",
                    Some("Datadog"),
                )],
                Some(StreamCursor::new("cur_next").unwrap()),
            ),
        );

        let list_text = render_text(&state);
        assert!(list_text.contains("meshh-tui v0.1.0 | offline | 1 row | row 1/1"));
        assert!(!list_text.contains("resume:"));
        assert!(!list_text.contains("history:"));
        assert!(list_text.contains("CPU alert routed to ops"));
        assert!(list_text.contains("Datadog"));
        assert!(list_text.contains("delivered"));
        assert!(list_text.contains("Published"));
        let expected_timestamp =
            super::time_display::format_delivery_timestamp(Some("2026-06-04T19:09:10Z"));
        assert!(list_text.contains(&expected_timestamp));

        state.open_selected_detail().unwrap();
        apply_detail(&mut state, detail("del_pub_01", "CPU alert routed to ops"));

        let detail_text = render_text(&state);
        assert!(detail_text.contains("CPU alert routed to ops"));
        assert!(detail_text.contains("A production route matched this delivery."));
        assert!(detail_text.contains("https://alerts.example/del_pub_01"));
        assert!(detail_text.contains("Ops Escalation"));
        assert!(detail_text.contains(&format!("Published: {expected_timestamp}")));
    }

    #[test]
    fn timestamp_formatting_converts_rfc3339_to_user_offset() {
        let timestamp = "2026-06-04T19:09:10Z";
        let tokyo = time::UtcOffset::from_hms(9, 0, 0).unwrap();
        let new_york = time::UtcOffset::from_hms(-4, 0, 0).unwrap();

        assert_eq!(
            super::time_display::compact_timestamp_at_offset(timestamp, tokyo).as_deref(),
            Some("Jun 05 04:09")
        );
        assert_eq!(
            super::time_display::compact_timestamp_at_offset(timestamp, new_york).as_deref(),
            Some("Jun 04 15:09")
        );
    }

    #[test]
    fn timestamp_formatting_falls_back_for_non_ascii_malformed_input() {
        let timestamp = "2026é6-04T19:09:00Z";

        assert_eq!(
            super::time_display::format_delivery_timestamp(Some(timestamp)),
            timestamp
        );
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
        apply_page(&mut state, DeliveryListPage::new(deliveries, None));

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

    fn apply_page(state: &mut AppState, page: DeliveryListPage) {
        state.receive_delivery_page(state.list_request_generation, page);
    }

    fn apply_list_error(state: &mut AppState, error: &ApiError) {
        state.receive_list_error(state.list_request_generation, error);
    }

    fn apply_detail(state: &mut AppState, detail: DeliveryDetail) {
        state.receive_detail(state.detail_request_generation, detail);
    }

    fn apply_detail_error(state: &mut AppState, error: &ApiError) {
        state.receive_detail_error(state.detail_request_generation, error);
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
