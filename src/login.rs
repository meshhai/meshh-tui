use std::{error::Error, fmt, future::Future, io, io::Write, time::Duration};

use crate::{
    api::{ApiError, DeviceAuthorization, DeviceLoginApi, DeviceTokenPoll},
    credentials::{CredentialError, CredentialStore},
};

const SLOW_DOWN_INTERVAL_INCREMENT: Duration = Duration::from_secs(5);

/// Options that control device-login polling.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginOptions {
    max_poll_attempts: Option<usize>,
}

impl LoginOptions {
    /// Limits pending polls before the login attempt is treated as timed out.
    pub fn with_max_poll_attempts(mut self, max_poll_attempts: usize) -> Self {
        self.max_poll_attempts = Some(max_poll_attempts);
        self
    }
}

/// Sleeps between pending device-token polls.
pub trait LoginSleeper {
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send;
}

/// Tokio-backed sleeper used by the CLI.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioLoginSleeper;

impl LoginSleeper for TokioLoginSleeper {
    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

/// Successful login result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginSuccess {
    user_code: String,
}

impl LoginSuccess {
    /// Returns the user code associated with the approved login.
    pub fn user_code(&self) -> &str {
        &self.user_code
    }
}

/// Errors produced by the device login flow.
#[derive(Debug)]
pub enum LoginError {
    Api { source: ApiError },
    Credential { source: CredentialError },
    Output { source: io::Error },
    Denied,
    Expired,
    InvalidDeviceCode,
    TimedOut,
    Interrupted,
}

impl fmt::Display for LoginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api { .. } => formatter.write_str("could not contact the Meshh login API"),
            Self::Credential { .. } => formatter.write_str("could not store Meshh credentials"),
            Self::Output { .. } => formatter.write_str("could not write login instructions"),
            Self::Denied => formatter.write_str("login was denied in the browser"),
            Self::Expired => formatter.write_str("login expired; run `meshh login` again"),
            Self::InvalidDeviceCode => formatter.write_str(
                "login failed because the device code was rejected; run `meshh login` again",
            ),
            Self::TimedOut => formatter.write_str("login timed out before browser approval"),
            Self::Interrupted => formatter.write_str("login interrupted before browser approval"),
        }
    }
}

impl Error for LoginError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Api { source } => Some(source),
            Self::Credential { source } => Some(source),
            Self::Output { source } => Some(source),
            Self::Denied
            | Self::Expired
            | Self::InvalidDeviceCode
            | Self::TimedOut
            | Self::Interrupted => None,
        }
    }
}

/// Runs the Meshh device login flow and stores the approved bearer token.
pub async fn run_device_login<A, S, C, W>(
    api: &A,
    credentials: &S,
    sleeper: &C,
    output: &mut W,
    options: LoginOptions,
) -> Result<LoginSuccess, LoginError>
where
    A: DeviceLoginApi,
    S: CredentialStore,
    C: LoginSleeper,
    W: Write,
{
    let authorization = api
        .create_device_authorization()
        .await
        .map_err(|source| LoginError::Api { source })?;

    write_login_instructions(output, &authorization)?;

    let max_poll_attempts = options
        .max_poll_attempts
        .unwrap_or_else(|| default_max_poll_attempts(&authorization));
    let mut poll_interval = authorization.poll_interval();

    for attempt in 0..max_poll_attempts {
        match api
            .poll_device_token(authorization.device_code())
            .await
            .map_err(|source| LoginError::Api { source })?
        {
            DeviceTokenPoll::Approved { token } => {
                credentials
                    .save_token(&token)
                    .map_err(|source| LoginError::Credential { source })?;
                writeln!(output, "Login approved. Token stored.")
                    .map_err(|source| LoginError::Output { source })?;
                return Ok(LoginSuccess {
                    user_code: authorization.user_code().as_str().to_owned(),
                });
            }
            DeviceTokenPoll::Pending => {
                if attempt + 1 < max_poll_attempts {
                    sleeper.sleep(poll_interval).await;
                }
            }
            DeviceTokenPoll::SlowDown => {
                poll_interval = poll_interval.saturating_add(SLOW_DOWN_INTERVAL_INCREMENT);

                if attempt + 1 < max_poll_attempts {
                    sleeper.sleep(poll_interval).await;
                }
            }
            DeviceTokenPoll::Denied => return Err(LoginError::Denied),
            DeviceTokenPoll::Expired => return Err(LoginError::Expired),
            DeviceTokenPoll::InvalidDeviceCode => return Err(LoginError::InvalidDeviceCode),
        }
    }

    Err(LoginError::TimedOut)
}

