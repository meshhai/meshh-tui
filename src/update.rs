use std::{
    cmp::Ordering,
    error::Error,
    fmt, fs, io,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    time::{Duration, SystemTime},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{ConfigError, default_config_dir};

const DEFAULT_REPO: &str = "meshhai/meshh-tui";
const BINARY_NAME: &str = "meshh";
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
    package: Option<ReleasePackage>,
    installable: bool,
    update_available: bool,
}

impl UpdateCheck {
    fn new(
        current_version: impl Into<String>,
        latest_version: impl Into<String>,
        html_url: Option<String>,
        package: Option<ReleasePackage>,
    ) -> Self {
        let current_version = current_version.into();
        let latest_version = latest_version.into();
        let update_available = version_is_newer(&latest_version, &current_version);
        let installable = package.is_some();

        Self {
            current_version,
            latest_version,
            html_url,
            package,
            installable,
            update_available,
        }
    }

    fn cached(
        current_version: impl Into<String>,
        latest_version: impl Into<String>,
        html_url: Option<String>,
        installable: bool,
    ) -> Self {
        let current_version = current_version.into();
        let latest_version = latest_version.into();
        let update_available = version_is_newer(&latest_version, &current_version);

        Self {
            current_version,
            latest_version,
            html_url,
            package: None,
            installable,
            update_available,
        }
    }

