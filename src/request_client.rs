use std::sync::LazyLock;

use reqwest::ClientBuilder;
use reqwest_middleware::{ClientBuilder as ClientWithMiddlewareBuilder, ClientWithMiddleware};
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};

pub const UPLOAD_RETRY_COUNT: u32 = 3;
const OIDC_RETRY_COUNT: u32 = 10;
const USER_AGENT: &str = "codspeed-runner";

/// Shared backoff policy for upload retries, used both by the retry middleware on
/// [`REQUEST_CLIENT`] and by the manual stream-retry loop in the uploader. Under
/// `cfg(test)` the intervals are shrunk to milliseconds so retry tests don't sleep
/// through the real exponential backoff (1s, 2s, 4s).
pub fn upload_backoff() -> ExponentialBackoff {
    let builder = ExponentialBackoff::builder();
    #[cfg(test)]
    let builder = builder.retry_bounds(
        std::time::Duration::from_millis(1),
        std::time::Duration::from_millis(5),
    );
    builder.build_with_max_retries(UPLOAD_RETRY_COUNT)
}

pub static REQUEST_CLIENT: LazyLock<ClientWithMiddleware> = LazyLock::new(|| {
    ClientWithMiddlewareBuilder::new(ClientBuilder::new().user_agent(USER_AGENT).build().unwrap())
        .with(RetryTransientMiddleware::new_with_policy(upload_backoff()))
        .build()
});

/// Client without retry middleware for streaming uploads (can't be cloned)
pub static STREAMING_CLIENT: LazyLock<reqwest::Client> =
    LazyLock::new(|| ClientBuilder::new().user_agent(USER_AGENT).build().unwrap());

/// Client with retry middleware for OIDC token requests
pub static OIDC_CLIENT: LazyLock<ClientWithMiddleware> = LazyLock::new(|| {
    ClientWithMiddlewareBuilder::new(ClientBuilder::new().user_agent(USER_AGENT).build().unwrap())
        .with(RetryTransientMiddleware::new_with_policy(
            ExponentialBackoff::builder().build_with_max_retries(OIDC_RETRY_COUNT),
        ))
        .build()
});
