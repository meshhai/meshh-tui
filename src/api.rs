use std::{env, error::Error, fmt, future::Future, time::Duration};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::config::RuntimeConfig;
use crate::credentials::BearerToken;

const DEVICE_AUTHORIZATION_PATH: &str = "/api/v1/tui/device-authorizations";
const DEVICE_TOKEN_PATH: &str = "/api/v1/tui/device-tokens";
const DELIVERY_LIST_PATH: &str = "/api/v1/destination/deliveries";
const DELIVERY_STREAM_PATH: &str = "/api/v1/destination/stream";
const CLIENT_NAME: &str = "meshh-tui";
const DEFAULT_DEVICE_AUTHORIZATION_EXPIRES_IN_SECS: u64 = 600;
const DEFAULT_DEVICE_POLL_INTERVAL_SECS: u64 = 5;
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Serialize)]
struct DeviceAuthorizationRequest {
    device_name: String,
    client_name: &'static str,
    client_version: &'static str,
    platform: String,
}

impl DeviceAuthorizationRequest {
    fn current_device() -> Self {
        Self {
            device_name: default_device_name(),
            client_name: CLIENT_NAME,
            client_version: env!("CARGO_PKG_VERSION"),
            platform: format!("{}-{}", env::consts::OS, env::consts::ARCH),
        }
    }
}

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
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default = "default_device_poll_interval_secs", alias = "poll_interval")]
    interval: u64,
}

impl TryFrom<DeviceAuthorizationResponse> for DeviceAuthorization {
    type Error = ApiError;

    fn try_from(response: DeviceAuthorizationResponse) -> Result<Self, Self::Error> {
        let expires_in = response.expires_in_secs()?;

        Self::new(
            DeviceCode::new(response.device_code)?,
            UserCode::new(response.user_code)?,
            response.verification_url,
            response.verification_url_complete,
            Duration::from_secs(expires_in),
            Duration::from_secs(response.interval),
        )
    }
}

impl DeviceAuthorizationResponse {
    fn expires_in_secs(&self) -> Result<u64, ApiError> {
        if let Some(expires_in) = self.expires_in {
            return Ok(expires_in);
        }

        if self
            .expires_at
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Ok(DEFAULT_DEVICE_AUTHORIZATION_EXPIRES_IN_SECS);
        }

        Err(ApiError::InvalidResponse {
            message: "device authorization response did not include expires_in or expires_at"
                .to_owned(),
        })
    }
}

fn default_device_poll_interval_secs() -> u64 {
    DEFAULT_DEVICE_POLL_INTERVAL_SECS
}

