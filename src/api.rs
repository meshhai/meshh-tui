use std::{error::Error, fmt, future::Future, time::Duration};

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::config::RuntimeConfig;
use crate::credentials::BearerToken;

const DEVICE_AUTHORIZATION_PATH: &str = "/api/v1/tui/device-authorizations";
const DEVICE_TOKEN_PATH: &str = "/api/v1/tui/device-tokens";
const DEFAULT_DEVICE_POLL_INTERVAL_SECS: u64 = 5;

/// Opaque code used to poll for a device authorization result.
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceCode(String);

impl DeviceCode {
    /// Creates a device code, rejecting empty values.
    pub fn new(value: impl Into<String>) -> Result<Self, ApiError> {
        let value = value.into();

        if value.trim().is_empty() {
            return Err(ApiError::InvalidResponse {
                message: "device authorization response had an empty device code".to_owned(),
            });
        }

        Ok(Self(value))
    }

    /// Returns the code value to send back to the API while polling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for DeviceCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DeviceCode([redacted])")
    }
}

/// Short code the user types into the browser verification page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserCode(String);

impl UserCode {
    /// Creates a user code, rejecting empty values.
    pub fn new(value: impl Into<String>) -> Result<Self, ApiError> {
        let value = value.into();

        if value.trim().is_empty() {
            return Err(ApiError::InvalidResponse {
                message: "device authorization response had an empty user code".to_owned(),
            });
        }

        Ok(Self(value))
    }

    /// Returns the displayable user code.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Device authorization details returned before browser approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAuthorization {
    device_code: DeviceCode,
    user_code: UserCode,
    verification_url: String,
    verification_url_complete: Option<String>,
    expires_in: Duration,
    poll_interval: Duration,
}

impl DeviceAuthorization {
    /// Creates a validated device authorization value.
    pub fn new(
        device_code: DeviceCode,
        user_code: UserCode,
        verification_url: impl Into<String>,
        verification_url_complete: Option<String>,
        expires_in: Duration,
        poll_interval: Duration,
    ) -> Result<Self, ApiError> {
        let verification_url = verification_url.into();

        if verification_url.trim().is_empty() {
            return Err(ApiError::InvalidResponse {
                message: "device authorization response had an empty verification URL".to_owned(),
            });
        }

        if expires_in.is_zero() {
            return Err(ApiError::InvalidResponse {
                message: "device authorization response had a zero expiration".to_owned(),
            });
        }

        if poll_interval.is_zero() {
            return Err(ApiError::InvalidResponse {
                message: "device authorization response had a zero polling interval".to_owned(),
            });
        }

        Ok(Self {
            device_code,
            user_code,
            verification_url,
            verification_url_complete,
            expires_in,
            poll_interval,
        })
    }

    /// Returns the code used by the token polling endpoint.
    pub fn device_code(&self) -> &DeviceCode {
        &self.device_code
    }

    /// Returns the browser-visible user code.
    pub fn user_code(&self) -> &UserCode {
        &self.user_code
    }

    /// Returns the browser verification URL.
    pub fn verification_url(&self) -> &str {
        &self.verification_url
    }

    /// Returns the complete verification URL when the API provides one.
    pub fn verification_url_complete(&self) -> Option<&str> {
        self.verification_url_complete.as_deref()
    }

    /// Returns how long the device authorization remains valid.
    pub fn expires_in(&self) -> Duration {
        self.expires_in
    }

    /// Returns how long the client should wait between token polls.
    pub fn poll_interval(&self) -> Duration {
        self.poll_interval
    }
}

#[derive(Debug, Deserialize)]
struct DeviceAuthorizationResponse {
    device_code: String,
    user_code: String,
    #[serde(alias = "verification_uri")]
    verification_url: String,
    #[serde(default, alias = "verification_uri_complete")]
    verification_url_complete: Option<String>,
    expires_in: u64,
    #[serde(default = "default_device_poll_interval_secs", alias = "poll_interval")]
    interval: u64,
}

impl TryFrom<DeviceAuthorizationResponse> for DeviceAuthorization {
    type Error = ApiError;

    fn try_from(response: DeviceAuthorizationResponse) -> Result<Self, Self::Error> {
        Self::new(
            DeviceCode::new(response.device_code)?,
            UserCode::new(response.user_code)?,
            response.verification_url,
            response.verification_url_complete,
            Duration::from_secs(response.expires_in),
            Duration::from_secs(response.interval),
        )
    }
}

fn default_device_poll_interval_secs() -> u64 {
    DEFAULT_DEVICE_POLL_INTERVAL_SECS
}

/// Result returned by a device-token polling request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceTokenPoll {
    Pending,
    Approved { token: BearerToken },
    Denied,
    Expired,
    InvalidDeviceCode,
}

#[derive(Debug, Deserialize)]
struct DeviceTokenResponse {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    token: Option<String>,
}

