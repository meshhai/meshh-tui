use std::{
    env,
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use serde::Deserialize;

/// Environment variable used to override the MESHH API base URL.
pub const ENV_API_BASE_URL: &str = "MESHH_API_BASE_URL";

/// Production MESHH API base URL used when no override is configured.
pub const DEFAULT_API_BASE_URL: &str = "https://api.meshh.ai";

const APP_CONFIG_DIR_NAME: &str = "meshh";
const CONFIG_FILE_NAME: &str = "config.json";

/// Runtime configuration assembled from CLI, environment, config files, and defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    api_base_url: String,
    api_base_url_source: ApiBaseUrlSource,
}

impl RuntimeConfig {
    /// Builds runtime configuration from CLI values and built-in defaults.
    ///
    /// This helper is useful in tests and in code paths that intentionally do not read the
    /// process environment or config file. Use [`RuntimeConfig::load`] for normal CLI startup.
    pub fn from_cli(api_base_url: Option<String>) -> Self {
        Self::resolve(ConfigSources::default().with_cli_api_base_url(api_base_url))
            .expect("default MESHH API base URL must be non-empty")
    }

    /// Loads runtime configuration from CLI values, the process environment, and the default
    /// platform config file.
    pub fn load(cli_api_base_url: Option<String>) -> Result<Self, ConfigError> {
        Self::load_from(ConfigLoadOptions::new(cli_api_base_url))
    }

    /// Loads runtime configuration using explicit loading options.
    pub fn load_from(options: ConfigLoadOptions) -> Result<Self, ConfigError> {
        let ConfigLoadOptions {
            cli_api_base_url,
            config_path,
        } = options;

        if let Some(cli_api_base_url) = cli_api_base_url {
            return Self::resolve(
                ConfigSources::default().with_cli_api_base_url(Some(cli_api_base_url)),
            );
        }

        if let Some(env_api_base_url) = read_env_api_base_url()? {
            return Self::resolve(
                ConfigSources::default().with_env_api_base_url(Some(env_api_base_url)),
            );
        }

        let config_path = match config_path {
            Some(path) => path,
            None => default_config_path()?,
        };
        let config_file = FileConfig::read_optional(config_path)?;

        Self::resolve(ConfigSources::default().with_config_file(config_file))
    }

    /// Resolves runtime configuration from already-collected sources.
    pub fn resolve(sources: ConfigSources) -> Result<Self, ConfigError> {
        if let Some(value) = sources.cli_api_base_url {
            return Self::from_value(value, ApiBaseUrlSource::Cli);
        }

        if let Some(value) = sources.env_api_base_url {
            return Self::from_value(value, ApiBaseUrlSource::Environment);
        }

        if let Some(value) = sources
            .config_file
            .and_then(|config_file| config_file.api_base_url)
        {
            return Self::from_value(value, ApiBaseUrlSource::ConfigFile);
        }

        Self::from_value(DEFAULT_API_BASE_URL.to_owned(), ApiBaseUrlSource::Default)
    }

    /// Returns the resolved API base URL.
    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }

    /// Returns the source that provided the resolved API base URL.
    pub fn api_base_url_source(&self) -> ApiBaseUrlSource {
        self.api_base_url_source
    }

    fn from_value(value: String, source: ApiBaseUrlSource) -> Result<Self, ConfigError> {
        let api_base_url = value.trim();

        if api_base_url.is_empty() {
            return Err(ConfigError::EmptyApiBaseUrl { source });
        }

        Ok(Self {
            api_base_url: api_base_url.to_owned(),
            api_base_url_source: source,
        })
    }
}

/// Options for loading configuration from external process-local sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigLoadOptions {
    cli_api_base_url: Option<String>,
    config_path: Option<PathBuf>,
}

impl ConfigLoadOptions {
    /// Creates load options from a CLI API base URL override.
    pub fn new(cli_api_base_url: Option<String>) -> Self {
        Self {
            cli_api_base_url,
            config_path: None,
        }
    }

    /// Uses an explicit config file path instead of the platform default.
    pub fn with_config_path(mut self, config_path: impl Into<PathBuf>) -> Self {
        self.config_path = Some(config_path.into());
        self
    }
}

/// Already-collected configuration values in precedence order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigSources {
    cli_api_base_url: Option<String>,
    env_api_base_url: Option<String>,
    config_file: Option<FileConfig>,
}

impl ConfigSources {
    /// Sets the API base URL value provided by CLI flags.
    pub fn with_cli_api_base_url(mut self, value: Option<String>) -> Self {
        self.cli_api_base_url = value;
        self
    }

    /// Sets the API base URL value provided by the process environment.
    pub fn with_env_api_base_url(mut self, value: Option<String>) -> Self {
        self.env_api_base_url = value;
        self
    }