/// Result returned by a device-token polling request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceTokenPoll {
    Pending,
    SlowDown,
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
        let Self {
            status,
            error,
            access_token,
            token,
        } = self;
        let bearer_token = access_token.or(token);
        let Some(status) = status.as_deref().or(error.as_deref()) else {
            return bearer_token
                .ok_or_else(|| ApiError::InvalidResponse {
                    message: "device-token response did not include a status or bearer token"
                        .to_owned(),
                })
                .and_then(approved_device_token_poll);
        };
        let normalized_status = status.trim().to_ascii_lowercase();

        match normalized_status.as_str() {
            "approved" | "complete" | "completed" => {
                let token = bearer_token.ok_or_else(|| ApiError::InvalidResponse {
                    message: "approved device-token response did not include a bearer token"
                        .to_owned(),
                })?;

                approved_device_token_poll(token)
            }
            "pending" | "authorization_pending" => Ok(DeviceTokenPoll::Pending),
            "slow_down" => Ok(DeviceTokenPoll::SlowDown),
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

fn approved_device_token_poll(token: String) -> Result<DeviceTokenPoll, ApiError> {
    BearerToken::new(token)
        .map(|token| DeviceTokenPoll::Approved { token })
        .map_err(|_| ApiError::InvalidResponse {
            message: "approved device-token response had an empty bearer token".to_owned(),
        })
}

/// Public delivery identifier exposed by the Meshh destination API.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PublicDeliveryId(String);

impl PublicDeliveryId {
    /// Creates a public delivery ID, rejecting empty values and path separators.
    pub fn new(value: impl Into<String>) -> Result<Self, ApiError> {
        let value = value.into();

        if value.trim().is_empty() {
            return Err(ApiError::InvalidResponse {
                message: "delivery response had an empty public delivery ID".to_owned(),
            });
        }

        if value.contains(['/', '?', '#']) {
            return Err(ApiError::InvalidResponse {
                message: "public delivery ID cannot contain URL path separators".to_owned(),
            });
        }

        Ok(Self(value))
    }

    /// Returns the API-visible public delivery ID.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Cursor returned by list or stream responses for resume behavior.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StreamCursor(String);

impl StreamCursor {
    /// Creates a stream cursor, rejecting empty values.
    pub fn new(value: impl Into<String>) -> Result<Self, ApiError> {
        let value = value.into();

        if value.trim().is_empty() {
            return Err(ApiError::InvalidResponse {
                message: "delivery response had an empty stream cursor".to_owned(),
            });
        }

        Ok(Self(value))
    }

    /// Returns the cursor value to send back to the API.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Status label returned by the delivery API.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeliveryStatus(String);

impl DeliveryStatus {
    /// Creates a delivery status, rejecting empty values.
    pub fn new(value: impl Into<String>) -> Result<Self, ApiError> {
        let value = value.into();

        if value.trim().is_empty() {
            return Err(ApiError::InvalidResponse {
                message: "delivery response had an empty status".to_owned(),
            });
        }

        Ok(Self(value))
    }

    /// Returns the API status label.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Route that matched a delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedRoute {
    name: String,
}

impl MatchedRoute {
    /// Creates a matched route display value, rejecting empty names.
    pub fn new(name: impl Into<String>) -> Result<Self, ApiError> {
        let name = name.into();

        if name.trim().is_empty() {
            return Err(ApiError::InvalidResponse {
                message: "delivery response had an empty matched route name".to_owned(),
            });
        }

        Ok(Self { name })
    }

    /// Returns the displayable route name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A page of recent public deliveries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryListPage {
    items: Vec<DeliveryListItem>,
    next_cursor: Option<StreamCursor>,
}

impl DeliveryListPage {
    /// Creates a delivery list page.
    pub fn new(items: Vec<DeliveryListItem>, next_cursor: Option<StreamCursor>) -> Self {
        Self { items, next_cursor }
    }

    /// Returns the deliveries in API order.
    pub fn items(&self) -> &[DeliveryListItem] {
        &self.items
    }

    /// Returns the cursor for the next history page, when provided.
    pub fn next_cursor(&self) -> Option<&StreamCursor> {
        self.next_cursor.as_ref()
    }
}

/// Compact public delivery item used by list and stream responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryListItem {
    public_delivery_id: PublicDeliveryId,
    headline: String,
    source_context: Option<String>,
    status: DeliveryStatus,
    published_at: Option<String>,
    detected_at: Option<String>,
    cursor: Option<StreamCursor>,
}

impl DeliveryListItem {
    /// Creates a compact delivery item.
    pub fn new(
        public_delivery_id: PublicDeliveryId,
        headline: impl Into<String>,
        source_context: Option<String>,
        status: DeliveryStatus,
        published_at: Option<String>,
        detected_at: Option<String>,
        cursor: Option<StreamCursor>,
    ) -> Result<Self, ApiError> {
        let headline = headline.into();

        if headline.trim().is_empty() {
            return Err(ApiError::InvalidResponse {
                message: "delivery response had an empty headline".to_owned(),
            });
        }

        Ok(Self {
            public_delivery_id,
            headline,
            source_context: non_empty_optional(source_context),
            status,
            published_at: non_empty_optional(published_at),
            detected_at: non_empty_optional(detected_at),
            cursor,
        })
    }

    /// Returns the public delivery ID.
    pub fn public_delivery_id(&self) -> &PublicDeliveryId {
        &self.public_delivery_id
    }

    /// Returns the delivery headline.
    pub fn headline(&self) -> &str {
        &self.headline
    }

    /// Returns the source context shown in the list, when provided.
    pub fn source_context(&self) -> Option<&str> {
        self.source_context.as_deref()
    }

    /// Returns the API status label.
    pub fn status(&self) -> &DeliveryStatus {
        &self.status
    }

    /// Returns the API timestamp string for route-feed publication time, when provided.
    pub fn published_at(&self) -> Option<&str> {
        self.published_at.as_deref()
    }

    /// Returns the preferred timestamp for list display.
    pub fn display_timestamp(&self) -> Option<&str> {
        self.published_at().or_else(|| self.detected_at())
    }

    /// Returns the API timestamp string for detection time, when provided.
    pub fn detected_at(&self) -> Option<&str> {
        self.detected_at.as_deref()
    }

    /// Returns the cursor associated with this item, when provided.
    pub fn cursor(&self) -> Option<&StreamCursor> {
        self.cursor.as_ref()
    }
}

/// Full public delivery detail returned for a selected item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryDetail {
    public_delivery_id: PublicDeliveryId,
    headline: String,
    summary: Option<String>,
    body: Option<String>,
    source_url: Option<String>,
    source_context: Option<String>,
    status: DeliveryStatus,
    published_at: Option<String>,
    detected_at: Option<String>,
    matched_routes: Vec<MatchedRoute>,
    cursor: Option<StreamCursor>,
}

impl DeliveryDetail {
    /// Creates a full delivery detail value.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        public_delivery_id: PublicDeliveryId,
        headline: impl Into<String>,
        summary: Option<String>,
        body: Option<String>,
        source_url: Option<String>,
        source_context: Option<String>,
        status: DeliveryStatus,
        published_at: Option<String>,
        detected_at: Option<String>,
        matched_routes: Vec<MatchedRoute>,
        cursor: Option<StreamCursor>,
    ) -> Result<Self, ApiError> {
        let headline = headline.into();

        if headline.trim().is_empty() {
            return Err(ApiError::InvalidResponse {
                message: "delivery response had an empty headline".to_owned(),
            });
        }

        Ok(Self {
            public_delivery_id,
            headline,
            summary: non_empty_optional(summary),
            body: non_empty_optional(body),
            source_url: non_empty_optional(source_url),
            source_context: non_empty_optional(source_context),
            status,
            published_at: non_empty_optional(published_at),
            detected_at: non_empty_optional(detected_at),
            matched_routes,
            cursor,
        })
    }

    /// Returns the public delivery ID.
    pub fn public_delivery_id(&self) -> &PublicDeliveryId {
        &self.public_delivery_id
    }

    /// Returns the delivery headline.
    pub fn headline(&self) -> &str {
        &self.headline
    }

    /// Returns the delivery summary, when provided.
    pub fn summary(&self) -> Option<&str> {
        self.summary.as_deref()
    }

    /// Returns the full delivery body, when provided.
    pub fn body(&self) -> Option<&str> {
        self.body.as_deref()
    }

    /// Returns the source URL, when provided.
    pub fn source_url(&self) -> Option<&str> {
        self.source_url.as_deref()
    }

    /// Returns the source context shown in the list, when provided.
    pub fn source_context(&self) -> Option<&str> {
        self.source_context.as_deref()
    }

    /// Returns the API status label.
    pub fn status(&self) -> &DeliveryStatus {
        &self.status
    }

    /// Returns the API timestamp string for route-feed publication time, when provided.
    pub fn published_at(&self) -> Option<&str> {
        self.published_at.as_deref()
    }

    /// Returns the preferred timestamp for detail display.
    pub fn display_timestamp(&self) -> Option<&str> {
        self.published_at().or_else(|| self.detected_at())
    }

    /// Returns the API timestamp string for detection time, when provided.
    pub fn detected_at(&self) -> Option<&str> {
        self.detected_at.as_deref()
    }

    /// Returns matched route display values.
    pub fn matched_routes(&self) -> &[MatchedRoute] {
        &self.matched_routes
    }

    /// Returns the cursor associated with this item, when provided.
    pub fn cursor(&self) -> Option<&StreamCursor> {
        self.cursor.as_ref()
    }
}

/// Parsed server-sent stream frame from the delivery stream endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryStreamFrame {
    Connected,
    Cursor(StreamCursor),
    Delivery {
        cursor: Option<StreamCursor>,
        item: DeliveryListItem,
    },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DeliveryListEnvelope {
    Page(DeliveryListResponse),
    Items(Vec<DeliveryResponse>),
}

