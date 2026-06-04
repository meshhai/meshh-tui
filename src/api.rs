use crate::config::RuntimeConfig;

/// Configuration needed by future Meshh API clients.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApiClientConfig {
    api_base_url: Option<String>,
}

impl ApiClientConfig {
    /// Creates API client configuration from runtime configuration.
    pub fn from_runtime(config: &RuntimeConfig) -> Self {
        Self {
            api_base_url: config.api_base_url().map(str::to_owned),
        }
    }

    /// Returns the API base URL override, when one has been configured.
    pub fn api_base_url(&self) -> Option<&str> {
        self.api_base_url.as_deref()
    }
}