    fn from_latest_release(current_version: impl Into<String>, latest: LatestRelease) -> Self {
        let package = select_release_package(&latest);

        Self::new(current_version, latest.tag_name, latest.html_url, package)
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

    /// Returns whether a newer release can be installed on this platform.
    pub fn installable_update_available(&self) -> bool {
        self.update_available && self.installable
    }

    /// Returns the release package for the current platform, when the release publishes one.
    fn package(&self) -> Option<&ReleasePackage> {
        self.package.as_ref()
    }

    /// Converts an available update into a TUI notice.
    pub fn notice(&self) -> Option<UpdateNotice> {
        self.installable_update_available()
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

    Ok(UpdateCheck::from_latest_release(
        current_version_tag(),
        latest,
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
        if let Some(package) = check.package() {
            writeln!(output, "Archive: {}", package.archive_url)?;
            writeln!(output, "Checksum: {}", package.checksum_url)?;
        } else {
            writeln!(output, "No update archive is available for this platform.")?;
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

/// Installs the checked latest release into the provided install directory.
pub async fn install_checked_release(
    check: &UpdateCheck,
    install_dir: PathBuf,
) -> Result<(), UpdateError> {
    let target = release_target()?.triple;
    let package = check
        .package()
        .ok_or_else(|| UpdateError::MissingReleaseAsset {
            version: check.latest_version.clone(),
            target,
        })?;
    let temp_dir = TempDir::create("meshh-update")?;
    let archive_path = temp_dir.path().join(&package.archive_name);
    let checksum_path = temp_dir.path().join(&package.checksum_name);

    let archive = download_bytes(&package.archive_url).await?;
    let checksum = download_text(&package.checksum_url).await?;
    let expected_hash = parse_checksum(&checksum, &package.archive_name)?;
    verify_sha256(&archive, &expected_hash)?;

    fs::write(&archive_path, archive).map_err(UpdateError::WriteUpdateFile)?;
    fs::write(&checksum_path, checksum).map_err(UpdateError::WriteUpdateFile)?;

    validate_archive(&archive_path, &package.archive_dir)?;
    extract_archive_binary(&archive_path, &package.archive_dir, temp_dir.path())?;
    install_binary(
        &temp_dir.path().join(&package.archive_dir).join(BINARY_NAME),
        &install_dir,
    )?;

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

fn select_release_package(release: &LatestRelease) -> Option<ReleasePackage> {
    let target = release_target().ok()?;
    let version_number = release.tag_name.trim_start_matches('v');
    let archive_name = format!("meshh_{version_number}_{}.tar.gz", target.triple);
    let checksum_name = format!("{archive_name}.sha256");
    let archive_asset = release
        .assets
        .iter()
        .find(|asset| asset.name == archive_name)?;
    let checksum_asset = release
        .assets
        .iter()
        .find(|asset| asset.name == checksum_name)?;

    Some(ReleasePackage {
        archive_name,
        archive_url: archive_asset.browser_download_url.clone(),
        checksum_name,
        checksum_url: checksum_asset.browser_download_url.clone(),
        archive_dir: format!("meshh_{version_number}_{}", target.triple),
    })
}

fn release_target() -> Result<ReleaseTarget, UpdateError> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let triple = match (os, arch) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        _ => {
            return Err(UpdateError::UnsupportedTarget {
                os: os.to_owned(),
                arch: arch.to_owned(),
            });
        }
    };

    Ok(ReleaseTarget {
        triple: triple.to_owned(),
    })
}

async fn download_bytes(url: &str) -> Result<Vec<u8>, UpdateError> {
    let response = reqwest::Client::new()
        .get(url)
        .header(
            reqwest::header::USER_AGENT,
            format!("meshh-tui/{}", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .await
        .map_err(UpdateError::DownloadReleaseAsset)?;

    let status = response.status();

    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(UpdateError::ReleaseAssetStatus {
            status: status.as_u16(),
            body: clipped(&body),
        });
    }

    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(UpdateError::ReadReleaseAsset)
}

async fn download_text(url: &str) -> Result<String, UpdateError> {
    let bytes = download_bytes(url).await?;

    String::from_utf8(bytes).map_err(UpdateError::DecodeReleaseAsset)
}

fn parse_checksum(contents: &str, archive_name: &str) -> Result<String, UpdateError> {
    let mut fields = contents.split_whitespace();
    let Some(hash) = fields.next() else {
        return Err(UpdateError::InvalidChecksum {
            message: "checksum file was empty".to_owned(),
        });
    };

    if hash.len() != 64 || !hash.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(UpdateError::InvalidChecksum {
            message: "checksum file did not start with a SHA256 hex digest".to_owned(),
        });
    }

    if let Some(path) = fields.next() {
        let path = path.trim_start_matches('*');
        let name_matches = Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == archive_name);

        if !name_matches {
            return Err(UpdateError::InvalidChecksum {
                message: format!("checksum file referenced unexpected archive `{path}`"),
            });
        }
    }

    Ok(hash.to_ascii_lowercase())
}

fn verify_sha256(bytes: &[u8], expected_hash: &str) -> Result<(), UpdateError> {
    let actual_hash = hex_sha256(bytes);

    if actual_hash != expected_hash {
        return Err(UpdateError::ChecksumMismatch {
            expected: expected_hash.to_owned(),
            actual: actual_hash,
        });
    }

    Ok(())
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);

    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to a string cannot fail");
    }

    output
}

fn validate_archive(archive_path: &Path, expected_dir: &str) -> Result<(), UpdateError> {
    let entries_output = run_command_capture(
        Command::new("tar").arg("-tzf").arg(archive_path),
        "listing release archive",
    )?;
    let entries = String::from_utf8(entries_output).map_err(UpdateError::DecodeCommandOutput)?;

    for entry in entries.lines() {
        if !safe_archive_entry(entry, expected_dir) {
            return Err(UpdateError::UnsafeArchiveEntry {
                entry: entry.to_owned(),
            });
        }
    }

    let binary_entry = format!("{expected_dir}/{BINARY_NAME}");
    let binary_entry_count = entries
        .lines()
        .filter(|entry| *entry == binary_entry)
        .count();

    match binary_entry_count {
        0 => {
            return Err(UpdateError::MissingArchiveBinary { path: binary_entry });
        }
        1 => {}
        _ => {
            return Err(UpdateError::DuplicateArchiveBinary { path: binary_entry });
        }
    }

    let verbose_output = run_command_capture(
        Command::new("tar").arg("-tvzf").arg(archive_path),
        "inspecting release archive",
    )?;
    let verbose = String::from_utf8(verbose_output).map_err(UpdateError::DecodeCommandOutput)?;

    let binary_verbose_entries = verbose
        .lines()
        .filter(|line| {
            line.strip_suffix(&format!(" {expected_dir}/{BINARY_NAME}"))
                .is_some()
        })
        .collect::<Vec<_>>();

    if binary_verbose_entries.len() != 1 {
        return Err(UpdateError::DuplicateArchiveBinary {
            path: format!("{expected_dir}/{BINARY_NAME}"),
        });
    }

    if !binary_verbose_entries[0].starts_with('-') {
        return Err(UpdateError::ArchiveBinaryNotRegular {
            path: format!("{expected_dir}/{BINARY_NAME}"),
        });
    }

    Ok(())
}

fn safe_archive_entry(entry: &str, expected_dir: &str) -> bool {
    !entry.is_empty()
        && !entry.starts_with('/')
        && entry != ".."
        && !entry.starts_with("../")
        && !entry.ends_with("/..")
        && !entry.contains("/../")
        && (entry == expected_dir || entry.starts_with(&format!("{expected_dir}/")))
}

fn extract_archive_binary(
    archive_path: &Path,
    archive_dir: &str,
    output_dir: &Path,
) -> Result<(), UpdateError> {
    let binary_entry = format!("{archive_dir}/{BINARY_NAME}");
    run_command(
        Command::new("tar")
            .arg("-xzf")
            .arg(archive_path)
            .arg("-C")
            .arg(output_dir)
            .arg(&binary_entry),
        "extracting release archive",
    )?;

    let binary_path = output_dir.join(binary_entry);
    let metadata = fs::symlink_metadata(&binary_path).map_err(UpdateError::ReadUpdateFile)?;

    if !metadata.file_type().is_file() {
        return Err(UpdateError::ArchiveBinaryNotRegular {
            path: binary_path.display().to_string(),
        });
    }

    Ok(())
}

fn install_binary(source_path: &Path, install_dir: &Path) -> Result<(), UpdateError> {
    fs::create_dir_all(install_dir).map_err(UpdateError::InstallDirectory)?;
    let destination_path = install_dir.join(BINARY_NAME);
    let status = Command::new("install")
        .arg("-m")
        .arg("0755")
        .arg(source_path)
        .arg(&destination_path)
        .status()
        .map_err(UpdateError::InstallBinary)?;

    if status.success() {
        return Ok(());
    }

    if command_exists("sudo") {
        run_command(
            Command::new("sudo")
                .arg("install")
                .arg("-m")
                .arg("0755")
                .arg(source_path)
                .arg(&destination_path),
            "installing release binary with sudo",
        )?;

        return Ok(());
    }

    Err(UpdateError::InstallFailed { status })
}

fn command_exists(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

fn run_command(command: &mut Command, operation: &'static str) -> Result<(), UpdateError> {
    let status = command
        .status()
        .map_err(|source| UpdateError::Command { operation, source })?;

    if !status.success() {
        return Err(UpdateError::CommandStatus { operation, status });
    }

    Ok(())
}

fn run_command_capture(
    command: &mut Command,
    operation: &'static str,
) -> Result<Vec<u8>, UpdateError> {
    let output = command
        .output()
        .map_err(|source| UpdateError::Command { operation, source })?;

    if !output.status.success() {
        return Err(UpdateError::CommandStatus {
            operation,
            status: output.status,
        });
    }

    Ok(output.stdout)
}

#[derive(Debug)]
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn create(prefix: &str) -> Result<Self, UpdateError> {
        let base = std::env::temp_dir();

        for attempt in 0..100 {
            let unique = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = base.join(format!("{prefix}-{unique}-{attempt}"));

            match create_private_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(source) if source.kind() == ErrorKind::AlreadyExists => {}
                Err(source) => return Err(UpdateError::CreateTempDir(source)),
            }
        }

        Err(UpdateError::CreateTempDir(io::Error::new(
            ErrorKind::AlreadyExists,
            "could not create unique temporary update directory",
        )))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
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

    Ok(Some(cached_update_check(cache)))
}

fn cached_update_check(cache: CachedUpdateCheck) -> UpdateCheck {
    UpdateCheck::cached(
        current_version_tag(),
        cache.latest_version,
        cache.html_url,
        cache.installable,
    )
}

fn write_cached_check(path: &PathBuf, check: &UpdateCheck) -> Result<(), UpdateError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(UpdateError::WriteCache)?;
    }

