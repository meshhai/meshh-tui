use std::{
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    string::FromUtf8Error,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAuditConfig {
    secret_needles: Vec<SensitiveNeedle>,
    internal_id_fields: Vec<String>,
    local_path_fragments: Vec<String>,
}

impl Default for ReleaseAuditConfig {
    fn default() -> Self {
        Self {
            secret_needles: default_secret_needles(),
            internal_id_fields: default_internal_id_fields(),
            local_path_fragments: default_local_path_fragments(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAuditViolation {
    path: PathBuf,
    line: usize,
    kind: ReleaseAuditViolationKind,
}

impl ReleaseAuditViolation {
    pub fn line(&self) -> usize {
        self.line
    }

    pub fn kind(&self) -> &ReleaseAuditViolationKind {
        &self.kind
    }
}

impl fmt::Display for ReleaseAuditViolation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}: {}",
            self.path.display(),
            self.line,
            self.kind
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseAuditViolationKind {
    SensitiveValue { label: String },
    InternalIdentifierField { field: String },
    LocalOnlyPath { fragment: String },
}

impl fmt::Display for ReleaseAuditViolationKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SensitiveValue { label } => write!(formatter, "sensitive value: {label}"),
            Self::InternalIdentifierField { field } => {
                write!(formatter, "internal identifier field: {field}")
            }
            Self::LocalOnlyPath { fragment } => {
                write!(formatter, "local-only path fragment: {fragment}")
            }
        }
    }
}

#[derive(Debug)]
pub enum ReleaseAuditError {
    ListTrackedFiles {
        repo_root: PathBuf,
        source: io::Error,
    },
    ListTrackedFilesFailed {
        repo_root: PathBuf,
        status: ExitStatus,
        stderr: String,
    },
    ListTrackedFilesUtf8 {
        repo_root: PathBuf,
        source: FromUtf8Error,
    },
    ReadTrackedFile {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for ReleaseAuditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ListTrackedFiles { repo_root, .. } => {
                write!(
                    formatter,
                    "failed to list tracked files in {}",
                    repo_root.display()
                )
            }
            Self::ListTrackedFilesFailed {
                repo_root,
                status,
                stderr,
            } => {
                write!(
                    formatter,
                    "listing tracked files in {} failed with {status}: {}",
                    repo_root.display(),
                    stderr.trim()
                )
            }
            Self::ListTrackedFilesUtf8 { repo_root, .. } => {
                write!(
                    formatter,
                    "tracked file list in {} was not valid UTF-8",
                    repo_root.display()
                )
            }
            Self::ReadTrackedFile { path, .. } => {
                write!(formatter, "failed to read tracked file {}", path.display())
            }
        }
    }
}

impl Error for ReleaseAuditError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ListTrackedFiles { source, .. } => Some(source),
            Self::ListTrackedFilesUtf8 { source, .. } => Some(source),
            Self::ReadTrackedFile { source, .. } => Some(source),
            Self::ListTrackedFilesFailed { .. } => None,
        }
    }
}

pub fn find_violations(
    repo_root: impl AsRef<Path>,
    config: &ReleaseAuditConfig,
) -> Result<Vec<ReleaseAuditViolation>, ReleaseAuditError> {
    let repo_root = repo_root.as_ref();
    let tracked_files = list_release_files(repo_root)?;
    let mut violations = Vec::new();

    for relative_path in tracked_files {
        let full_path = repo_root.join(&relative_path);
        let contents = fs::read_to_string(&full_path).map_err(|source| {
            ReleaseAuditError::ReadTrackedFile {
                path: relative_path.clone(),
                source,
            }
        })?;

        violations.extend(find_file_violations(config, &relative_path, &contents));
    }

    Ok(violations)
}

fn list_release_files(repo_root: &Path) -> Result<Vec<PathBuf>, ReleaseAuditError> {
    if repo_root.join(".git").exists() {
        list_git_tracked_files(repo_root)
    } else {
        list_filesystem_release_files(repo_root)
    }
}

