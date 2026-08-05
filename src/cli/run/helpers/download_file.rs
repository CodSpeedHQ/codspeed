use crate::binary_pins::PinnedBinary;
use crate::{prelude::*, request_client::DOWNLOAD_CLIENT};
use reqwest_retry::{
    RetryDecision, RetryPolicy, Retryable, default_on_request_success, policies::ExponentialBackoff,
};
use std::path::Path;
use std::time::SystemTime;

use url::Url;

const DOWNLOAD_RETRY_COUNT: u32 = 5;

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

enum AttemptError {
    Fatal(Error),
    Transient(Error),
}

async fn download_file(url: &Url, path: &Path) -> Result<(), AttemptError> {
    debug!("Downloading file: {url}");
    let response = DOWNLOAD_CLIENT
        .get(url.clone())
        .send()
        .await
        .map_err(|e| AttemptError::Transient(anyhow!("Failed to download file: {e}")))?;

    if !response.status().is_success() {
        let error = anyhow!("Failed to download file: {}", response.status());
        return Err(match default_on_request_success(&response) {
            Some(Retryable::Transient) => AttemptError::Transient(error),
            _ => AttemptError::Fatal(error),
        });
    }

    let content = response
        .bytes()
        .await
        .map_err(|e| AttemptError::Transient(anyhow!("Failed to read response: {e}")))?;
    let mut file = std::fs::File::create(path).map_err(|e| {
        AttemptError::Fatal(anyhow!("Failed to create file: {}, {}", path.display(), e))
    })?;
    std::io::copy(&mut content.as_ref(), &mut file).map_err(|e| {
        AttemptError::Fatal(anyhow!(
            "Failed to write to file: {}, {}",
            path.display(),
            e
        ))
    })?;
    Ok(())
}

async fn download_and_verify_once(
    url: &Url,
    expected_sha256: &str,
    path: &Path,
) -> Result<(), AttemptError> {
    download_file(url, path).await?;

    let actual = sha256::try_digest(path).map_err(|e| {
        AttemptError::Fatal(
            anyhow!(e).context(format!("failed to compute sha256 of {}", path.display())),
        )
    })?;

    if actual != expected_sha256 {
        let _ = std::fs::remove_file(path);
        return Err(AttemptError::Fatal(anyhow!(
            "Hash mismatch for {url}: expected {expected_sha256}, got {actual}. The downloaded file has been deleted."
        )));
    }

    debug!("Verified sha256 of {url}");
    Ok(())
}

async fn download_and_verify(url: &Url, expected_sha256: &str, path: &Path) -> Result<()> {
    let policy = download_backoff();
    let start = SystemTime::now();
    let mut n_past_retries = 0;

    loop {
        let error = match download_and_verify_once(url, expected_sha256, path).await {
            Ok(()) => return Ok(()),
            Err(AttemptError::Fatal(error)) => return Err(error),
            Err(AttemptError::Transient(error)) => error,
        };

        match policy.should_retry(start, n_past_retries) {
            RetryDecision::Retry { execute_after } => {
                let wait = execute_after
                    .duration_since(SystemTime::now())
                    .unwrap_or_default();
                warn!("Downloading {url} failed: {error}. Retrying in {wait:?}.");
                tokio::time::sleep(wait).await;
                n_past_retries += 1;
            }
            RetryDecision::DoNotRetry => return Err(error),
        }
    }
}