    /// Sets the parsed config file, when a config file exists.
    pub fn with_config_file(mut self, value: Option<FileConfig>) -> Self {
        self.config_file = value;
        self
    }
}

/// Parsed MESHH config file.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct FileConfig {
    #[serde(default)]
    api_base_url: Option<String>,
}

impl FileConfig {
    /// Creates a config-file value for tests or embedding callers.
    pub fn new(api_base_url: Option<String>) -> Self {
        Self { api_base_url }
    }

    /// Returns the API base URL configured in the file.
    pub fn api_base_url(&self) -> Option<&str> {
        self.api_base_url.as_deref()
    }

    /// Reads a config file if it exists.
    pub fn read_optional(path: impl AsRef<Path>) -> Result<Option<Self>, ConfigError> {
        let path = path.as_ref();
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ConfigError::ReadConfigFile {
                    path: path.to_owned(),
                    source,
                });
            }
        };

        serde_json::from_str(&contents)
            .map(Some)
            .map_err(|source| ConfigError::ParseConfigFile {
                path: path.to_owned(),
                source,
            })
    }
}

/// Source that provided the resolved API base URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiBaseUrlSource {
    Cli,
    Environment,
    ConfigFile,
    Default,
}

impl fmt::Display for ApiBaseUrlSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cli => formatter.write_str("CLI flag"),
            Self::Environment => formatter.write_str("environment variable"),
            Self::ConfigFile => formatter.write_str("config file"),
            Self::Default => formatter.write_str("default"),
        }
    }
}

/// Errors produced while loading runtime configuration.
#[derive(Debug)]
pub enum ConfigError {
    MissingConfigDirectory,
    NonUnicodeEnvironmentVariable {
        name: &'static str,
    },
    EmptyApiBaseUrl {
        source: ApiBaseUrlSource,
    },
    ReadConfigFile {
        path: PathBuf,
        source: io::Error,
    },
    ParseConfigFile {
        path: PathBuf,
        source: serde_json::Error,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingConfigDirectory => formatter
                .write_str("could not determine a platform config directory; set HOME or APPDATA"),
            Self::NonUnicodeEnvironmentVariable { name } => {
                write!(formatter, "{name} must contain valid unicode")
            }
            Self::EmptyApiBaseUrl { source } => {
                write!(formatter, "API base URL from {source} cannot be empty")
            }
            Self::ReadConfigFile { path, .. } => {
                write!(formatter, "could not read config file {}", path.display())
            }
            Self::ParseConfigFile { path, .. } => {
                write!(formatter, "could not parse config file {}", path.display())
            }
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReadConfigFile { source, .. } => Some(source),
            Self::ParseConfigFile { source, .. } => Some(source),
            Self::MissingConfigDirectory
            | Self::NonUnicodeEnvironmentVariable { .. }
            | Self::EmptyApiBaseUrl { .. } => None,
        }
    }
}

/// Returns the platform config directory used by MESHH.
pub fn default_config_dir() -> Result<PathBuf, ConfigError> {
    default_config_dir_from_env()
}

/// Returns the platform config file path used by MESHH.
pub fn default_config_path() -> Result<PathBuf, ConfigError> {
    Ok(default_config_dir()?.join(CONFIG_FILE_NAME))
}

#[cfg(target_os = "windows")]
fn default_config_dir_from_env() -> Result<PathBuf, ConfigError> {
    env_path("APPDATA")
        .map(|path| path.join(APP_CONFIG_DIR_NAME))
        .ok_or(ConfigError::MissingConfigDirectory)
}

