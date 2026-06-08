use std::{
    cmp::Ordering,
    error::Error,
    fmt, fs, io,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, SystemTime},
};

use serde::{Deserialize, Serialize};

use crate::config::{ConfigError, default_config_dir};

const DEFAULT_REPO: &str = "meshhai/meshh-tui";
const INSTALL_SCRIPT_URL: &str =
    "https://raw.githubusercontent.com/meshhai/meshh-tui/master/scripts/install.sh";
const UPDATE_CACHE_FILE_NAME: &str = "update-check.json";
const UPDATE_CACHE_TTL: Duration = Duration::from_secs(60 * 60 * 24);

/// User-visible update notice rendered by the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateNotice {
    latest_version: String,
}

impl UpdateNotice {
    /// Creates a notice for a newer release tag.
    pub fn new(latest_version: impl Into<String>) -> Self {
        Self {
            latest_version: latest_version.into(),
        }
    }

    /// Returns the newest available release tag.
    pub fn latest_version(&self) -> &str {
        &self.latest_version
    }

    /// Returns the compact message shown in the terminal UI.
    pub fn message(&self) -> String {
        format!(
            "Update available: {} - run 'meshh update'",
            self.latest_version
        )
    }
}

/// Result of checking the latest release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateCheck {
    current_version: String,
    latest_version: String,
    html_url: Option<String>,
    update_available: bool,
}

impl UpdateCheck {
    fn new(
        current_version: impl Into<String>,
        latest_version: impl Into<String>,
        html_url: Option<String>,
    ) -> Self {
        let current_version = current_version.into();
        let latest_version = latest_version.into();
        let update_available = version_is_newer(&latest_version, &current_version);

        Self {
            current_version,
            latest_version,
            html_url,
            update_available,
        }
    }

    /// Returns the running client version.
    pub fn current_version(&self) -> &str {
        &self.current_version
    }

    /// Returns the latest release tag.
    pub fn latest_version(&self) -> &str {
        &self.latest_version
    }

    /// Returns the latest release page URL, when GitHub provided one.
    pub fn html_url(&self) -> Option<&str> {
        self.html_url.as_deref()
    }

    /// Returns whether the latest release is newer than the running version.
    pub fn update_available(&self) -> bool {
        self.update_available
    }

    /// Converts an available update into a TUI notice.
    pub fn notice(&self) -> Option<UpdateNotice> {
        self.update_available
            .then(|| UpdateNotice::new(self.latest_version.clone()))
    }
}

/// Checks GitHub Releases using the local cache when it is still fresh.
pub async fn check_for_update_with_cache() -> Result<Option<UpdateNotice>, UpdateError> {
    let cache_path = default_update_cache_path()?;

    if let Some(check) = read_fresh_cached_check(&cache_path).unwrap_or(None) {
        return Ok(check.notice());
    }

    let check = check_for_update().await?;
    let _ = write_cached_check(&cache_path, &check);

    Ok(check.notice())
}

/// Checks GitHub Releases without reading or writing the local cache.
pub async fn check_for_update() -> Result<UpdateCheck, UpdateError> {
    let latest = fetch_latest_release(DEFAULT_REPO).await?;

    Ok(UpdateCheck::new(
        current_version_tag(),
        latest.tag_name,
        latest.html_url,
    ))
}

/// Writes a human-readable update check result.
pub fn write_check_report(output: &mut impl Write, check: &UpdateCheck) -> Result<(), UpdateError> {
    writeln!(output, "Current version: {}", check.current_version())?;
    writeln!(output, "Latest version:  {}", check.latest_version())?;

    if check.update_available() {
        writeln!(output)?;
        writeln!(output, "Update available.")?;
        if let Some(html_url) = check.html_url() {
            writeln!(output, "Release: {html_url}")?;
        }
    } else {
        writeln!(output)?;
        writeln!(output, "meshh is up to date.")?;
    }

    Ok(())
}

/// Returns the install directory containing the current executable.
pub fn current_install_dir() -> Result<PathBuf, UpdateError> {
    let current_exe = std::env::current_exe().map_err(UpdateError::CurrentExecutable)?;

    current_exe
        .parent()
        .map(PathBuf::from)
        .ok_or(UpdateError::MissingInstallDirectory)
}

/// Runs the checked release installer into the provided install directory.
pub async fn run_installer(install_dir: PathBuf) -> Result<(), UpdateError> {
    let script = download_install_script().await?;
    let mut child = Command::new("sh")
        .env("MESHH_INSTALL_DIR", install_dir)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(UpdateError::RunInstaller)?;

    child
        .stdin
        .as_mut()
        .ok_or(UpdateError::InstallerStdin)?
        .write_all(script.as_bytes())
        .map_err(UpdateError::WriteInstaller)?;

    let status = child.wait().map_err(UpdateError::RunInstaller)?;

    if !status.success() {
        return Err(UpdateError::InstallerFailed { status });
    }

    Ok(())
}

