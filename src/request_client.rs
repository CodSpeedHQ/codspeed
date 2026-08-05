use std::sync::LazyLock;

use reqwest::ClientBuilder;
use reqwest_middleware::{ClientBuilder as ClientWithMiddlewareBuilder, ClientWithMiddleware};
use reqwest_retry::{
    RetryTransientMiddleware, Retryable, RetryableStrategy, default_on_request_success,
    policies::ExponentialBackoff,
};

pub const UPLOAD_RETRY_COUNT: u32 = 3;
const OIDC_RETRY_COUNT: u32 = 10;
const DOWNLOAD_RETRY_COUNT: u32 = 5;
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

/// Backoff policy for pinned binary downloads. Under `cfg(test)` the intervals
/// are shrunk to milliseconds so retry tests don't sleep through the real
/// exponential backoff.
fn download_backoff() -> ExponentialBackoff {
    let builder = ExponentialBackoff::builder();
    #[cfg(test)]
    let builder = builder.retry_bounds(
        std::time::Duration::from_millis(1),
        std::time::Duration::from_millis(5),
    );
    #[cfg(not(test))]
    let builder = builder.retry_bounds(
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(30),
    );
    builder.build_with_max_retries(DOWNLOAD_RETRY_COUNT)
}

/// Retry strategy for downloads. `DefaultRetryableStrategy` classifies several
/// transient network errors as fatal — an `UnexpectedEof`, `BrokenPipe`, or
/// `TimedOut` io error surfaces as a reqwest "error sending request" that it
/// declines to retry — which made a single GitHub blip fail a whole CI run.
/// Downloads are idempotent GETs, so every request-level failure is safe to
/// retry; only responses keep the default status-based classification.
struct RetryAllRequestErrors;

impl RetryableStrategy for RetryAllRequestErrors {
    fn handle(
        &self,
        res: &Result<reqwest::Response, reqwest_middleware::Error>,
    ) -> Option<Retryable> {
        match res {
            Ok(success) => default_on_request_success(success),
            // A failure in our own middleware stack won't fix itself.
            Err(reqwest_middleware::Error::Middleware(_)) => Some(Retryable::Fatal),
            Err(reqwest_middleware::Error::Reqwest(_)) => Some(Retryable::Transient),
        }
    }
}

pub static REQUEST_CLIENT: LazyLock<ClientWithMiddleware> = LazyLock::new(|| {
    ClientWithMiddlewareBuilder::new(ClientBuilder::new().user_agent(USER_AGENT).build().unwrap())
        .with(RetryTransientMiddleware::new_with_policy(upload_backoff()))
        .build()
});

/// Client for pinned binary downloads, retrying any transient request failure.
pub static DOWNLOAD_CLIENT: LazyLock<ClientWithMiddleware> = LazyLock::new(|| {
    ClientWithMiddlewareBuilder::new(ClientBuilder::new().user_agent(USER_AGENT).build().unwrap())
        .with(RetryTransientMiddleware::new_with_policy_and_strategy(
            download_backoff(),
            RetryAllRequestErrors,
        ))
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