fn write_login_instructions(
    output: &mut impl Write,
    authorization: &DeviceAuthorization,
) -> Result<(), LoginError> {
    writeln!(output, "Approve this terminal in Meshh:")
        .map_err(|source| LoginError::Output { source })?;
    writeln!(
        output,
        "Verification URL: {}",
        authorization.verification_url()
    )
    .map_err(|source| LoginError::Output { source })?;
    writeln!(output, "User code: {}", authorization.user_code().as_str())
        .map_err(|source| LoginError::Output { source })?;

    if let Some(verification_url_complete) = authorization.verification_url_complete() {
        writeln!(output, "Direct URL: {verification_url_complete}")
            .map_err(|source| LoginError::Output { source })?;
    }

    writeln!(output, "Waiting for approval...").map_err(|source| LoginError::Output { source })?;
    output
        .flush()
        .map_err(|source| LoginError::Output { source })
}

fn default_max_poll_attempts(authorization: &DeviceAuthorization) -> usize {
    let expires_secs = authorization.expires_in().as_secs();
    let interval_secs = authorization.poll_interval().as_secs().max(1);
    let attempts = expires_secs.div_ceil(interval_secs);

    attempts.max(1) as usize
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::Mutex,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use crate::{
        api::{
            ApiError, DeviceAuthorization, DeviceCode, DeviceLoginApi, DeviceTokenPoll, UserCode,
        },
        credentials::{BearerToken, CredentialError, CredentialStore},
    };

    use super::{LoginError, LoginOptions, LoginSleeper, run_device_login};

    #[tokio::test]
    async fn approved_device_login_prints_prompt_and_stores_token() {
        let api = ScriptedDeviceLoginApi::new(
            authorization(),
            [DeviceTokenPoll::Approved {
                token: BearerToken::new("approved-token").unwrap(),
            }],
        );
        let credentials = MemoryCredentialStore::default();
        let sleeper = RecordingSleeper::default();
        let mut output = Vec::new();

        run_device_login(
            &api,
            &credentials,
            &sleeper,
            &mut output,
            LoginOptions::default(),
        )
        .await
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("https://mesh.example/device"));
        assert!(output.contains("ABCD-EFGH"));
        assert_eq!(
            credentials.load_token().unwrap().unwrap().as_str(),
            "approved-token"
        );
    }

    #[tokio::test]
    async fn pending_device_login_polls_again_before_storing_token() {
        let api = ScriptedDeviceLoginApi::new(
            authorization(),
            [
                DeviceTokenPoll::Pending,
                DeviceTokenPoll::Approved {
                    token: BearerToken::new("approved-after-pending").unwrap(),
                },
            ],
        );
        let credentials = MemoryCredentialStore::default();
        let sleeper = RecordingSleeper::default();
        let mut output = Vec::new();

        run_device_login(
            &api,
            &credentials,
            &sleeper,
            &mut output,
            LoginOptions::default(),
        )
        .await
        .unwrap();

        assert_eq!(api.poll_count(), 2);
        assert_eq!(sleeper.sleep_count(), 1);
        assert_eq!(
            credentials.load_token().unwrap().unwrap().as_str(),
            "approved-after-pending"
        );
    }

    #[tokio::test]
    async fn default_poll_attempts_cover_non_multiple_expiration_windows() {
        let api = ScriptedDeviceLoginApi::new(
            authorization_with_expiry_and_interval(Duration::from_secs(6), Duration::from_secs(5)),
            [
                DeviceTokenPoll::Pending,
                DeviceTokenPoll::Approved {
                    token: BearerToken::new("approved-before-expiry").unwrap(),
                },
            ],
        );
        let credentials = MemoryCredentialStore::default();
        let sleeper = RecordingSleeper::default();
        let mut output = Vec::new();

        run_device_login(
            &api,
            &credentials,
            &sleeper,
            &mut output,
            LoginOptions::default(),
        )
        .await
        .unwrap();

        assert_eq!(api.poll_count(), 2);
        assert_eq!(sleeper.sleep_durations(), vec![Duration::from_secs(5)]);
        assert_eq!(
            credentials.load_token().unwrap().unwrap().as_str(),
            "approved-before-expiry"
        );
    }

    #[tokio::test]
    async fn slow_down_device_login_increases_polling_interval_before_retrying() {
        let api = ScriptedDeviceLoginApi::new(
            authorization(),
            [
                DeviceTokenPoll::Pending,
                DeviceTokenPoll::SlowDown,
                DeviceTokenPoll::Pending,
                DeviceTokenPoll::Approved {
                    token: BearerToken::new("approved-after-slow-down").unwrap(),
                },
            ],
        );
        let credentials = MemoryCredentialStore::default();
        let sleeper = RecordingSleeper::default();
        let mut output = Vec::new();

        run_device_login(
            &api,
            &credentials,
            &sleeper,
            &mut output,
            LoginOptions::default(),
        )
        .await
        .unwrap();

        assert_eq!(api.poll_count(), 4);
        assert_eq!(
            sleeper.sleep_durations(),
            vec![
                Duration::from_secs(5),
                Duration::from_secs(10),
                Duration::from_secs(10)
            ]
        );
        assert_eq!(
            credentials.load_token().unwrap().unwrap().as_str(),
            "approved-after-slow-down"
        );
    }

    #[tokio::test]
    async fn pending_device_login_times_out_after_poll_limit() {
        let api = ScriptedDeviceLoginApi::new(authorization(), [DeviceTokenPoll::Pending]);
        let credentials = MemoryCredentialStore::default();
        let sleeper = RecordingSleeper::default();
        let mut output = Vec::new();

        let error = run_device_login(
            &api,
            &credentials,
            &sleeper,
            &mut output,
            LoginOptions::default().with_max_poll_attempts(1),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, LoginError::TimedOut));
        assert!(credentials.load_token().unwrap().is_none());
    }

    #[derive(Debug)]
    struct ScriptedDeviceLoginApi {
        authorization: DeviceAuthorization,
        polls: Mutex<VecDeque<DeviceTokenPoll>>,
        poll_count: Mutex<usize>,
    }

    impl ScriptedDeviceLoginApi {
        fn new(
            authorization: DeviceAuthorization,
            polls: impl IntoIterator<Item = DeviceTokenPoll>,
        ) -> Self {
            Self {
                authorization,
                polls: Mutex::new(polls.into_iter().collect()),
                poll_count: Mutex::new(0),
            }
        }

        fn poll_count(&self) -> usize {
            *self.poll_count.lock().unwrap()
        }
    }

    impl DeviceLoginApi for ScriptedDeviceLoginApi {
        async fn create_device_authorization(&self) -> Result<DeviceAuthorization, ApiError> {
            Ok(self.authorization.clone())
        }

        async fn poll_device_token(
            &self,
            _device_code: &DeviceCode,
        ) -> Result<DeviceTokenPoll, ApiError> {
            *self.poll_count.lock().unwrap() += 1;
            Ok(self
                .polls
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted poll response"))
        }
    }

    #[derive(Debug, Default)]
    struct MemoryCredentialStore {
        token: Mutex<Option<BearerToken>>,
    }

    impl CredentialStore for MemoryCredentialStore {
        fn load_token(&self) -> Result<Option<BearerToken>, CredentialError> {
            Ok(self.token.lock().unwrap().clone())
        }

        fn save_token(&self, token: &BearerToken) -> Result<(), CredentialError> {
            *self.token.lock().unwrap() = Some(token.clone());
            Ok(())
        }

        fn delete_token(&self) -> Result<(), CredentialError> {
            *self.token.lock().unwrap() = None;
            Ok(())
        }
    }

    #[derive(Debug)]
    struct RecordingSleeper {
        sleeps: Mutex<Vec<Duration>>,
    }

    impl Default for RecordingSleeper {
        fn default() -> Self {
            Self {
                sleeps: Mutex::new(Vec::new()),
            }
        }
    }

    impl RecordingSleeper {
        fn sleep_count(&self) -> usize {
            self.sleeps.lock().unwrap().len()
        }

        fn sleep_durations(&self) -> Vec<Duration> {
            self.sleeps.lock().unwrap().clone()
        }
    }

    impl LoginSleeper for RecordingSleeper {
        async fn sleep(&self, duration: Duration) {
            self.sleeps.lock().unwrap().push(duration);
        }
    }

    fn authorization() -> DeviceAuthorization {
        authorization_with_expiry_and_interval(Duration::from_secs(600), Duration::from_secs(5))
    }

    fn authorization_with_expiry_and_interval(
        expires_in: Duration,
        poll_interval: Duration,
    ) -> DeviceAuthorization {
        DeviceAuthorization::new(
            DeviceCode::new(format!("device-{}", unique_suffix())).unwrap(),
            UserCode::new("ABCD-EFGH").unwrap(),
            "https://mesh.example/device",
            Some("https://mesh.example/device?user_code=ABCD-EFGH".to_owned()),
            expires_in,
            poll_interval,
        )
        .unwrap()
    }

    fn unique_suffix() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }
}