    let cache = CachedUpdateCheck {
        latest_version: check.latest_version.clone(),
        html_url: check.html_url.clone(),
        installable: check.installable,
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
    let Some(left) = ParsedVersion::parse(left_raw) else {
        return left_raw.cmp(right_raw);
    };
    let Some(right) = ParsedVersion::parse(right_raw) else {
        return left_raw.cmp(right_raw);
    };

    left.cmp(&right)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedVersion {
    components: Vec<u64>,
    prerelease: Option<Vec<String>>,
}

impl ParsedVersion {
    fn parse(version: &str) -> Option<Self> {
        let version = version.trim().trim_start_matches('v');
        let core_and_pre = version
            .split_once('+')
            .map_or(version, |(left, _build)| left);
        let (core, prerelease) = core_and_pre
            .split_once('-')
            .map_or((core_and_pre, None), |(core, prerelease)| {
                (core, Some(prerelease))
            });
        let components = core
            .split('.')
            .map(str::parse)
            .collect::<Result<Vec<u64>, _>>()
            .ok()?;

        if components.is_empty() {
            return None;
        }

        let prerelease = prerelease.map(|prerelease| {
            prerelease
                .split('.')
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        });

        Some(Self {
            components,
            prerelease,
        })
    }
}

impl Ord for ParsedVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        let len = self.components.len().max(other.components.len());

        for index in 0..len {
            let left = self.components.get(index).copied().unwrap_or(0);
            let right = other.components.get(index).copied().unwrap_or(0);

            match left.cmp(&right) {
                Ordering::Equal => {}
                ordering => return ordering,
            }
        }

        compare_prerelease(&self.prerelease, &other.prerelease)
    }
}

