use crate::api::{
    ApiError, DeliveryDetail, DeliveryListItem, DeliveryListPage, DeliveryStreamFrame,
    PublicDeliveryId, StreamCursor,
};
use crate::update::UpdateNotice;

use super::stream_session::DeliveryStreamSession;

/// High-level screen currently shown by the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    DeliveryList,
    DeliveryDetail {
        public_delivery_id: PublicDeliveryId,
    },
}

/// Delivery list state visible to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryListStatus {
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

    pub(super) fn heading(&self) -> &'static str {
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
    LoadDeliveries {
        generation: u64,
    },
    LoadDetail {
        public_delivery_id: PublicDeliveryId,
        generation: u64,
    },
    Quit,
}

/// Testable application state for the terminal UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    screen: Screen,
    delivery_list_status: DeliveryListStatus,
    stream_status: StreamStatus,
    deliveries: Vec<DeliveryListItem>,
    selected_index: Option<usize>,
    next_cursor: Option<StreamCursor>,
    stream_session: DeliveryStreamSession,
    detail_status: DetailStatus,
    detail: Option<DeliveryDetail>,
    update_notice: Option<UpdateNotice>,
    pub(super) list_request_generation: u64,
    pub(super) detail_request_generation: u64,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            screen: Screen::DeliveryList,
            delivery_list_status: DeliveryListStatus::Loading,
            stream_status: StreamStatus::Disconnected,
            deliveries: Vec::new(),
            selected_index: None,
            next_cursor: None,
            stream_session: DeliveryStreamSession::default(),
            detail_status: DetailStatus::Hidden,
            detail: None,
            update_notice: None,
            list_request_generation: 0,
            detail_request_generation: 0,
        }
    }
}

impl AppState {
    /// Returns the active screen.
    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    /// Returns the current delivery list status.
    pub fn delivery_list_status(&self) -> &DeliveryListStatus {
        &self.delivery_list_status
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
        self.stream_session.resume_cursor()
    }

    /// Returns the loaded detail record.
    pub fn detail(&self) -> Option<&DeliveryDetail> {
        self.detail.as_ref()
    }

    /// Returns the available update notice, when one has been discovered.
    pub fn update_notice(&self) -> Option<&UpdateNotice> {
        self.update_notice.as_ref()
    }

    /// Applies the latest update notice from the background checker.
    pub fn receive_update_notice(&mut self, notice: Option<UpdateNotice>) {
        self.update_notice = notice;
    }

    /// Moves the delivery list into a loading state.
    pub fn start_loading(&mut self) {
        self.screen = Screen::DeliveryList;
        self.delivery_list_status = DeliveryListStatus::Loading;
        self.detail_status = DetailStatus::Hidden;
        self.detail = None;
        self.list_request_generation = self.list_request_generation.saturating_add(1);
        self.detail_request_generation = self.detail_request_generation.saturating_add(1);
    }

    /// Applies a successful delivery list response.
    pub fn receive_delivery_page(&mut self, generation: u64, page: DeliveryListPage) {
        if generation != self.list_request_generation {
            return;
        }

        self.deliveries = page.items().to_vec();
        self.selected_index = if self.deliveries.is_empty() {
            None
        } else {
            Some(0)
        };
        self.next_cursor = page.next_cursor().cloned();
        self.stream_session.track_page_cursors(&self.deliveries);
        self.delivery_list_status = if self.deliveries.is_empty() {
            DeliveryListStatus::Empty
        } else {
            DeliveryListStatus::Ready
        };
    }

    /// Applies a failed delivery list response without panicking or printing secrets.
    pub fn receive_list_error(&mut self, generation: u64, error: &ApiError) {
        if generation != self.list_request_generation {
            return;
        }

        self.delivery_list_status = DeliveryListStatus::Error(AppError::from_api_error(error));
        self.clamp_selection();
    }

    /// Moves the stream into a connecting state.
    pub fn start_stream(&mut self) {
        self.stream_status = StreamStatus::Connecting;
    }

    /// Applies stream frames, prepending new deliveries and suppressing cursor replays.
    pub fn receive_stream_frames(&mut self, frames: Vec<DeliveryStreamFrame>) {
        let deliveries = self.stream_session.accept_frames(frames);
        let inserted_delivery = !deliveries.is_empty();

        for item in deliveries {
            self.insert_or_replace_stream_delivery(item);
        }

        if inserted_delivery {
            self.delivery_list_status = if self.deliveries.is_empty() {
                DeliveryListStatus::Empty
            } else {
                DeliveryListStatus::Ready
            };
        }

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
        self.detail_request_generation = self.detail_request_generation.saturating_add(1);

        Some(public_delivery_id)
    }

    /// Applies a successful detail response.
    pub fn receive_detail(&mut self, generation: u64, detail: DeliveryDetail) {
        let public_delivery_id = detail.public_delivery_id().clone();

        if !self.detail_request_matches(generation, &public_delivery_id) {
            return;
        }

        self.screen = Screen::DeliveryDetail { public_delivery_id };
        self.detail_status = DetailStatus::Ready;
        self.detail = Some(detail);
    }

    /// Applies a failed detail response while keeping the detail screen visible.
    pub fn receive_detail_error(&mut self, generation: u64, error: &ApiError) {
        if generation != self.detail_request_generation {
            return;
        }

        self.detail_status = DetailStatus::Error(AppError::from_api_error(error));
    }

    /// Returns from detail to the delivery list.
    pub fn close_detail(&mut self) {
        self.screen = Screen::DeliveryList;
        self.detail_status = DetailStatus::Hidden;
        self.detail = None;
        self.detail_request_generation = self.detail_request_generation.saturating_add(1);
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
                        .map(|public_delivery_id| AppCommand::LoadDetail {
                            public_delivery_id,
                            generation: self.detail_request_generation,
                        })
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
                    AppCommand::LoadDeliveries {
                        generation: self.list_request_generation,
                    }
                }
                Screen::DeliveryDetail { public_delivery_id } => {
                    let public_delivery_id = public_delivery_id.clone();
                    self.detail_status = DetailStatus::Loading;
                    self.detail = None;
                    self.detail_request_generation =
                        self.detail_request_generation.saturating_add(1);
                    AppCommand::LoadDetail {
                        public_delivery_id,
                        generation: self.detail_request_generation,
                    }
                }
            },
        }
    }

    pub(super) fn detail_request_matches(
        &self,
        generation: u64,
        public_delivery_id: &PublicDeliveryId,
    ) -> bool {
        generation == self.detail_request_generation
            && matches!(
                &self.screen,
                Screen::DeliveryDetail {
                    public_delivery_id: current_public_delivery_id
                } if current_public_delivery_id == public_delivery_id
            )
    }

    fn clamp_selection(&mut self) {
        self.selected_index = match (self.selected_index, self.deliveries.len()) {
            (_, 0) => None,
            (Some(index), len) if index >= len => Some(len - 1),
            (index, _) => index,
        };
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
