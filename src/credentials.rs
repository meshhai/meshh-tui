use std::{
    error::Error,
    fmt, fs, io,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::config::{ConfigError, default_config_dir};

const CREDENTIALS_FILE_NAME: &str = "credentials.json";

/// Destination-scoped bearer token used by Meshh API calls.
#[derive(Clone, PartialEq, Eq)]
pub struct BearerToken(String);

impl BearerToken {
    /// Creates a token value, rejecting empty or whitespace-only input.
    pub fn new(value: impl Into<String>) -> Result<Self, CredentialError> {
        let value = value.into();

        if value.trim().is_empty() {
            return Err(CredentialError::EmptyToken);
        }

        Ok(Self(value))
    }

    /// Returns the token string for authenticated API calls.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BearerToken([redacted])")
    }
}

/// Storage interface for destination-scoped credentials.
pub trait CredentialStore {
    fn load_token(&self) -> Result<Option<BearerToken>, CredentialError>;
    fn save_token(&self, token: &BearerToken) -> Result<(), CredentialError>;
    fn delete_token(&self) -> Result<(), CredentialError>;
}

/// File-backed credential store used when no OS keychain backend is available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCredentialStore {
    path: PathBuf,
}

impl FileCredentialStore {
    /// Creates a credential store backed by an explicit file path.
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_owned(),
        }
    }

    /// Creates a credential store under the platform Meshh config directory.
    pub fn new_default() -> Result<Self, CredentialError> {
        Ok(Self::new(default_credentials_path()?))
    }

    /// Returns the backing file path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl CredentialStore for FileCredentialStore {
    fn load_token(&self) -> Result<Option<BearerToken>, CredentialError> {
        let contents = match fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(CredentialError::ReadTokenFile {
                    path: self.path.clone(),
                    source,
                });
            }
        };

        let stored: StoredCredentials =
            serde_json::from_str(&contents).map_err(|source| CredentialError::ParseTokenFile {
                path: self.path.clone(),
                source,
            })?;

        BearerToken::new(stored.token)
            .map(Some)
            .map_err(|_| CredentialError::InvalidStoredToken {
                path: self.path.clone(),
            })
    }

    fn save_token(&self, token: &BearerToken) -> Result<(), CredentialError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| {
                CredentialError::CreateCredentialDirectory {
                    path: parent.to_owned(),
                    source,
                }
            })?;
        }

        let contents = serde_json::to_vec(&StoredCredentials {
            token: token.as_str().to_owned(),
        })
        .expect("serializing credential JSON cannot fail");

        write_secret_file(&self.path, &contents).map_err(|source| CredentialError::WriteTokenFile {
            path: self.path.clone(),
            source,
        })
    }

    fn delete_token(&self) -> Result<(), CredentialError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(CredentialError::DeleteTokenFile {
                path: self.path.clone(),
                source,
            }),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct StoredCredentials {
    token: String,
}

/// Errors produced by credential validation or storage.
#[derive(Debug)]
pub enum CredentialError {
    EmptyToken,
    ConfigDirectory {
        source: ConfigError,
    },
    CreateCredentialDirectory {
        path: PathBuf,
        source: io::Error,
    },
    ReadTokenFile {
        path: PathBuf,
        source: io::Error,
    },
    WriteTokenFile {
        path: PathBuf,
        source: io::Error,
    },
    DeleteTokenFile {
        path: PathBuf,
        source: io::Error,
    },
    ParseTokenFile {
        path: PathBuf,
        source: serde_json::Error,
    },
    InvalidStoredToken {
        path: PathBuf,
    },
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyToken => formatter.write_str("credential token cannot be empty"),
            Self::ConfigDirectory { .. } => {
                formatter.write_str("could not determine credential directory")
            }
            Self::CreateCredentialDirectory { path, .. } => {
                write!(
                    formatter,
                    "could not create credential directory {}",
                    path.display()
                )
            }
            Self::ReadTokenFile { path, .. } => {
                write!(
                    formatter,
                    "could not read credential file {}",
                    path.display()
                )
            }
            Self::WriteTokenFile { path, .. } => {
                write!(
                    formatter,
                    "could not write credential file {}",
                    path.display()
                )
            }
            Self::DeleteTokenFile { path, .. } => {
                write!(
                    formatter,
                    "could not delete credential file {}",
                    path.display()
                )
            }
            Self::ParseTokenFile { path, .. } => {
                write!(
                    formatter,
                    "could not parse credential file {}",
                    path.display()
                )
            }
            Self::InvalidStoredToken { path } => {
                write!(
                    formatter,
                    "credential file {} has an invalid token",
                    path.display()
                )
            }
        }
    }
}