impl PartialOrd for ParsedVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn compare_prerelease(left: &Option<Vec<String>>, right: &Option<Vec<String>>) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left), Some(right)) => compare_prerelease_identifiers(left, right),
    }
}

fn compare_prerelease_identifiers(left: &[String], right: &[String]) -> Ordering {
    let len = left.len().max(right.len());

    for index in 0..len {
        match (left.get(index), right.get(index)) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(left), Some(right)) => match compare_prerelease_identifier(left, right) {
                Ordering::Equal => {}
                ordering => return ordering,
            },
        }
    }

    Ordering::Equal
}

fn compare_prerelease_identifier(left: &str, right: &str) -> Ordering {
    match (left.parse::<u64>(), right.parse::<u64>()) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => left.cmp(right),
    }
}

fn clipped(value: &str) -> String {
    const MAX: usize = 500;

    if value.chars().count() <= MAX {
        return value.to_owned();
    }

    let clipped = value.chars().take(MAX).collect::<String>();
    format!("{clipped}...")
}

#[derive(Debug, Deserialize)]
struct LatestRelease {
    tag_name: String,
    html_url: Option<String>,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReleasePackage {
    archive_name: String,
    archive_url: String,
    checksum_name: String,
    checksum_url: String,
    archive_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReleaseTarget {
    triple: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CachedUpdateCheck {
    latest_version: String,
    html_url: Option<String>,
    #[serde(default)]
    installable: bool,
}

/// Errors produced while checking for or installing updates.
#[derive(Debug)]
pub enum UpdateError {
    Config(ConfigError),
    UnsupportedTarget {
        os: String,
        arch: String,
    },
    CheckRequest(reqwest::Error),
    CheckStatus {
        status: u16,
        body: String,
    },
    DecodeRelease(reqwest::Error),
    Output(io::Error),
    ReadCache(io::Error),
    DecodeCache(serde_json::Error),
    EncodeCache(serde_json::Error),
    WriteCache(io::Error),
    CurrentExecutable(io::Error),
    MissingInstallDirectory,
    MissingReleaseAsset {
        version: String,
        target: String,
    },
    CreateTempDir(io::Error),
    DownloadReleaseAsset(reqwest::Error),
    ReleaseAssetStatus {
        status: u16,
        body: String,
    },
    ReadReleaseAsset(reqwest::Error),
    DecodeReleaseAsset(std::string::FromUtf8Error),
    InvalidChecksum {
        message: String,
    },
    ChecksumMismatch {
        expected: String,
        actual: String,
    },
    WriteUpdateFile(io::Error),
    ReadUpdateFile(io::Error),
    DecodeCommandOutput(std::string::FromUtf8Error),
    UnsafeArchiveEntry {
        entry: String,
    },
    MissingArchiveBinary {
        path: String,
    },
    DuplicateArchiveBinary {
        path: String,
    },
    ArchiveBinaryNotRegular {
        path: String,
    },
    InstallDirectory(io::Error),
    InstallBinary(io::Error),
    InstallFailed {
        status: ExitStatus,
    },
    Command {
        operation: &'static str,
        source: io::Error,
    },
    CommandStatus {
        operation: &'static str,
        status: ExitStatus,
    },
}

impl fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(source) => write!(formatter, "{source}"),
            Self::UnsupportedTarget { os, arch } => {
                write!(formatter, "unsupported update target: {os}-{arch}")
            }
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
            Self::MissingReleaseAsset { version, target } => {
                write!(
                    formatter,
                    "release {version} does not include a {target} update archive"
                )
            }
            Self::CreateTempDir(source) => {
                write!(
                    formatter,
                    "could not create update work directory: {source}"
                )
            }
            Self::DownloadReleaseAsset(source) => {
                write!(formatter, "could not download release asset: {source}")
            }
            Self::ReleaseAssetStatus { status, body } if body.is_empty() => {
                write!(formatter, "release asset download returned HTTP {status}")
            }
            Self::ReleaseAssetStatus { status, body } => {
                write!(
                    formatter,
                    "release asset download returned HTTP {status}: {body}"
                )
            }
            Self::ReadReleaseAsset(source) => {
                write!(formatter, "could not read release asset: {source}")
            }
            Self::DecodeReleaseAsset(source) => {
                write!(formatter, "release asset was not valid UTF-8: {source}")
            }
            Self::InvalidChecksum { message } => write!(formatter, "invalid checksum: {message}"),
            Self::ChecksumMismatch { expected, actual } => write!(
                formatter,
                "release archive checksum mismatch: expected {expected}, got {actual}"
            ),
            Self::WriteUpdateFile(source) => {
                write!(formatter, "could not write update file: {source}")
            }
            Self::ReadUpdateFile(source) => {
                write!(formatter, "could not read update file: {source}")
            }
            Self::DecodeCommandOutput(source) => {
                write!(formatter, "command output was not valid UTF-8: {source}")
            }
            Self::UnsafeArchiveEntry { entry } => {
                write!(
                    formatter,
                    "release archive contained unsafe entry `{entry}`"
                )
            }
            Self::MissingArchiveBinary { path } => {
                write!(formatter, "release archive did not contain {path}")
            }
            Self::DuplicateArchiveBinary { path } => {
                write!(
                    formatter,
                    "release archive contained duplicate {path} entries"
                )
            }
            Self::ArchiveBinaryNotRegular { path } => {
                write!(
                    formatter,
                    "release archive binary was not a regular file: {path}"
                )
            }
            Self::InstallDirectory(source) => {
                write!(formatter, "could not create install directory: {source}")
            }
            Self::InstallBinary(source) => {
                write!(formatter, "could not install release binary: {source}")
            }
            Self::InstallFailed { status } => write!(formatter, "install failed with {status}"),
            Self::Command { operation, source } => {
                write!(
                    formatter,
                    "could not run command while {operation}: {source}"
                )
            }
            Self::CommandStatus { operation, status } => {
                write!(formatter, "command failed with {status} while {operation}")
            }
        }
    }
}