impl DeviceTokenResponse {
    fn into_poll(self) -> Result<DeviceTokenPoll, ApiError> {
        let status = self
            .status
            .as_deref()
            .or(self.error.as_deref())
            .ok_or_else(|| ApiError::InvalidResponse {
                message: "device-token response did not include a status".to_owned(),
            })?;
        let normalized_status = status.trim().to_ascii_lowercase();

        match normalized_status.as_str() {
            "approved" | "complete" | "completed" => {
                let token =
                    self.access_token
                        .or(self.token)
                        .ok_or_else(|| ApiError::InvalidResponse {
                            message:
                                "approved device-token response did not include a bearer token"
                                    .to_owned(),
                        })?;

                BearerToken::new(token)
                    .map(|token| DeviceTokenPoll::Approved { token })
                    .map_err(|_| ApiError::InvalidResponse {
                        message: "approved device-token response had an empty bearer token"
                            .to_owned(),
                    })
            }
            "pending" | "authorization_pending" | "slow_down" => Ok(DeviceTokenPoll::Pending),
            "denied" | "access_denied" => Ok(DeviceTokenPoll::Denied),
            "expired" | "expired_token" => Ok(DeviceTokenPoll::Expired),
            "invalid_device_code" | "invalid_grant" => Ok(DeviceTokenPoll::InvalidDeviceCode),
            _ => Err(ApiError::InvalidResponse {
                message: format!(
                    "device-token response had an unknown status `{}`",
                    clipped(status)
                ),
            }),
        }
    }
}

/// API operations needed by the device login flow.
pub trait DeviceLoginApi {
    fn create_device_authorization(
        &self,
    ) -> impl Future<Output = Result<DeviceAuthorization, ApiError>> + Send;

    fn poll_device_token(
        &self,
        device_code: &DeviceCode,
    ) -> impl Future<Output = Result<DeviceTokenPoll, ApiError>> + Send;
}

/// Configuration needed by Meshh API clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiClientConfig {
    api_base_url: String,
}

impl ApiClientConfig {
    /// Creates API client configuration from an explicit API base URL.
    pub fn new(api_base_url: impl Into<String>) -> Result<Self, ApiError> {
        let api_base_url = api_base_url.into();

        if api_base_url.trim().is_empty() {
            return Err(ApiError::InvalidResponse {
                message: "API base URL cannot be empty".to_owned(),
            });
        }

        Ok(Self { api_base_url })
    }

    /// Creates API client configuration from runtime configuration.
    pub fn from_runtime(config: &RuntimeConfig) -> Self {
        Self::new(config.api_base_url()).expect("runtime API base URL must be validated")
    }

    /// Returns the resolved API base URL.
    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }
}

/// Meshh API client backed by an injectable HTTP transport.
#[derive(Debug, Clone)]
pub struct ApiClient<T = ReqwestTransport> {
    config: ApiClientConfig,
    transport: T,
}

impl ApiClient<ReqwestTransport> {
    /// Creates an API client using the default reqwest transport.
    pub fn new(config: ApiClientConfig) -> Self {
        Self::with_transport(config, ReqwestTransport::new())
    }
}

impl<T> ApiClient<T> {
    /// Creates an API client with an explicit HTTP transport.
    pub fn with_transport(config: ApiClientConfig, transport: T) -> Self {
        Self { config, transport }
    }

    /// Returns the client configuration.
    pub fn config(&self) -> &ApiClientConfig {
        &self.config
    }

    fn endpoint(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.config.api_base_url().trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}

impl<T> DeviceLoginApi for ApiClient<T>
where
    T: HttpTransport + Sync,
{
    async fn create_device_authorization(&self) -> Result<DeviceAuthorization, ApiError> {
        let transport = &self.transport;
        let url = self.endpoint(DEVICE_AUTHORIZATION_PATH);

        let response = transport
            .post_json(url, json!({}))
            .await
            .map_err(|source| ApiError::Transport { source })?;
        let response: DeviceAuthorizationResponse =
            parse_success_json(response, "creating a device authorization")?;

        response.try_into()
    }

    async fn poll_device_token(
        &self,
        device_code: &DeviceCode,
    ) -> Result<DeviceTokenPoll, ApiError> {
        let transport = &self.transport;
        let url = self.endpoint(DEVICE_TOKEN_PATH);
        let device_code = device_code.as_str().to_owned();

        let response = transport
            .post_json(url, json!({ "device_code": device_code }))
            .await
            .map_err(|source| ApiError::Transport { source })?;
        let response: DeviceTokenResponse = parse_success_json(response, "polling a device token")?;

        response.into_poll()
    }
}

/// Minimal HTTP transport surface needed by the Meshh API client.
pub trait HttpTransport {
    fn post_json(
        &self,
        url: String,
        body: Value,
    ) -> impl Future<Output = Result<HttpResponse, TransportError>> + Send;
}

/// HTTP response returned by an [`HttpTransport`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

impl HttpResponse {
    /// Creates a response value from a status and raw body.
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }

    /// Returns the HTTP status code.
    pub fn status(&self) -> u16 {
        self.status
    }

    /// Returns the raw response body.
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

/// Reqwest-backed HTTP transport used by the CLI.
#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    /// Creates a reqwest-backed transport with default client settings.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpTransport for ReqwestTransport {
    async fn post_json(&self, url: String, body: Value) -> Result<HttpResponse, TransportError> {
        let response = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|source| {
                TransportError::with_source("network error while sending request", source)
            })?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(|source| {
            TransportError::with_source("network error while reading response body", source)
        })?;