#[cfg(target_os = "macos")]
fn default_config_dir_from_env() -> Result<PathBuf, ConfigError> {
    env_path("HOME")
        .map(|path| {
            path.join("Library")
                .join("Application Support")
                .join(APP_CONFIG_DIR_NAME)
        })
        .ok_or(ConfigError::MissingConfigDirectory)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn default_config_dir_from_env() -> Result<PathBuf, ConfigError> {
    if let Some(path) = env_path("XDG_CONFIG_HOME") {
        return Ok(path.join(APP_CONFIG_DIR_NAME));
    }

    env_path("HOME")
        .map(|path| path.join(".config").join(APP_CONFIG_DIR_NAME))
        .ok_or(ConfigError::MissingConfigDirectory)
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn read_env_api_base_url() -> Result<Option<String>, ConfigError> {
    match env::var(ENV_API_BASE_URL) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(ConfigError::NonUnicodeEnvironmentVariable {
            name: ENV_API_BASE_URL,
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        env,
        ffi::OsString,
        fs,
        path::PathBuf,
        sync::Mutex,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        ApiBaseUrlSource, ConfigError, ConfigLoadOptions, ConfigSources, DEFAULT_API_BASE_URL,
        ENV_API_BASE_URL, FileConfig, RuntimeConfig,
    };

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn api_base_url_precedence_is_cli_env_config_file_default() {
        let file_config = FileConfig::new(Some("https://config.example".to_owned()));

        let default_config = RuntimeConfig::resolve(ConfigSources::default()).unwrap();
        assert_eq!(default_config.api_base_url(), DEFAULT_API_BASE_URL);
        assert_eq!(
            default_config.api_base_url_source(),
            ApiBaseUrlSource::Default
        );

        let from_file = RuntimeConfig::resolve(
            ConfigSources::default().with_config_file(Some(file_config.clone())),
        )
        .unwrap();
        assert_eq!(from_file.api_base_url(), "https://config.example");
        assert_eq!(
            from_file.api_base_url_source(),
            ApiBaseUrlSource::ConfigFile
        );

        let from_env = RuntimeConfig::resolve(
            ConfigSources::default()
                .with_env_api_base_url(Some("https://env.example".to_owned()))
                .with_config_file(Some(file_config.clone())),
        )
        .unwrap();
        assert_eq!(from_env.api_base_url(), "https://env.example");
        assert_eq!(
            from_env.api_base_url_source(),
            ApiBaseUrlSource::Environment
        );

        let from_cli = RuntimeConfig::resolve(
            ConfigSources::default()
                .with_cli_api_base_url(Some("https://cli.example".to_owned()))
                .with_env_api_base_url(Some("https://env.example".to_owned()))
                .with_config_file(Some(file_config)),
        )
        .unwrap();
        assert_eq!(from_cli.api_base_url(), "https://cli.example");
        assert_eq!(from_cli.api_base_url_source(), ApiBaseUrlSource::Cli);
    }

    #[test]
    fn rejects_empty_api_base_url_from_highest_precedence_source() {
        let error = RuntimeConfig::resolve(
            ConfigSources::default()
                .with_cli_api_base_url(Some(" ".to_owned()))
                .with_env_api_base_url(Some("https://env.example".to_owned())),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ConfigError::EmptyApiBaseUrl {
                source: ApiBaseUrlSource::Cli
            }
        ));
    }

    #[test]
    fn reads_config_file_from_json() {
        let path = temp_file_path("meshh-config", "json");
        fs::write(&path, r#"{"api_base_url":"https://file.example"}"#).unwrap();

        let config = FileConfig::read_optional(&path).unwrap().unwrap();

        assert_eq!(config.api_base_url(), Some("https://file.example"));

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn cli_api_base_url_wins_without_parsing_malformed_config_file() {
        let path = temp_file_path("meshh-config-malformed", "json");
        fs::write(&path, "{not-json").unwrap();

        let config = RuntimeConfig::load_from(
            ConfigLoadOptions::new(Some("https://cli.example".to_owned())).with_config_path(&path),
        )
        .unwrap();

        assert_eq!(config.api_base_url(), "https://cli.example");
        assert_eq!(config.api_base_url_source(), ApiBaseUrlSource::Cli);

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn env_api_base_url_wins_without_parsing_malformed_config_file() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let path = temp_file_path("meshh-config-malformed", "json");
        fs::write(&path, "{not-json").unwrap();
        let previous = set_api_base_url_env("https://env.example");

        let result = RuntimeConfig::load_from(ConfigLoadOptions::new(None).with_config_path(&path));

        restore_api_base_url_env(previous);
        fs::remove_file(path).unwrap();

        let config = result.unwrap();
        assert_eq!(config.api_base_url(), "https://env.example");
        assert_eq!(config.api_base_url_source(), ApiBaseUrlSource::Environment);
    }

    fn temp_file_path(prefix: &str, extension: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        std::env::temp_dir().join(format!("{prefix}-{}.{extension}", unique))
    }

    fn set_api_base_url_env(value: &str) -> Option<OsString> {
        let previous = env::var_os(ENV_API_BASE_URL);

        // SAFETY: this test module serializes process-environment mutation with ENV_MUTEX and
        // restores the original value before releasing the lock.
        unsafe {
            env::set_var(ENV_API_BASE_URL, value);
        }

        previous
    }

    fn restore_api_base_url_env(previous: Option<OsString>) {
        // SAFETY: this test module serializes process-environment mutation with ENV_MUTEX and
        // restores the original value before releasing the lock.
        unsafe {
            if let Some(value) = previous {
                env::set_var(ENV_API_BASE_URL, value);
            } else {
                env::remove_var(ENV_API_BASE_URL);
            }
        }
    }
}