impl Error for UpdateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Config(source) => Some(source),
            Self::CheckRequest(source)
            | Self::DecodeRelease(source)
            | Self::DownloadReleaseAsset(source)
            | Self::ReadReleaseAsset(source) => Some(source),
            Self::Output(source)
            | Self::ReadCache(source)
            | Self::WriteCache(source)
            | Self::CurrentExecutable(source)
            | Self::CreateTempDir(source)
            | Self::WriteUpdateFile(source)
            | Self::ReadUpdateFile(source)
            | Self::InstallDirectory(source)
            | Self::InstallBinary(source) => Some(source),
            Self::DecodeCache(source) | Self::EncodeCache(source) => Some(source),
            Self::DecodeReleaseAsset(source) | Self::DecodeCommandOutput(source) => Some(source),
            Self::Command { source, .. } => Some(source),
            Self::UnsupportedTarget { .. }
            | Self::MissingReleaseAsset { .. }
            | Self::CheckStatus { .. }
            | Self::ReleaseAssetStatus { .. }
            | Self::MissingInstallDirectory
            | Self::InvalidChecksum { .. }
            | Self::ChecksumMismatch { .. }
            | Self::UnsafeArchiveEntry { .. }
            | Self::MissingArchiveBinary { .. }
            | Self::DuplicateArchiveBinary { .. }
            | Self::ArchiveBinaryNotRegular { .. }
            | Self::InstallFailed { .. }
            | Self::CommandStatus { .. } => None,
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
    use super::{
        LatestRelease, ReleaseAsset, UpdateCheck, clipped, compare_versions, parse_checksum,
        select_release_package, verify_sha256,
    };

    #[test]
    fn semantic_versions_compare_numerically() {
        assert!(compare_versions("v0.1.10", "v0.1.9").is_gt());
        assert!(compare_versions("v0.2.0", "v0.1.99").is_gt());
        assert!(compare_versions("v0.1.1", "v0.1.1").is_eq());
        assert!(compare_versions("v0.1", "v0.1.0").is_eq());
        assert!(compare_versions("v0.2.0", "v0.2.0-rc.1").is_gt());
        assert!(compare_versions("v0.2.0-rc.2", "v0.2.0-rc.1").is_gt());
        assert!(compare_versions("v0.2.0-rc.1", "v0.2.0").is_lt());
    }

    #[test]
    fn update_check_builds_notice_only_for_installable_newer_release() {
        let newer = UpdateCheck::new("v0.1.1", "v0.1.2", None, Some(package()));
        assert!(newer.update_available());
        assert!(newer.installable_update_available());
        assert_eq!(
            newer.notice().unwrap().message(),
            "Update available: v0.1.2 - run 'meshh update'"
        );

        let unavailable = UpdateCheck::new("v0.1.1", "v0.1.2", None, None);
        assert!(unavailable.update_available());
        assert!(!unavailable.installable_update_available());
        assert!(unavailable.notice().is_none());

        let current = UpdateCheck::new("v0.1.1", "v0.1.1", None, None);
        assert!(!current.update_available());
        assert!(!current.installable_update_available());
        assert!(current.notice().is_none());
    }

    #[test]
    fn cached_update_check_preserves_installable_notice_state() {
        let cached = super::cached_update_check(super::CachedUpdateCheck {
            latest_version: "v0.1.2".to_owned(),
            html_url: None,
            installable: true,
        });

        assert!(cached.update_available());
        assert!(cached.installable_update_available());
        assert!(cached.notice().is_some());
    }

    #[test]
    fn selects_current_target_release_package() {
        let target = super::release_target().unwrap();
        let version = "0.1.2";
        let archive_name = format!("meshh_{version}_{}.tar.gz", target.triple);
        let checksum_name = format!("{archive_name}.sha256");
        let release = LatestRelease {
            tag_name: format!("v{version}"),
            html_url: Some("https://github.com/meshhai/meshh-tui/releases/tag/v0.1.2".to_owned()),
            assets: vec![
                ReleaseAsset {
                    name: archive_name.clone(),
                    browser_download_url: format!("https://example.test/{archive_name}"),
                },
                ReleaseAsset {
                    name: checksum_name.clone(),
                    browser_download_url: format!("https://example.test/{checksum_name}"),
                },
            ],
        };

        let package = select_release_package(&release).unwrap();

        assert_eq!(package.archive_name, archive_name);
        assert_eq!(package.checksum_name, checksum_name);
        assert_eq!(
            package.archive_dir,
            format!("meshh_{version}_{}", target.triple)
        );
    }

    #[test]
    fn parses_and_verifies_release_checksum() {
        let archive_name = "meshh_0.1.2_aarch64-apple-darwin.tar.gz";
        let bytes = b"release archive";
        let checksum = format!("{}  {archive_name}\n", super::hex_sha256(bytes));

        let expected_hash = parse_checksum(&checksum, archive_name).unwrap();

        verify_sha256(bytes, &expected_hash).unwrap();
        assert!(verify_sha256(b"tampered", &expected_hash).is_err());
    }

    #[test]
    fn clipped_truncates_on_character_boundaries() {
        let value = "é".repeat(501);
        let clipped = clipped(&value);

        assert_eq!(clipped.chars().count(), 503);
        assert!(clipped.ends_with("..."));
    }

    #[cfg(unix)]
    #[test]
    fn temp_update_directory_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = super::TempDir::create("meshh-update-test").unwrap();
        let mode = std::fs::metadata(temp_dir.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(mode, 0o700);
    }

    fn package() -> super::ReleasePackage {
        super::ReleasePackage {
            archive_name: "meshh_0.1.2_aarch64-apple-darwin.tar.gz".to_owned(),
            archive_url: "https://example.test/archive.tar.gz".to_owned(),
            checksum_name: "meshh_0.1.2_aarch64-apple-darwin.tar.gz.sha256".to_owned(),
            checksum_url: "https://example.test/archive.tar.gz.sha256".to_owned(),
            archive_dir: "meshh_0.1.2_aarch64-apple-darwin".to_owned(),
        }
    }
}