fn current_version_tag() -> String {
    format!("v{}", env!("CARGO_PKG_VERSION"))
}

async fn fetch_latest_release(repo: &str) -> Result<LatestRelease, UpdateError> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let response = reqwest::Client::new()
        .get(url)
        .header(
            reqwest::header::USER_AGENT,
            format!("meshh-tui/{}", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .await
        .map_err(UpdateError::CheckRequest)?;

    let status = response.status();

    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(UpdateError::CheckStatus {
            status: status.as_u16(),
            body: clipped(&body),
        });
    }

    response.json().await.map_err(UpdateError::DecodeRelease)
}

async fn download_install_script() -> Result<String, UpdateError> {
    let response = reqwest::Client::new()
        .get(INSTALL_SCRIPT_URL)
        .header(
            reqwest::header::USER_AGENT,
            format!("meshh-tui/{}", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .await
        .map_err(UpdateError::DownloadInstaller)?;

    let status = response.status();

    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(UpdateError::InstallerDownloadStatus {
            status: status.as_u16(),
            body: clipped(&body),
        });
    }

    response.text().await.map_err(UpdateError::ReadInstaller)
}

fn default_update_cache_path() -> Result<PathBuf, UpdateError> {
    Ok(default_config_dir()?.join(UPDATE_CACHE_FILE_NAME))
}

fn read_fresh_cached_check(path: &PathBuf) -> Result<Option<UpdateCheck>, UpdateError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(UpdateError::ReadCache(source)),
    };
    let modified = metadata.modified().map_err(UpdateError::ReadCache)?;
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::MAX);

    if age > UPDATE_CACHE_TTL {
        return Ok(None);
    }

    let contents = fs::read_to_string(path).map_err(UpdateError::ReadCache)?;
    let cache: CachedUpdateCheck =
        serde_json::from_str(&contents).map_err(UpdateError::DecodeCache)?;

    Ok(Some(UpdateCheck::new(
        current_version_tag(),
        cache.latest_version,
        cache.html_url,
    )))
}

fn write_cached_check(path: &PathBuf, check: &UpdateCheck) -> Result<(), UpdateError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(UpdateError::WriteCache)?;
    }

    let cache = CachedUpdateCheck {
        latest_version: check.latest_version.clone(),
        html_url: check.html_url.clone(),
    };
    let contents = serde_json::to_string(&cache).map_err(UpdateError::EncodeCache)?;

    fs::write(path, contents).map_err(UpdateError::WriteCache)
}

fn version_is_newer(candidate: &str, current: &str) -> bool {
    compare_versions(candidate, current) == Ordering::Greater
}

fn compare_versions(left: &str, right: &str) -> Ordering {
    let left_raw = left;
    let right_raw = right;
    let Some(left) = version_components(left_raw) else {
        return left_raw.cmp(right_raw);
    };
    let Some(right) = version_components(right_raw) else {
        return left_raw.cmp(right_raw);
    };
    let len = left.len().max(right.len());

    for index in 0..len {
        let left = left.get(index).copied().unwrap_or(0);
        let right = right.get(index).copied().unwrap_or(0);

        match left.cmp(&right) {
            Ordering::Equal => {}
            ordering => return ordering,
        }
    }

    Ordering::Equal
}

fn version_components(version: &str) -> Option<Vec<u64>> {
    let version = version.trim().trim_start_matches('v');
    let core = version.split_once('-').map_or(version, |(core, _)| core);
    let components = core
        .split('.')
        .map(str::parse)
        .collect::<Result<Vec<u64>, _>>()
        .ok()?;

    (!components.is_empty()).then_some(components)
}

fn clipped(value: &str) -> String {
    const MAX: usize = 500;

    if value.len() <= MAX {
        return value.to_owned();
    }

    format!("{}...", &value[..MAX])
}

