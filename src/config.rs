/// Environment variable used to override the Meshh API base URL.
pub const ENV_API_BASE_URL: &str = "MESHH_API_BASE_URL";

/// Runtime configuration assembled from CLI, environment, and config files.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeConfig {
    api_base_url: Option<String>,
}

impl RuntimeConfig {
    /// Builds runtime configuration from CLI values.
    pub fn from_cli(api_base_url: Option<String>) -> Self {
        Self { api_base_url }
    }

    /// Returns the API base URL override, when one has been configured.
    pub fn api_base_url(&self) -> Option<&str> {
        self.api_base_url.as_deref()
    }
}