impl Error for CredentialError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ConfigDirectory { source } => Some(source),
            Self::CreateCredentialDirectory { source, .. }
            | Self::ReadTokenFile { source, .. }
            | Self::WriteTokenFile { source, .. }
            | Self::DeleteTokenFile { source, .. } => Some(source),
            Self::ParseTokenFile { source, .. } => Some(source),
            Self::EmptyToken | Self::InvalidStoredToken { .. } => None,
        }
    }
}

/// Returns the platform credentials file path used by Meshh.
pub fn default_credentials_path() -> Result<PathBuf, CredentialError> {
    default_config_dir()
        .map(|dir| dir.join(CREDENTIALS_FILE_NAME))
        .map_err(|source| CredentialError::ConfigDirectory { source })
}

#[cfg(unix)]
fn write_secret_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let temp_path = temporary_secret_file_path(path);

    let mut file = fs::OpenOptions::new()
        .create(true)
        .create_new(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&temp_path)?;

    file.write_all(contents)?;
    file.flush()?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp_path, path).inspect_err(|_source| {
        let _ = fs::remove_file(&temp_path);
    })?;

    Ok(())
}

#[cfg(not(unix))]
fn write_secret_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let temp_path = temporary_secret_file_path(path);
    let mut file = fs::OpenOptions::new()
        .create(true)
        .create_new(true)
        .truncate(false)
        .write(true)
        .open(&temp_path)?;

    file.write_all(contents)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    replace_secret_file(&temp_path, path)?;

    Ok(())
}

#[cfg(not(unix))]
fn replace_secret_file(temp_path: &Path, path: &Path) -> io::Result<()> {
    match fs::rename(temp_path, path) {
        Ok(()) => Ok(()),
        Err(first_error) if path.try_exists().unwrap_or(false) => {
            if let Err(remove_error) = fs::remove_file(path) {
                let _ = fs::remove_file(temp_path);
                return Err(remove_error);
            }

            fs::rename(temp_path, path).inspect_err(|_source| {
                let _ = fs::remove_file(temp_path);
                let _ = first_error;
            })
        }
        Err(source) => {
            let _ = fs::remove_file(temp_path);
            Err(source)
        }
    }
}

fn temporary_secret_file_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .unwrap_or("credentials");
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);

    parent.join(format!(".{file_name}.tmp-{}-{unique}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{BearerToken, CredentialStore, FileCredentialStore};

    #[test]
    fn file_store_saves_loads_and_deletes_token() {
        let path = temp_file_path("meshh-credentials", "json");
        let store = FileCredentialStore::new(&path);
        let token = BearerToken::new("secret-token").unwrap();

        assert!(store.load_token().unwrap().is_none());

        store.save_token(&token).unwrap();

        let loaded = store.load_token().unwrap().unwrap();
        assert_eq!(loaded.as_str(), "secret-token");

        store.delete_token().unwrap();

        assert!(store.load_token().unwrap().is_none());

        let _ = fs::remove_file(path);
    }

    #[cfg(not(unix))]
    #[test]
    fn file_store_overwrites_existing_token() {
        let path = temp_file_path("meshh-existing-credentials", "json");
        let store = FileCredentialStore::new(&path);
        let first_token = BearerToken::new("first-token").unwrap();
        let second_token = BearerToken::new("second-token").unwrap();

        store.save_token(&first_token).unwrap();
        store.save_token(&second_token).unwrap();

        assert_eq!(
            store.load_token().unwrap().unwrap().as_str(),
            "second-token"
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn bearer_token_debug_output_is_redacted() {
        let token = BearerToken::new("secret-token").unwrap();

        let debug = format!("{token:?}");

        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("secret-token"));
    }

    #[cfg(unix)]
    #[test]
    fn file_store_tightens_existing_file_permissions_before_replacing_token() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_file_path("meshh-existing-credentials", "json");
        fs::write(&path, r#"{"token":"old-token"}"#).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();

        let store = FileCredentialStore::new(&path);
        let token = BearerToken::new("replacement-token").unwrap();

        store.save_token(&token).unwrap();

        let loaded = store.load_token().unwrap().unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;

        assert_eq!(loaded.as_str(), "replacement-token");
        assert_eq!(mode, 0o600);

        let _ = fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn file_store_replaces_symlink_without_truncating_target() {
        use std::os::unix::fs::symlink;

        let path = temp_file_path("meshh-symlink-credentials", "json");
        let target = temp_file_path("meshh-symlink-target", "txt");
        fs::write(&target, "do not truncate").unwrap();
        symlink(&target, &path).unwrap();

        let store = FileCredentialStore::new(&path);
        let token = BearerToken::new("replacement-token").unwrap();

        store.save_token(&token).unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "do not truncate");
        assert!(
            !fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            store.load_token().unwrap().unwrap().as_str(),
            "replacement-token"
        );

        let _ = fs::remove_file(path);
        let _ = fs::remove_file(target);
    }

    fn temp_file_path(prefix: &str, extension: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        std::env::temp_dir().join(format!("{prefix}-{}.{extension}", unique))
    }
}
