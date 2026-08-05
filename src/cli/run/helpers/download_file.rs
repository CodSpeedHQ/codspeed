use crate::binary_pins::PinnedBinary;
use crate::{prelude::*, request_client::DOWNLOAD_CLIENT};
use std::path::Path;

use url::Url;

async fn download_file(url: &Url, path: &Path) -> Result<()> {
    debug!("Downloading file: {url}");
    let response = DOWNLOAD_CLIENT
        .get(url.clone())
        .send()
        .await
        .map_err(|e| anyhow!("Failed to download file: {e}"))?;
    if !response.status().is_success() {
        bail!("Failed to download file: {}", response.status());
    }
    let mut file = std::fs::File::create(path)
        .map_err(|e| anyhow!("Failed to create file: {}, {}", path.display(), e))?;
    let content = response
        .bytes()
        .await
        .map_err(|e| anyhow!("Failed to read response: {e}"))?;
    std::io::copy(&mut content.as_ref(), &mut file)
        .map_err(|e| anyhow!("Failed to write to file: {}, {}", path.display(), e))?;
    Ok(())
}

/// Download a URL and verify its bytes against an expected SHA-256. Transient
/// request failures are retried by the middleware on [`DOWNLOAD_CLIENT`]. A
/// mismatch is not retried — the bytes arrived intact (a torn transfer fails
/// earlier, on the body read), so it means the pin is wrong rather than the
/// download. The partial file is removed and an error is returned.
async fn download_and_verify(url: &Url, expected_sha256: &str, path: &Path) -> Result<()> {
    download_file(url, path).await?;

    let actual = sha256::try_digest(path)
        .with_context(|| format!("failed to compute sha256 of {}", path.display()))?;

    if actual != expected_sha256 {
        let _ = std::fs::remove_file(path);
        bail!(
            "Hash mismatch for {url}: expected {expected_sha256}, got {actual}. The downloaded file has been deleted."
        );
    }

    debug!("Verified sha256 of {url}");
    Ok(())
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
