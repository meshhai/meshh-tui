use crate::config::RuntimeConfig;

/// Configuration needed by future Meshh API clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiClientConfig {
    api_base_url: String,
}

impl ApiClientConfig {
    /// Creates API client configuration from runtime configuration.
    pub fn from_runtime(config: &RuntimeConfig) -> Self {
        Self {
            api_base_url: config.api_base_url().to_owned(),
        }
    }

    /// Returns the resolved API base URL.
    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }
}