/// Download a `PinnedBinary` and verify its bytes against its pinned SHA-256.
pub async fn download_pinned_file(binary: PinnedBinary, path: &Path) -> Result<()> {
    let url_str = binary.url();
    let url = Url::parse(&url_str).context("failed to parse pinned URL")?;
    download_and_verify(&url, binary.sha256(), path).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::NamedTempFile;

    const GOOD_BODY: &[u8] = b"expected file content";
    const BAD_BODY: &[u8] = b"corrupted file content";

    enum ScriptedResponse {
        /// Respond 200 with the given body.
        Body(&'static [u8]),
        /// Respond 200 with fewer bytes than declared by `Content-Length`.
        TruncatedBody {
            body: &'static [u8],
            declared_length: usize,
        },
        /// Respond with the given status code and an empty body.
        Status(u16),
        /// Close the connection without responding.
        Abort,
    }

    /// Serve one scripted response per connection, then stop listening.
    /// Every response closes the connection so each request is a new
    /// connection, making the accept counter a request counter.
    fn spawn_scripted_server(script: Vec<ScriptedResponse>) -> (Url, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind test server");
        let url = Url::parse(&format!("http://{}/file", listener.local_addr().unwrap())).unwrap();
        let request_count = Arc::new(AtomicUsize::new(0));

        let counter = Arc::clone(&request_count);
        std::thread::spawn(move || {
            for response in script {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(_) => return,
                };
                counter.fetch_add(1, Ordering::SeqCst);

                // Read until the end of the request headers.
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            request.extend_from_slice(&buf[..n]);
                            if request.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }

                match response {
                    ScriptedResponse::Body(body) => {
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(body);
                    }
                    ScriptedResponse::TruncatedBody {
                        body,
                        declared_length,
                    } => {
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Length: {declared_length}\r\nConnection: close\r\n\r\n"
                        );
                        let _ = stream.write_all(body);
                    }
                    ScriptedResponse::Status(status) => {
                        let _ = write!(
                            stream,
                            "HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        );
                    }
                    ScriptedResponse::Abort => {}
                }
            }
        });

        (url, request_count)
    }

    #[tokio::test]
    async fn recovers_from_aborted_connection() {
        let (url, request_count) = spawn_scripted_server(vec![
            ScriptedResponse::Abort,
            ScriptedResponse::Body(GOOD_BODY),
        ]);
        let file = NamedTempFile::new().unwrap();

        download_and_verify(&url, &sha256::digest(GOOD_BODY), file.path())
            .await
            .expect("download should recover from an aborted connection");

        assert_eq!(std::fs::read(file.path()).unwrap(), GOOD_BODY);
        assert_eq!(request_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn recovers_from_truncated_body() {
        let (url, request_count) = spawn_scripted_server(vec![
            ScriptedResponse::TruncatedBody {
                body: GOOD_BODY,
                declared_length: GOOD_BODY.len() + 1,
            },
            ScriptedResponse::Body(GOOD_BODY),
        ]);
        let file = NamedTempFile::new().unwrap();

        download_and_verify(&url, &sha256::digest(GOOD_BODY), file.path())
            .await
            .expect("download should recover from a truncated response body");

        assert_eq!(std::fs::read(file.path()).unwrap(), GOOD_BODY);
        assert_eq!(request_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn fails_after_exhausting_retries_on_truncated_bodies() {
        let attempts = (DOWNLOAD_RETRY_COUNT + 1) as usize;
        let (url, request_count) = spawn_scripted_server(
            (0..attempts)
                .map(|_| ScriptedResponse::TruncatedBody {
                    body: GOOD_BODY,
                    declared_length: GOOD_BODY.len() + 1,
                })
                .collect(),
        );
        let file = NamedTempFile::new().unwrap();

        let error = download_and_verify(&url, &sha256::digest(GOOD_BODY), file.path())
            .await
            .expect_err("persistent truncated bodies should fail the download");

        assert!(
            error.to_string().contains("Failed to read response"),
            "unexpected error: {error}"
        );
        assert_eq!(request_count.load(Ordering::SeqCst), attempts);
    }

    #[tokio::test]
    async fn retries_server_errors_and_recovers() {
        let (url, request_count) = spawn_scripted_server(vec![
            ScriptedResponse::Status(500),
            ScriptedResponse::Body(GOOD_BODY),
        ]);
        let file = NamedTempFile::new().unwrap();

        download_and_verify(&url, &sha256::digest(GOOD_BODY), file.path())
            .await
            .expect("download should recover from a transient server error");

        assert_eq!(std::fs::read(file.path()).unwrap(), GOOD_BODY);
        assert_eq!(request_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn does_not_retry_client_errors() {
        let (url, request_count) = spawn_scripted_server(vec![ScriptedResponse::Status(404)]);
        let file = NamedTempFile::new().unwrap();

        let error = download_and_verify(&url, &sha256::digest(GOOD_BODY), file.path())
            .await
            .expect_err("a 404 should fail the download");

        assert!(
            error.to_string().contains("404"),
            "unexpected error: {error}"
        );
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn does_not_retry_hash_mismatch() {
        let (url, request_count) = spawn_scripted_server(vec![ScriptedResponse::Body(BAD_BODY)]);
        let file = NamedTempFile::new().unwrap();

        let error = download_and_verify(&url, &sha256::digest(GOOD_BODY), file.path())
            .await
            .expect_err("a hash mismatch should fail the download");

        assert!(
            error.to_string().contains("Hash mismatch"),
            "unexpected error: {error}"
        );
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        assert!(!file.path().exists(), "partial file should be deleted");
    }
}
