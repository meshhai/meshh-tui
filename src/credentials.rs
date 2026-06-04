use std::{error::Error, fmt};

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

/// Errors produced by credential validation or storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialError {
    EmptyToken,
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyToken => formatter.write_str("credential token cannot be empty"),
        }
    }
}

impl Error for CredentialError {}