        Ok(HttpResponse::new(status, body.to_vec()))
    }
}

fn parse_success_json<T>(response: HttpResponse, operation: &'static str) -> Result<T, ApiError>
where
    T: DeserializeOwned,
{
    if !(200..300).contains(&response.status()) {
        return Err(ApiError::HttpStatus {
            operation,
            status: response.status(),
            body: clipped(&String::from_utf8_lossy(response.body())),
        });
    }

    serde_json::from_slice(response.body()).map_err(|source| ApiError::InvalidResponse {
        message: format!("could not decode Meshh API response while {operation}: {source}"),
    })
}

fn clipped(value: &str) -> String {
    value.chars().take(512).collect()
}

/// Errors produced by a lower-level HTTP transport.
#[derive(Debug)]
pub struct TransportError {
    message: String,
    source: Option<Box<dyn Error + Send + Sync + 'static>>,
}

impl TransportError {
    /// Creates a transport error with an underlying source error.
    pub fn with_source(
        message: impl Into<String>,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for TransportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

/// Errors produced by Meshh API calls or response decoding.
#[derive(Debug)]
pub enum ApiError {
    Transport {
        source: TransportError,
    },
    HttpStatus {
        operation: &'static str,
        status: u16,
        body: String,
    },
    InvalidResponse {
        message: String,
    },
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport { .. } => formatter.write_str("network error while contacting Meshh"),
            Self::HttpStatus {
                operation,
                status,
                body,
            } if body.is_empty() => {
                write!(
                    formatter,
                    "Meshh API returned HTTP {status} while {operation}"
                )
            }
            Self::HttpStatus {
                operation,
                status,
                body,
            } => {
                write!(
                    formatter,
                    "Meshh API returned HTTP {status} while {operation}: {body}"
                )
            }
            Self::InvalidResponse { message } => formatter.write_str(message),
        }
    }
}

impl Error for ApiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Transport { source } => Some(source),
            Self::HttpStatus { .. } | Self::InvalidResponse { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{DeviceAuthorization, DeviceTokenPoll, HttpResponse, parse_success_json};
    use crate::api::DeviceTokenResponse;

    #[test]
    fn decodes_device_authorization_response() {
        let response: super::DeviceAuthorizationResponse = parse_success_json(
            HttpResponse::new(
                200,
                br#"{
                    "device_code": "device-code",
                    "user_code": "ABCD-EFGH",
                    "verification_uri": "https://mesh.example/device",
                    "verification_uri_complete": "https://mesh.example/device?user_code=ABCD-EFGH",
                    "expires_in": 600,
                    "interval": 5
                }"#,
            ),
            "creating a device authorization",
        )
        .unwrap();
        let authorization = DeviceAuthorization::try_from(response).unwrap();

        assert_eq!(authorization.device_code().as_str(), "device-code");
        assert_eq!(authorization.user_code().as_str(), "ABCD-EFGH");
        assert_eq!(
            authorization.verification_url(),
            "https://mesh.example/device"
        );
        assert_eq!(
            authorization.verification_url_complete(),
            Some("https://mesh.example/device?user_code=ABCD-EFGH")
        );
        assert_eq!(authorization.expires_in(), Duration::from_secs(600));
        assert_eq!(authorization.poll_interval(), Duration::from_secs(5));
    }

    #[test]
    fn decodes_device_token_success_pending_denied_expired_and_invalid() {
        let approved = parse_poll_fixture(r#"{"status":"approved","access_token":"token"}"#);
        assert_eq!(
            approved,
            DeviceTokenPoll::Approved {
                token: crate::credentials::BearerToken::new("token").unwrap()
            }
        );
        assert_eq!(
            parse_poll_fixture(r#"{"status":"pending"}"#),
            DeviceTokenPoll::Pending
        );
        assert_eq!(
            parse_poll_fixture(r#"{"error":"authorization_pending"}"#),
            DeviceTokenPoll::Pending
        );
        assert_eq!(
            parse_poll_fixture(r#"{"status":"denied"}"#),
            DeviceTokenPoll::Denied
        );
        assert_eq!(
            parse_poll_fixture(r#"{"error":"access_denied"}"#),
            DeviceTokenPoll::Denied
        );
        assert_eq!(
            parse_poll_fixture(r#"{"status":"expired"}"#),
            DeviceTokenPoll::Expired
        );
        assert_eq!(
            parse_poll_fixture(r#"{"error":"expired_token"}"#),
            DeviceTokenPoll::Expired
        );
        assert_eq!(
            parse_poll_fixture(r#"{"status":"invalid_device_code"}"#),
            DeviceTokenPoll::InvalidDeviceCode
        );
    }

    fn parse_poll_fixture(json: &str) -> DeviceTokenPoll {
        let response: DeviceTokenResponse = parse_success_json(
            HttpResponse::new(200, json.as_bytes()),
            "polling a device token",
        )
        .unwrap();

        response.into_poll().unwrap()
    }
}