impl TryFrom<DeliveryListEnvelope> for DeliveryListPage {
    type Error = ApiError;

    fn try_from(response: DeliveryListEnvelope) -> Result<Self, Self::Error> {
        match response {
            DeliveryListEnvelope::Page(response) => response.try_into(),
            DeliveryListEnvelope::Items(items) => {
                let items = items
                    .into_iter()
                    .map(DeliveryResponse::into_list_item)
                    .collect::<Result<Vec<_>, _>>()?;

                Ok(DeliveryListPage::new(items, None))
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DeliveryDetailEnvelope {
    Item(DeliveryResponse),
    Data { data: DeliveryResponse },
}

impl DeliveryDetailEnvelope {
    fn into_detail(self) -> Result<DeliveryDetail, ApiError> {
        match self {
            Self::Item(response) | Self::Data { data: response } => response.into_detail(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct DeliveryListResponse {
    #[serde(alias = "items", alias = "data")]
    deliveries: Vec<DeliveryResponse>,
    #[serde(default, alias = "nextCursor")]
    next_cursor: Option<String>,
}

impl TryFrom<DeliveryListResponse> for DeliveryListPage {
    type Error = ApiError;

    fn try_from(response: DeliveryListResponse) -> Result<Self, Self::Error> {
        let items = response
            .deliveries
            .into_iter()
            .map(DeliveryResponse::into_list_item)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = response.next_cursor.map(StreamCursor::new).transpose()?;

        Ok(DeliveryListPage::new(items, next_cursor))
    }
}

#[derive(Debug, Deserialize)]
struct DeliveryResponse {
    #[serde(alias = "publicDeliveryId")]
    public_delivery_id: String,
    #[serde(alias = "title")]
    headline: String,
    #[serde(
        default,
        alias = "sourceContext",
        alias = "source",
        alias = "sourceName"
    )]
    source_context: Option<String>,
    status: String,
    #[serde(default, alias = "publishedAt")]
    published_at: Option<String>,
    #[serde(default, alias = "detectedAt")]
    detected_at: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default, alias = "sourceUrl")]
    source_url: Option<String>,
    #[serde(default, alias = "matchedRoutes")]
    matched_routes: Vec<MatchedRouteResponse>,
}

impl DeliveryResponse {
    fn into_list_item(self) -> Result<DeliveryListItem, ApiError> {
        DeliveryListItem::new(
            PublicDeliveryId::new(self.public_delivery_id)?,
            self.headline,
            self.source_context,
            DeliveryStatus::new(self.status)?,
            self.published_at,
            self.detected_at,
            self.cursor.map(StreamCursor::new).transpose()?,
        )
    }

    fn into_detail(self) -> Result<DeliveryDetail, ApiError> {
        let matched_routes = self
            .matched_routes
            .into_iter()
            .map(MatchedRouteResponse::try_into_route)
            .collect::<Result<Vec<_>, _>>()?;

        DeliveryDetail::new(
            PublicDeliveryId::new(self.public_delivery_id)?,
            self.headline,
            self.summary,
            self.body,
            self.source_url,
            self.source_context,
            DeliveryStatus::new(self.status)?,
            self.published_at,
            self.detected_at,
            matched_routes,
            self.cursor.map(StreamCursor::new).transpose()?,
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum MatchedRouteResponse {
    Name(String),
    Object {
        name: Option<String>,
        label: Option<String>,
        title: Option<String>,
    },
}

impl MatchedRouteResponse {
    fn try_into_route(self) -> Result<MatchedRoute, ApiError> {
        match self {
            Self::Name(name) => MatchedRoute::new(name),
            Self::Object { name, label, title } => {
                let name = name
                    .or(label)
                    .or(title)
                    .ok_or_else(|| ApiError::InvalidResponse {
                        message: "matched route object did not include a display name".to_owned(),
                    })?;

                MatchedRoute::new(name)
            }
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

/// API operations needed by the delivery list, detail, and stream flows.
pub trait DeliveryApi {
    fn list_deliveries(
        &self,
        token: &BearerToken,
    ) -> impl Future<Output = Result<DeliveryListPage, ApiError>> + Send;

    fn get_delivery(
        &self,
        token: &BearerToken,
        public_delivery_id: &PublicDeliveryId,
    ) -> impl Future<Output = Result<DeliveryDetail, ApiError>> + Send;

    fn stream_deliveries(
        &self,
        token: &BearerToken,
        after: Option<&StreamCursor>,
        on_frame: impl FnMut(DeliveryStreamFrame) -> Result<(), ApiError> + Send,
    ) -> impl Future<Output = Result<(), ApiError>> + Send;
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

    fn delivery_detail_endpoint(&self, public_delivery_id: &PublicDeliveryId) -> String {
        format!(
            "{}/{}",
            self.endpoint(DELIVERY_LIST_PATH),
            public_delivery_id.as_str()
        )
    }

    fn delivery_stream_endpoint(&self, after: Option<&StreamCursor>) -> Result<String, ApiError> {
        let mut url =
            reqwest::Url::parse(&self.endpoint(DELIVERY_STREAM_PATH)).map_err(|source| {
                ApiError::InvalidResponse {
                    message: format!(
                        "API base URL could not be parsed for stream endpoint: {source}"
                    ),
                }
            })?;

        if let Some(after) = after {
            url.query_pairs_mut().append_pair("after", after.as_str());
        }

        Ok(url.to_string())
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
            .post_json(
                url,
                serde_json::to_value(DeviceAuthorizationRequest::current_device()).expect(
                    "serializing device authorization request cannot fail for primitive fields",
                ),
            )
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

        parse_device_token_poll_response(response)
    }
}

impl<T> DeliveryApi for ApiClient<T>
where
    T: HttpTransport + Sync,
{
    async fn list_deliveries(&self, token: &BearerToken) -> Result<DeliveryListPage, ApiError> {
        let response = self
            .transport
            .get_bearer(self.endpoint(DELIVERY_LIST_PATH), token.clone())
            .await
            .map_err(|source| ApiError::Transport { source })?;
        let response: DeliveryListEnvelope =
            parse_authenticated_success_json(response, "listing deliveries")?;

        response.try_into()
    }

    async fn get_delivery(
        &self,
        token: &BearerToken,
        public_delivery_id: &PublicDeliveryId,
    ) -> Result<DeliveryDetail, ApiError> {
        let response = self
            .transport
            .get_bearer(
                self.delivery_detail_endpoint(public_delivery_id),
                token.clone(),
            )
            .await
            .map_err(|source| ApiError::Transport { source })?;
        let response: DeliveryDetailEnvelope =
            parse_authenticated_success_json(response, "loading delivery detail")?;

        response.into_detail()
    }

    async fn stream_deliveries(
        &self,
        token: &BearerToken,
        after: Option<&StreamCursor>,
        mut on_frame: impl FnMut(DeliveryStreamFrame) -> Result<(), ApiError> + Send,
    ) -> Result<(), ApiError> {
        let mut parser = DeliveryStreamParser::default();
        let mut stream_error = None;
        let response = self
            .transport
            .get_bearer_stream(
                self.delivery_stream_endpoint(after)?,
                token.clone(),
                |chunk| {
                    let frames = match parser.push_chunk(chunk) {
                        Ok(frames) => frames,
                        Err(error) => {
                            stream_error = Some(error);
                            return Err(TransportError::new("invalid delivery stream response"));
                        }
                    };

                    if frames.is_empty()
                        && let Err(error) = on_frame(DeliveryStreamFrame::Connected)
                    {
                        stream_error = Some(error);
                        return Err(TransportError::new("delivery stream frame handler failed"));
                    }

                    for frame in frames {
                        if let Err(error) = on_frame(frame) {
                            stream_error = Some(error);
                            return Err(TransportError::new(
                                "delivery stream frame handler failed",
                            ));
                        }
                    }

                    Ok(())
                },
            )
            .await;

        if let Some(error) = stream_error {
            return Err(error);
        }

        let response = response.map_err(|source| ApiError::Transport { source })?;
        parse_authenticated_success_body(response, "streaming deliveries")?;

        for frame in parser.finish()? {
            on_frame(frame)?;
        }

        Ok(())
    }
}

/// Minimal HTTP transport surface needed by the Meshh API client.
pub trait HttpTransport {
    fn post_json(
        &self,
        url: String,
        body: Value,
    ) -> impl Future<Output = Result<HttpResponse, TransportError>> + Send;

    fn get_bearer(
        &self,
        url: String,
        token: BearerToken,
    ) -> impl Future<Output = Result<HttpResponse, TransportError>> + Send {
        async move {
            let _ = (url, token);

            Err(TransportError::new(
                "authenticated GET is not supported by this transport",
            ))
        }
    }

    fn get_bearer_stream(
        &self,
        url: String,
        token: BearerToken,
        mut on_chunk: impl FnMut(&[u8]) -> Result<(), TransportError> + Send,
    ) -> impl Future<Output = Result<HttpResponse, TransportError>> + Send
    where
        Self: Sync,
    {
        async move {
            let response = self.get_bearer(url, token).await?;

            if (200..300).contains(&response.status()) && !response.body().is_empty() {
                on_chunk(response.body())?;

                return Ok(HttpResponse::new(response.status(), Vec::new()));
            }

            Ok(response)
        }
    }
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
    request_timeout: Duration,
}

impl ReqwestTransport {
    /// Creates a reqwest-backed transport with default client settings.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
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
            .timeout(self.request_timeout)
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

    async fn get_bearer(
        &self,
        url: String,
        token: BearerToken,
    ) -> Result<HttpResponse, TransportError> {
        let response = self
            .client
            .get(url)
            .timeout(self.request_timeout)
            .bearer_auth(token.as_str())
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

    async fn get_bearer_stream(
        &self,
        url: String,
        token: BearerToken,
        mut on_chunk: impl FnMut(&[u8]) -> Result<(), TransportError> + Send,
    ) -> Result<HttpResponse, TransportError> {
        let mut response = self
            .client
            .get(url)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .bearer_auth(token.as_str())
            .send()
            .await
            .map_err(|source| {
                TransportError::with_source("network error while sending request", source)
            })?;
        let status = response.status().as_u16();

        if !(200..300).contains(&status) {
            let body = response.bytes().await.map_err(|source| {
                TransportError::with_source("network error while reading response body", source)
            })?;

            return Ok(HttpResponse::new(status, body.to_vec()));
        }

        on_chunk(&[])?;

        while let Some(chunk) = response.chunk().await.map_err(|source| {
            TransportError::with_source("network error while reading response body", source)
        })? {
            if !chunk.is_empty() {
                on_chunk(&chunk)?;
            }
        }

        Ok(HttpResponse::new(status, Vec::new()))
    }
}

fn parse_success_json<T>(response: HttpResponse, operation: &'static str) -> Result<T, ApiError>
where
    T: DeserializeOwned,
{
    if !(200..300).contains(&response.status()) {
        return Err(http_status_error(&response, operation));
    }

    parse_json_body(&response, operation)
}

fn parse_authenticated_success_json<T>(
    response: HttpResponse,
    operation: &'static str,
) -> Result<T, ApiError>
where
    T: DeserializeOwned,
{
    let response = parse_authenticated_success_body(response, operation)?;

    serde_json::from_slice(&response).map_err(|source| ApiError::InvalidResponse {
        message: format!("could not decode Meshh API response while {operation}: {source}"),
    })
}

fn parse_authenticated_success_body(
    response: HttpResponse,
    operation: &'static str,
) -> Result<Vec<u8>, ApiError> {
    if matches!(response.status(), 401 | 403) {
        return Err(authentication_error(&response, operation));
    }

    if !(200..300).contains(&response.status()) {
        return Err(http_status_error(&response, operation));
    }

    Ok(response.body().to_vec())
}

fn parse_device_token_poll_response(response: HttpResponse) -> Result<DeviceTokenPoll, ApiError> {
    const OPERATION: &str = "polling a device token";

    if (200..300).contains(&response.status()) {
        let response: DeviceTokenResponse = parse_json_body(&response, OPERATION)?;

        return response.into_poll();
    }

    if (400..500).contains(&response.status())
        && let Ok(response_body) = serde_json::from_slice::<DeviceTokenResponse>(response.body())
        && let Ok(poll) = response_body.into_poll()
    {
        match &poll {
            DeviceTokenPoll::Pending
            | DeviceTokenPoll::SlowDown
            | DeviceTokenPoll::Denied
            | DeviceTokenPoll::Expired
            | DeviceTokenPoll::InvalidDeviceCode => return Ok(poll),
            DeviceTokenPoll::Approved { .. } => {}
        }
    }

    Err(http_status_error(&response, OPERATION))
}

fn parse_json_body<T>(response: &HttpResponse, operation: &'static str) -> Result<T, ApiError>
where
    T: DeserializeOwned,
{
    serde_json::from_slice(response.body()).map_err(|source| ApiError::InvalidResponse {
        message: format!("could not decode Meshh API response while {operation}: {source}"),
    })
}

fn http_status_error(response: &HttpResponse, operation: &'static str) -> ApiError {
    ApiError::HttpStatus {
        operation,
        status: response.status(),
        body: clipped(&String::from_utf8_lossy(response.body())),
    }
}

fn authentication_error(response: &HttpResponse, operation: &'static str) -> ApiError {
    ApiError::Authentication {
        operation,
        status: response.status(),
        body: clipped(&String::from_utf8_lossy(response.body())),
    }
}

fn clipped(value: &str) -> String {
    value.chars().take(512).collect()
}

fn non_empty_optional(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn default_device_name() -> String {
    env::var("MESHH_TUI_DEVICE_NAME")
        .ok()
        .and_then(non_empty_string)
        .or_else(|| env::var("HOSTNAME").ok().and_then(non_empty_string))
        .or_else(|| env::var("COMPUTERNAME").ok().and_then(non_empty_string))
        .unwrap_or_else(|| CLIENT_NAME.to_owned())
}

fn non_empty_string(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

/// Parses server-sent events returned by the delivery stream endpoint.
pub fn parse_delivery_stream_frames(body: &[u8]) -> Result<Vec<DeliveryStreamFrame>, ApiError> {
    let mut parser = DeliveryStreamParser::default();
    let mut frames = Vec::new();

    frames.extend(parser.push_chunk(body)?);
    frames.extend(parser.finish()?);

    Ok(frames)
}

#[derive(Debug, Default)]
struct DeliveryStreamParser {
    buffer: Vec<u8>,
}

impl DeliveryStreamParser {
    fn push_chunk(&mut self, chunk: &[u8]) -> Result<Vec<DeliveryStreamFrame>, ApiError> {
        self.buffer.extend_from_slice(chunk);
        let mut frames = Vec::new();

        while let Some((index, separator_len)) = next_sse_message_separator(&self.buffer) {
            let block = self.buffer[..index].to_vec();
            self.buffer.drain(..index + separator_len);

            if let Some(frame) = parse_sse_message_bytes(&block)? {
                frames.push(frame);
            }
        }

        Ok(frames)
    }

    fn finish(&mut self) -> Result<Vec<DeliveryStreamFrame>, ApiError> {
        if self.buffer.is_empty() {
            return Ok(Vec::new());
        }

        let block = self.buffer.split_off(0);

        Ok(parse_sse_message_bytes(&block)?.into_iter().collect())
    }
}

fn next_sse_message_separator(buffer: &[u8]) -> Option<(usize, usize)> {
    [
        b"\r\n\r\n".as_slice(),
        b"\n\n".as_slice(),
        b"\r\r".as_slice(),
    ]
    .into_iter()
    .filter_map(|separator| {
        buffer
            .windows(separator.len())
            .position(|window| window == separator)
            .map(|index| (index, separator.len()))
    })
    .min_by_key(|(index, _)| *index)
}

#[derive(Debug)]
struct SseMessage {
    event: Option<String>,
    id: Option<String>,
    data: String,
}

fn parse_sse_message_bytes(block: &[u8]) -> Result<Option<DeliveryStreamFrame>, ApiError> {
    let text = std::str::from_utf8(block).map_err(|source| ApiError::InvalidResponse {
        message: format!("delivery stream response was not UTF-8: {source}"),
    })?;
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let Some(message) = parse_sse_message_block(&normalized) else {
        return Ok(None);
    };
    let event = message.event.as_deref().unwrap_or("message").to_owned();

    if message.data.trim().is_empty() {
        return Ok(None);
    }

    match event.as_str() {
        "cursor" => Ok(Some(DeliveryStreamFrame::Cursor(parse_cursor_event_data(
            &message.data,
        )?))),
        "delivery" | "message" => Ok(Some(parse_delivery_stream_frame(message)?)),
        "heartbeat" | "ping" => Ok(None),
        _ => Err(ApiError::InvalidResponse {
            message: format!(
                "delivery stream response had an unsupported event `{}`",
                clipped(&event)
            ),
        }),
    }
}

fn parse_sse_message_block(block: &str) -> Option<SseMessage> {
    let mut event = None;
    let mut id = None;
    let mut data = String::new();

    for line in block.lines() {
        if line.starts_with(':') {
            continue;
        }

        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);

        match field {
            "event" if !value.trim().is_empty() => event = Some(value.to_owned()),
            "id" if !value.trim().is_empty() => id = Some(value.to_owned()),
            "data" => {
                if !data.is_empty() {
                    data.push('\n');
                }

                data.push_str(value);
            }
            _ => {}
        }
    }

    if event.is_none() && id.is_none() && data.is_empty() {
        None
    } else {
        Some(SseMessage { event, id, data })
    }
}

fn parse_cursor_event_data(data: &str) -> Result<StreamCursor, ApiError> {
    let trimmed = data.trim();

    if trimmed.starts_with('{') {
        let value =
            serde_json::from_str::<Value>(trimmed).map_err(|source| ApiError::InvalidResponse {
                message: format!("could not decode delivery cursor stream event: {source}"),
            })?;
        let cursor = value.get("cursor").and_then(Value::as_str).ok_or_else(|| {
            ApiError::InvalidResponse {
                message: "delivery cursor stream event did not include a cursor".to_owned(),
            }
        })?;

        return StreamCursor::new(cursor.to_owned());
    }

    if trimmed.starts_with('"') {
        let cursor = serde_json::from_str::<String>(trimmed).map_err(|source| {
            ApiError::InvalidResponse {
                message: format!("could not decode delivery cursor stream event: {source}"),
            }
        })?;

        return StreamCursor::new(cursor);
    }

    StreamCursor::new(trimmed.to_owned())
}

fn parse_delivery_stream_frame(message: SseMessage) -> Result<DeliveryStreamFrame, ApiError> {
    let value = serde_json::from_str::<Value>(&message.data).map_err(|source| {
        ApiError::InvalidResponse {
            message: format!("could not decode delivery stream event: {source}"),
        }
    })?;
    let cursor = value
        .get("cursor")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or(message.id);
    let delivery_value = value.get("delivery").unwrap_or(&value);
    let response =
        serde_json::from_value::<DeliveryResponse>(delivery_value.clone()).map_err(|source| {
            ApiError::InvalidResponse {
                message: format!("could not decode delivery stream event item: {source}"),
            }
        })?;
    let item = response.into_list_item()?;
    let cursor = cursor
        .map(StreamCursor::new)
        .transpose()?
        .or_else(|| item.cursor().cloned());

    Ok(DeliveryStreamFrame::Delivery { cursor, item })
}

/// Errors produced by a lower-level HTTP transport.
#[derive(Debug)]
pub struct TransportError {
    message: String,
    source: Option<Box<dyn Error + Send + Sync + 'static>>,
}

impl TransportError {
    /// Creates a transport error without an underlying source.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

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
    Authentication {
        operation: &'static str,
        status: u16,
        body: String,
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
            Self::Authentication {
                operation, body, ..
            } if body.is_empty() => {
                write!(
                    formatter,
                    "Meshh API rejected the stored token while {operation}"
                )
            }
            Self::Authentication {
                operation, body, ..
            } => {
                write!(
                    formatter,
                    "Meshh API rejected the stored token while {operation}: {body}"
                )
            }
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
            Self::Authentication { .. }
            | Self::HttpStatus { .. }
            | Self::InvalidResponse { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use serde_json::Value;

    use super::{
        ApiClient, ApiClientConfig, ApiError, DeliveryApi, DeliveryStreamFrame,
        DeviceAuthorization, DeviceLoginApi, DeviceTokenPoll, HttpResponse, HttpTransport,
        PublicDeliveryId, ReqwestTransport, StreamCursor, TransportError,
        parse_delivery_stream_frames, parse_device_token_poll_response, parse_success_json,
    };
    use crate::api::{DeviceCode, DeviceTokenResponse};

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
    fn decodes_mesh_device_authorization_response_with_expires_at() {
        let response: super::DeviceAuthorizationResponse = parse_success_json(
            HttpResponse::new(
                201,
                br#"{
                    "device_code": "device-code",
                    "user_code": "ABCD-EFGH",
                    "verification_uri": "https://mesh.example/en/tui/login",
                    "verification_uri_complete": "https://mesh.example/en/tui/login?code=ABCD-EFGH",
                    "expires_at": "2026-06-04T09:29:52Z",
                    "interval": 5
                }"#,
            ),
            "creating a device authorization",
        )
        .unwrap();
        let authorization = DeviceAuthorization::try_from(response).unwrap();

        assert_eq!(authorization.device_code().as_str(), "device-code");
        assert_eq!(
            authorization.verification_url(),
            "https://mesh.example/en/tui/login"
        );
        assert_eq!(authorization.expires_in(), Duration::from_secs(600));
        assert_eq!(authorization.poll_interval(), Duration::from_secs(5));
    }

    #[tokio::test]
    async fn create_device_authorization_sends_public_device_metadata() {
        let observed_requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let client = ApiClient::with_transport(
            ApiClientConfig::new("https://mesh.example").unwrap(),
            ObservedPostTransport {
                response: HttpResponse::new(
                    201,
                    br#"{
                        "device_code": "device-code",
                        "user_code": "ABCD-EFGH",
                        "verification_uri": "https://mesh.example/device",
                        "expires_in": 600,
                        "interval": 5
                    }"#,
                ),
                observed_requests: observed_requests.clone(),
            },
        );

        let authorization = client.create_device_authorization().await.unwrap();

        assert_eq!(authorization.device_code().as_str(), "device-code");

        let requests = observed_requests.lock().unwrap();
        let request = requests.first().unwrap();
        assert_eq!(
            request.url,
            "https://mesh.example/api/v1/tui/device-authorizations"
        );
        assert_eq!(request.body["client_name"], "meshh-tui");
        assert_eq!(request.body["client_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            request.body["platform"],
            format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
        );
        assert!(
            request
                .body
                .get("device_name")
                .and_then(Value::as_str)
                .is_some_and(|device_name| !device_name.trim().is_empty())
        );
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
            parse_poll_fixture(
                r#"{"token":"mesh_tui_token","token_type":"bearer","destination":{"type":"tui"}}"#
            ),
            DeviceTokenPoll::Approved {
                token: crate::credentials::BearerToken::new("mesh_tui_token").unwrap()
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
            parse_poll_fixture(r#"{"error":"slow_down"}"#),
            DeviceTokenPoll::SlowDown
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

    #[test]
    fn maps_device_token_protocol_error_bodies_from_http_4xx() {
        let cases = [
            (
                400,
                r#"{"error":"authorization_pending"}"#,
                DeviceTokenPoll::Pending,
            ),
            (400, r#"{"error":"slow_down"}"#, DeviceTokenPoll::SlowDown),
            (403, r#"{"error":"access_denied"}"#, DeviceTokenPoll::Denied),
            (
                400,
                r#"{"error":"expired_token"}"#,
                DeviceTokenPoll::Expired,
            ),
            (
                400,
                r#"{"error":"invalid_device_code"}"#,
                DeviceTokenPoll::InvalidDeviceCode,
            ),
        ];

        for (status, body, expected) in cases {
            let poll = parse_device_token_poll_response(HttpResponse::new(status, body.as_bytes()))
                .unwrap();

            assert_eq!(poll, expected);
        }
    }

    #[tokio::test]
    async fn poll_device_token_maps_http_4xx_protocol_error_body() {
        let client = ApiClient::with_transport(
            ApiClientConfig::new("https://mesh.example").unwrap(),
            StaticTransport {
                response: HttpResponse::new(403, br#"{"error":"access_denied"}"#),
            },
        );

        let poll = client
            .poll_device_token(&DeviceCode::new("device-code").unwrap())
            .await
            .unwrap();

        assert_eq!(poll, DeviceTokenPoll::Denied);
    }

    #[test]
    fn reqwest_transport_uses_finite_request_timeout() {
        let transport = ReqwestTransport::new();

        assert_eq!(transport.request_timeout, Duration::from_secs(30));
        assert!(!transport.request_timeout.is_zero());
    }

    #[tokio::test]
    async fn delivery_api_decodes_public_list_detail_stream_and_auth_errors() {
        let token = crate::credentials::BearerToken::new("destination-token").unwrap();
        let observed_requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let client = ApiClient::with_transport(
            ApiClientConfig::new("https://mesh.example/").unwrap(),
            ScriptedDeliveryTransport::new(
                vec![
                    HttpResponse::new(
                        200,
                        br#"{
                        "deliveries": [
                            {
                                "public_delivery_id": "del_pub_01",
                                "headline": "CPU alert routed to ops",
                                "source_context": "Datadog",
                                "status": "delivered",
                                "published_at": "2026-06-04T19:09:10Z",
                                "detected_at": "2026-06-04T02:03:04Z",
                                "cursor": "cur_01"
                            }
                        ],
                        "next_cursor": "cur_next"
                    }"#,
                    ),
                    HttpResponse::new(
                        200,
                        br#"{
                        "data": {
                            "public_delivery_id": "del_pub_01",
                            "headline": "CPU alert routed to ops",
                            "summary": "CPU stayed over threshold for five minutes.",
                            "body": "The production worker pool crossed the CPU alert threshold.",
                            "source_url": "https://alerts.example/incidents/1",
                            "source_context": "Datadog",
                            "status": "delivered",
                            "published_at": "2026-06-04T19:09:10Z",
                            "detected_at": "2026-06-04T02:03:04Z",
                            "matched_routes": ["Ops Escalation"],
                            "cursor": "cur_01"
                        }
                    }"#,
                    ),
                    HttpResponse::new(
                        200,
                        b"event: cursor\ndata: {\"cursor\":\"cur_01\"}\n\n\
                          event: delivery\nid: cur_02\ndata: {\"public_delivery_id\":\"del_pub_02\",\"headline\":\"Deploy complete\",\"status\":\"delivered\"}\n\n",
                    ),
                    HttpResponse::new(401, br#"{"error":"token_expired"}"#),
                ],
                observed_requests.clone(),
            ),
        );

        let page = client.list_deliveries(&token).await.unwrap();
        assert_eq!(page.items().len(), 1);
        assert_eq!(page.items()[0].public_delivery_id().as_str(), "del_pub_01");
        assert_eq!(page.items()[0].headline(), "CPU alert routed to ops");
        assert_eq!(page.items()[0].source_context(), Some("Datadog"));
        assert_eq!(page.items()[0].status().as_str(), "delivered");
        assert_eq!(page.items()[0].published_at(), Some("2026-06-04T19:09:10Z"));
        assert_eq!(page.items()[0].detected_at(), Some("2026-06-04T02:03:04Z"));
        assert_eq!(
            page.items()[0].cursor().map(StreamCursor::as_str),
            Some("cur_01")
        );
        assert_eq!(
            page.next_cursor().map(StreamCursor::as_str),
            Some("cur_next")
        );

        let detail = client
            .get_delivery(&token, &PublicDeliveryId::new("del_pub_01").unwrap())
            .await
            .unwrap();
        assert_eq!(detail.public_delivery_id().as_str(), "del_pub_01");
        assert_eq!(detail.published_at(), Some("2026-06-04T19:09:10Z"));
        assert_eq!(
            detail.summary(),
            Some("CPU stayed over threshold for five minutes.")
        );
        assert_eq!(
            detail.body(),
            Some("The production worker pool crossed the CPU alert threshold.")
        );
        assert_eq!(
            detail.source_url(),
            Some("https://alerts.example/incidents/1")
        );
        assert_eq!(detail.matched_routes()[0].name(), "Ops Escalation");

        let mut frames = Vec::new();
        client
            .stream_deliveries(
                &token,
                Some(&StreamCursor::new("cur_01").unwrap()),
                |frame| {
                    frames.push(frame);

                    Ok(())
                },
            )
            .await
            .unwrap();

        assert_eq!(
            frames[0],
            DeliveryStreamFrame::Cursor(StreamCursor::new("cur_01").unwrap())
        );
        let DeliveryStreamFrame::Delivery { cursor, item } = &frames[1] else {
            panic!("expected a streamed delivery frame");
        };
        assert_eq!(cursor.as_ref().map(StreamCursor::as_str), Some("cur_02"));
        assert_eq!(item.public_delivery_id().as_str(), "del_pub_02");
        assert_eq!(item.headline(), "Deploy complete");
        assert_eq!(item.status().as_str(), "delivered");
        assert_eq!(item.source_context(), None);
        assert_eq!(item.published_at(), None);
        assert_eq!(item.detected_at(), None);
        assert_eq!(item.cursor(), None);

        let auth_error = client.list_deliveries(&token).await.unwrap_err();
        assert!(matches!(
            auth_error,
            ApiError::Authentication { status: 401, .. }
        ));

        let requests = observed_requests.lock().unwrap();
        assert_eq!(
            requests.as_slice(),
            [
                ObservedGet::new(
                    "https://mesh.example/api/v1/destination/deliveries",
                    "destination-token"
                ),
                ObservedGet::new(
                    "https://mesh.example/api/v1/destination/deliveries/del_pub_01",
                    "destination-token"
                ),
                ObservedGet::new(
                    "https://mesh.example/api/v1/destination/stream?after=cur_01",
                    "destination-token"
                ),
                ObservedGet::new(
                    "https://mesh.example/api/v1/destination/deliveries",
                    "destination-token"
                ),
            ]
        );
    }

    #[test]
    fn parses_delivery_stream_frames_without_internal_ids() {
        let frames = parse_delivery_stream_frames(
            b"event: delivery\ndata: {\"cursor\":\"cur_03\",\"delivery\":{\"public_delivery_id\":\"del_pub_03\",\"headline\":\"Webhook routed\",\"status\":\"delivered\"}}\n\n",
        )
        .unwrap();

        let DeliveryStreamFrame::Delivery { cursor, item } = &frames[0] else {
            panic!("expected a streamed delivery frame");
        };
        assert_eq!(cursor.as_ref().map(StreamCursor::as_str), Some("cur_03"));
        assert_eq!(item.public_delivery_id().as_str(), "del_pub_03");
    }

    #[test]
    fn parses_delivery_stream_frames_incrementally_across_chunks() {
        let mut parser = super::DeliveryStreamParser::default();

        assert!(parser.push_chunk(b"event: delivery\n").unwrap().is_empty());
        assert!(
            parser
                .push_chunk(
                    b"data: {\"public_delivery_id\":\"del_pub_04\",\"headline\":\"Partial event\","
                )
                .unwrap()
                .is_empty()
        );
        let frames = parser
            .push_chunk(b"\"status\":\"delivered\"}\n\nevent: cursor\r\ndata: cur_05\r\n\r\n")
            .unwrap();

        let DeliveryStreamFrame::Delivery { item, .. } = &frames[0] else {
            panic!("expected streamed delivery frame");
        };
        assert_eq!(item.public_delivery_id().as_str(), "del_pub_04");
        assert_eq!(
            frames[1],
            DeliveryStreamFrame::Cursor(StreamCursor::new("cur_05").unwrap())
        );
        assert!(parser.finish().unwrap().is_empty());
    }

    #[tokio::test]
    async fn delivery_api_emits_stream_frames_before_response_completes() {
        let client = ApiClient::with_transport(
            ApiClientConfig::new("https://mesh.example").unwrap(),
            HangingStreamTransport,
        );
        let token = crate::credentials::BearerToken::new("destination-token").unwrap();
        let (sender, receiver) = mpsc::channel();

        let handle = tokio::spawn(async move {
            client
                .stream_deliveries(&token, None, move |frame| {
                    sender.send(frame).map_err(|_| ApiError::InvalidResponse {
                        message: "test stream receiver closed".to_owned(),
                    })
                })
                .await
        });

        let frame = recv_delivery_stream_frame(&receiver).await;
        let DeliveryStreamFrame::Delivery { item, .. } = frame else {
            panic!("expected streamed delivery frame");
        };
        assert_eq!(item.public_delivery_id().as_str(), "del_pub_live");
        assert!(!handle.is_finished());
        handle.abort();
    }

    fn parse_poll_fixture(json: &str) -> DeviceTokenPoll {
        let response: DeviceTokenResponse = parse_success_json(
            HttpResponse::new(200, json.as_bytes()),
            "polling a device token",
        )
        .unwrap();

        response.into_poll().unwrap()
    }

    #[derive(Debug)]
    struct StaticTransport {
        response: HttpResponse,
    }

    impl HttpTransport for StaticTransport {
        async fn post_json(
            &self,
            _url: String,
            _body: Value,
        ) -> Result<HttpResponse, TransportError> {
            Ok(self.response.clone())
        }
    }

    #[derive(Debug)]
    struct ObservedPostTransport {
        response: HttpResponse,
        observed_requests: std::sync::Arc<std::sync::Mutex<Vec<ObservedPost>>>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct ObservedPost {
        url: String,
        body: Value,
    }

    impl HttpTransport for ObservedPostTransport {
        async fn post_json(
            &self,
            url: String,
            body: Value,
        ) -> Result<HttpResponse, TransportError> {
            self.observed_requests
                .lock()
                .unwrap()
                .push(ObservedPost { url, body });

            Ok(self.response.clone())
        }
    }

    #[derive(Debug)]
    struct ScriptedDeliveryTransport {
        responses: std::sync::Mutex<std::collections::VecDeque<HttpResponse>>,
        observed_requests: std::sync::Arc<std::sync::Mutex<Vec<ObservedGet>>>,
    }

    impl ScriptedDeliveryTransport {
        fn new(
            responses: Vec<HttpResponse>,
            observed_requests: std::sync::Arc<std::sync::Mutex<Vec<ObservedGet>>>,
        ) -> Self {
            Self {
                responses: std::sync::Mutex::new(responses.into()),
                observed_requests,
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct ObservedGet {
        url: String,
        token: String,
    }

    impl ObservedGet {
        fn new(url: impl Into<String>, token: impl Into<String>) -> Self {
            Self {
                url: url.into(),
                token: token.into(),
            }
        }
    }

    impl HttpTransport for ScriptedDeliveryTransport {
        async fn post_json(
            &self,
            _url: String,
            _body: Value,
        ) -> Result<HttpResponse, TransportError> {
            panic!("delivery API should use authenticated GET requests")
        }

        async fn get_bearer(
            &self,
            url: String,
            token: crate::credentials::BearerToken,
        ) -> Result<HttpResponse, TransportError> {
            self.observed_requests
                .lock()
                .unwrap()
                .push(ObservedGet::new(url, token.as_str()));

            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| TransportError::new("unexpected delivery API request"))
        }
    }

    #[derive(Debug)]
    struct HangingStreamTransport;

    impl HttpTransport for HangingStreamTransport {
        async fn post_json(
            &self,
            _url: String,
            _body: Value,
        ) -> Result<HttpResponse, TransportError> {
            panic!("delivery stream API should not use POST requests")
        }

        async fn get_bearer_stream(
            &self,
            _url: String,
            _token: crate::credentials::BearerToken,
            mut on_chunk: impl FnMut(&[u8]) -> Result<(), TransportError> + Send,
        ) -> Result<HttpResponse, TransportError> {
            on_chunk(
                b"event: delivery\ndata: {\"public_delivery_id\":\"del_pub_live\",\"headline\":\"Live delivery\",\"status\":\"delivered\"}\n\n",
            )?;

            std::future::pending::<Result<HttpResponse, TransportError>>().await
        }
    }

    async fn recv_delivery_stream_frame(
        receiver: &mpsc::Receiver<DeliveryStreamFrame>,
    ) -> DeliveryStreamFrame {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match receiver.try_recv() {
                    Ok(frame) => return frame,
                    Err(mpsc::TryRecvError::Empty) => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        panic!("stream disconnected before sending a frame")
                    }
                }
            }
        })
        .await
        .expect("stream frame")
    }
}