fn list_git_tracked_files(repo_root: &Path) -> Result<Vec<PathBuf>, ReleaseAuditError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("ls-files")
        .arg("-z")
        .output()
        .map_err(|source| ReleaseAuditError::ListTrackedFiles {
            repo_root: repo_root.to_owned(),
            source,
        })?;

    if !output.status.success() {
        return Err(ReleaseAuditError::ListTrackedFilesFailed {
            repo_root: repo_root.to_owned(),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let stdout = String::from_utf8(output.stdout).map_err(|source| {
        ReleaseAuditError::ListTrackedFilesUtf8 {
            repo_root: repo_root.to_owned(),
            source,
        }
    })?;

    Ok(stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect())
}

fn list_filesystem_release_files(repo_root: &Path) -> Result<Vec<PathBuf>, ReleaseAuditError> {
    let mut files = Vec::new();
    collect_filesystem_release_files(repo_root, repo_root, &mut files)?;
    files.sort();

    Ok(files)
}

fn collect_filesystem_release_files(
    repo_root: &Path,
    dir: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), ReleaseAuditError> {
    let entries = fs::read_dir(dir).map_err(|source| ReleaseAuditError::ReadTrackedFile {
        path: dir.strip_prefix(repo_root).unwrap_or(dir).to_owned(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| ReleaseAuditError::ReadTrackedFile {
            path: dir.strip_prefix(repo_root).unwrap_or(dir).to_owned(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|source| ReleaseAuditError::ReadTrackedFile {
                path: path.strip_prefix(repo_root).unwrap_or(&path).to_owned(),
                source,
            })?;

        if file_type.is_dir() {
            if matches!(entry.file_name().to_str(), Some(".git") | Some("target")) {
                continue;
            }

            collect_filesystem_release_files(repo_root, &path, files)?;
        } else if file_type.is_file() {
            files.push(path.strip_prefix(repo_root).unwrap_or(&path).to_owned());
        }
    }

    Ok(())
}

fn find_file_violations(
    config: &ReleaseAuditConfig,
    path: &Path,
    contents: &str,
) -> Vec<ReleaseAuditViolation> {
    let mut violations = Vec::new();

    for (index, line) in contents.lines().enumerate() {
        let line_number = index + 1;

        for needle in &config.secret_needles {
            if line.contains(&needle.value) {
                violations.push(ReleaseAuditViolation {
                    path: path.to_owned(),
                    line: line_number,
                    kind: ReleaseAuditViolationKind::SensitiveValue {
                        label: needle.label.clone(),
                    },
                });
            }
        }

        if contains_aws_access_key(line) {
            violations.push(ReleaseAuditViolation {
                path: path.to_owned(),
                line: line_number,
                kind: ReleaseAuditViolationKind::SensitiveValue {
                    label: "AWS access key".to_owned(),
                },
            });
        }

        for field in &config.internal_id_fields {
            if contains_bounded_token(line, field) {
                violations.push(ReleaseAuditViolation {
                    path: path.to_owned(),
                    line: line_number,
                    kind: ReleaseAuditViolationKind::InternalIdentifierField {
                        field: field.clone(),
                    },
                });
            }
        }

        for fragment in &config.local_path_fragments {
            if line.contains(fragment) {
                violations.push(ReleaseAuditViolation {
                    path: path.to_owned(),
                    line: line_number,
                    kind: ReleaseAuditViolationKind::LocalOnlyPath {
                        fragment: fragment.clone(),
                    },
                });
            }
        }
    }

    violations
}

fn contains_aws_access_key(line: &str) -> bool {
    line.as_bytes().windows(20).any(|window| {
        (window.starts_with(b"AKIA") || window.starts_with(b"ASIA"))
            && window[4..]
                .iter()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    })
}

fn contains_bounded_token(line: &str, token: &str) -> bool {
    let haystack = line.as_bytes();
    let needle = token.as_bytes();

    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }

    haystack
        .windows(needle.len())
        .enumerate()
        .any(|(index, window)| {
            if window != needle {
                return false;
            }

            let before_is_boundary = index == 0 || !is_identifier_byte(haystack[index - 1]);
            let after_index = index + needle.len();
            let after_is_boundary =
                after_index == haystack.len() || !is_identifier_byte(haystack[after_index]);

            before_is_boundary && after_is_boundary
        })
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SensitiveNeedle {
    label: String,
    value: String,
}

impl SensitiveNeedle {
    fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
        }
    }
}

fn default_secret_needles() -> Vec<SensitiveNeedle> {
    vec![
        SensitiveNeedle::new("OpenAI-style API key", ["s", "k", "-"].concat()),
        SensitiveNeedle::new("GitHub classic token", ["gh", "p", "_"].concat()),
        SensitiveNeedle::new(
            "GitHub fine-grained token",
            ["github", "_pat", "_"].concat(),
        ),
        SensitiveNeedle::new("GitHub server token", ["gh", "s", "_"].concat()),
        SensitiveNeedle::new("GitHub OAuth token", ["gh", "o", "_"].concat()),
        SensitiveNeedle::new("Slack bot token", ["xox", "b", "-"].concat()),
        SensitiveNeedle::new("Slack user token", ["xox", "p", "-"].concat()),
        SensitiveNeedle::new(
            "private key block",
            ["-----BEGIN ", "PRIVATE KEY", "-----"].concat(),
        ),
        SensitiveNeedle::new(
            "authorization bearer header",
            ["Authorization", ": ", "Bearer", " "].concat(),
        ),
    ]
}

fn default_internal_id_fields() -> Vec<String> {
    [
        ["destination", "id"],
        ["route", "id"],
        ["user", "id"],
        ["workspace", "id"],
        ["organization", "id"],
        ["account", "id"],
        ["internal", "id"],
    ]
    .into_iter()
    .map(|parts| parts.join("_"))
    .collect()
}

fn default_local_path_fragments() -> Vec<String> {
    vec![
        ["/", "Users", "/"].concat(),
        ["/", "home", "/"].concat(),
        ["/", "Volumes", "/"].concat(),
        ["C:", "\\", "Users", "\\"].concat(),
        ["file:", "//", "/"].concat(),
    ]
}

#[cfg(test)]
mod tests {
    use super::{
        ReleaseAuditConfig, ReleaseAuditViolationKind, find_file_violations,
        list_filesystem_release_files,
    };
    use std::{
        fs,
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn internal_identifier_fields_must_be_bounded_tokens() {
        let config = ReleaseAuditConfig::default();
        let internal_field = ["destination", "id"].join("_");
        let contents = format!(
            "public_delivery_id is safe\n\
             internal_ids in prose are safe\n\
             {{\"{internal_field}\":\"dest_123\"}}\n"
        );

        let violations = find_file_violations(&config, Path::new("fixture.rs"), &contents);

        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].line(), 3);
        assert!(matches!(
            violations[0].kind(),
            ReleaseAuditViolationKind::InternalIdentifierField { field }
                if field == &internal_field
        ));
    }

    #[test]
    fn detects_sensitive_values_and_local_paths() {
        let config = ReleaseAuditConfig::default();
        let api_key = ["s", "k", "-", "example"].concat();
        let local_path = ["/", "Users", "/", "alice", "/", "mesh"].concat();
        let contents = format!("{api_key}\n{local_path}\n");

        let violations = find_file_violations(&config, Path::new("fixture.txt"), &contents);

        assert_eq!(violations.len(), 2);
        assert!(matches!(
            violations[0].kind(),
            ReleaseAuditViolationKind::SensitiveValue { .. }
        ));
        assert!(matches!(
            violations[1].kind(),
            ReleaseAuditViolationKind::LocalOnlyPath { .. }
        ));
    }

    #[test]
    fn filesystem_release_file_listing_does_not_require_git_checkout() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let repo_root = std::env::temp_dir().join(format!("meshh-tui-release-audit-{suffix}"));

        fs::create_dir_all(repo_root.join("src")).unwrap();
        fs::create_dir_all(repo_root.join("target/debug")).unwrap();
        fs::create_dir_all(repo_root.join(".git")).unwrap();
        fs::write(repo_root.join("Cargo.toml"), "[package]\n").unwrap();
        fs::write(repo_root.join("src/lib.rs"), "pub fn ok() {}\n").unwrap();
        fs::write(repo_root.join("target/debug/build.log"), "generated\n").unwrap();
        fs::write(repo_root.join(".git/config"), "private git data\n").unwrap();

        let files = list_filesystem_release_files(&repo_root).unwrap();

        assert_eq!(
            files,
            vec![
                Path::new("Cargo.toml").to_owned(),
                Path::new("src/lib.rs").to_owned()
            ]
        );

        fs::remove_dir_all(repo_root).unwrap();
    }
}
