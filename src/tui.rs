/// High-level screen currently shown by the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    DeliveryList,
    DeliveryDetail { public_delivery_id: String },
}

/// Testable application state for the terminal UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    screen: Screen,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            screen: Screen::DeliveryList,
        }
    }
}

impl AppState {
    /// Returns the active screen.
    pub fn screen(&self) -> &Screen {
        &self.screen
    }
}