#[derive(Debug, Deserialize)]
struct LatestRelease {
    tag_name: String,
    html_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CachedUpdateCheck {
    latest_version: String,
    html_url: Option<String>,
}

/// Errors produced while checking for or installing updates.
#[derive(Debug)]
pub enum UpdateError {
    Config(ConfigError),
    CheckRequest(reqwest::Error),
    CheckStatus { status: u16, body: String },
    DecodeRelease(reqwest::Error),
    Output(io::Error),
    ReadCache(io::Error),
    DecodeCache(serde_json::Error),
    EncodeCache(serde_json::Error),
    WriteCache(io::Error),
    CurrentExecutable(io::Error),
    MissingInstallDirectory,
    DownloadInstaller(reqwest::Error),
    InstallerDownloadStatus { status: u16, body: String },
    ReadInstaller(reqwest::Error),
    RunInstaller(io::Error),
    InstallerStdin,
    WriteInstaller(io::Error),
    InstallerFailed { status: std::process::ExitStatus },
}

impl fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(source) => write!(formatter, "{source}"),
            Self::CheckRequest(source) => {
                write!(formatter, "could not check for updates: {source}")
            }
            Self::CheckStatus { status, body } if body.is_empty() => {
                write!(formatter, "update check returned HTTP {status}")
            }
            Self::CheckStatus { status, body } => {
                write!(formatter, "update check returned HTTP {status}: {body}")
            }
            Self::DecodeRelease(source) => {
                write!(
                    formatter,
                    "could not decode latest release response: {source}"
                )
            }
            Self::Output(source) => write!(formatter, "could not write update output: {source}"),
            Self::ReadCache(source) => write!(formatter, "could not read update cache: {source}"),
            Self::DecodeCache(source) => {
                write!(formatter, "could not decode update cache: {source}")
            }
            Self::EncodeCache(source) => {
                write!(formatter, "could not encode update cache: {source}")
            }
            Self::WriteCache(source) => write!(formatter, "could not write update cache: {source}"),
            Self::CurrentExecutable(source) => {
                write!(formatter, "could not locate current executable: {source}")
            }
            Self::MissingInstallDirectory => {
                formatter.write_str("could not determine current install directory")
            }
            Self::DownloadInstaller(source) => {
                write!(formatter, "could not download installer: {source}")
            }
            Self::InstallerDownloadStatus { status, body } if body.is_empty() => {
                write!(formatter, "installer download returned HTTP {status}")
            }
            Self::InstallerDownloadStatus { status, body } => {
                write!(
                    formatter,
                    "installer download returned HTTP {status}: {body}"
                )
            }
            Self::ReadInstaller(source) => write!(formatter, "could not read installer: {source}"),
            Self::RunInstaller(source) => write!(formatter, "could not run installer: {source}"),
            Self::InstallerStdin => formatter.write_str("could not open installer input"),
            Self::WriteInstaller(source) => {
                write!(formatter, "could not write installer: {source}")
            }
            Self::InstallerFailed { status } => write!(formatter, "installer failed with {status}"),
        }
    }
}

impl Error for UpdateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Config(source) => Some(source),
            Self::CheckRequest(source)
            | Self::DecodeRelease(source)
            | Self::DownloadInstaller(source)
            | Self::ReadInstaller(source) => Some(source),
            Self::Output(source)
            | Self::ReadCache(source)
            | Self::WriteCache(source)
            | Self::CurrentExecutable(source)
            | Self::RunInstaller(source)
            | Self::WriteInstaller(source) => Some(source),
            Self::DecodeCache(source) | Self::EncodeCache(source) => Some(source),
            Self::CheckStatus { .. }
            | Self::MissingInstallDirectory
            | Self::InstallerDownloadStatus { .. }
            | Self::InstallerStdin
            | Self::InstallerFailed { .. } => None,
        }
    }
}

impl From<ConfigError> for UpdateError {
    fn from(source: ConfigError) -> Self {
        Self::Config(source)
    }
}

impl From<io::Error> for UpdateError {
    fn from(source: io::Error) -> Self {
        Self::Output(source)
    }
}

#[cfg(test)]
mod tests {
    use super::{UpdateCheck, compare_versions};

    #[test]
    fn semantic_versions_compare_numerically() {
        assert!(compare_versions("v0.1.10", "v0.1.9").is_gt());
        assert!(compare_versions("v0.2.0", "v0.1.99").is_gt());
        assert!(compare_versions("v0.1.1", "v0.1.1").is_eq());
        assert!(compare_versions("v0.1", "v0.1.0").is_eq());
    }

    #[test]
    fn update_check_builds_notice_only_for_newer_release() {
        let newer = UpdateCheck::new("v0.1.1", "v0.1.2", None);
        assert!(newer.update_available());
        assert_eq!(
            newer.notice().unwrap().message(),
            "Update available: v0.1.2 - run 'meshh update'"
        );

        let current = UpdateCheck::new("v0.1.1", "v0.1.1", None);
        assert!(!current.update_available());
        assert!(current.notice().is_none());
    }
}
