use crate::api_client::CodSpeedAPIClient;
use crate::executor::ExecutionContext;
use crate::executor::ExecutorName;
use crate::executor::Orchestrator;
use crate::run_environment::RunEnvironment;
use crate::upload::{UploadError, profile_archive::ProfileArchiveContent};
use crate::{
    prelude::*,
    request_client::{REQUEST_CLIENT, STREAMING_CLIENT, upload_backoff},
};
use async_compression::tokio::write::GzipEncoder;
use console::style;
use futures::{StreamExt, TryStreamExt};
use reqwest::StatusCode;
use reqwest_retry::{
    DefaultRetryableStrategy, RetryDecision, RetryPolicy, Retryable, RetryableStrategy,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio_tar::Builder;

use super::interfaces::{MultipartUploadUrls, ProfileMultipart, UploadData, UploadMetadata};
use super::profile_archive::{ProfileArchive, concurrent_part_uploads};
use super::s3;

fn human_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes}")
    }
}

fn human_bytes_per_second(bytes: u64, elapsed: std::time::Duration) -> String {
    let bytes_per_second = bytes as f64 / elapsed.as_secs_f64();
    format!("{}/s", human_bytes(bytes_per_second as u64))
}

/// Create a profile archive from the profile folder and return its md5 hash encoded in base64
///
/// For Valgrind, we create a gzip-compressed tar archive of the entire profile folder.
/// For WallTime and Memory, we create an uncompressed tar archive on disk: their
/// profiles are already compressed, so gzip would barely shrink them.
async fn create_profile_archive(
    profile_folder: &std::path::Path,
    executor_name: ExecutorName,
) -> Result<ProfileArchive> {
    let time_start = std::time::Instant::now();
    let profile_archive = match executor_name {
        ExecutorName::Valgrind => {
            debug!("Creating compressed tar archive for Valgrind");
            let enc = GzipEncoder::new(Vec::new());
            let mut tar = Builder::new(enc);
            tar.append_dir_all(".", profile_folder).await?;
            let mut gzip_encoder = tar.into_inner().await?;
            gzip_encoder.shutdown().await?;
            let data = gzip_encoder.into_inner();
            ProfileArchive::new_compressed_in_memory(data).await?
        }
        ExecutorName::Memory | ExecutorName::WallTime => {
            debug!("Creating uncompressed tar archive on disk");
            let temp_file = tempfile::NamedTempFile::new()?;
            let temp_path = temp_file.path().to_path_buf();

            // Create a tokio File handle to the temporary file
            let file = File::create(&temp_path).await?;

            // Persist the temporary file to prevent deletion when temp_file goes out of scope
            let persistent_path = temp_file.into_temp_path().keep()?;

            let mut tar = Builder::new(file);
            tar.append_dir_all(".", profile_folder).await?;
            tar.into_inner().await?.sync_all().await?;

            ProfileArchive::new_uncompressed_on_disk(persistent_path).await?
        }
    };

    let archive_size = profile_archive.content.size().await?;
    debug!(
        "Created archive ({} bytes) in {:.2?}",
        archive_size,
        time_start.elapsed()
    );

    Ok(profile_archive)
}

async fn retrieve_upload_data(
    orchestrator: &Orchestrator,
    api_client: &CodSpeedAPIClient,
    upload_metadata: &UploadMetadata,
) -> Result<UploadData> {
    let mut upload_request = REQUEST_CLIENT
        .post(orchestrator.config.upload_url.clone())
        .json(&upload_metadata);
    if let Some(token) = api_client.token() {
        upload_request = upload_request.header("Authorization", token.to_owned());
    }

    let response = upload_request.send().await;

    match response {
        Ok(response) => {
            if !response.status().is_success() {
                let status = response.status();
                let text = response.text().await?;
                let mut error_message = serde_json::from_str::<UploadError>(&text)
                    .map(|body| body.error)
                    .unwrap_or(text);
                if status == StatusCode::UNAUTHORIZED {
                    let run_environment = &upload_metadata.run_environment;
                    let additional_message = match run_environment {
                        RunEnvironment::GithubActions => {
                            "Check that the workflow is correctly authenticated."
                        }
                        RunEnvironment::GitlabCi | RunEnvironment::Circleci => {
                            "Check that the CI job is correctly authenticated."
                        }
                        RunEnvironment::Buildkite => {
                            "Check that CODSPEED_TOKEN is set and has the correct value."
                        }
                        RunEnvironment::Local => {
                            "Run `codspeed auth login` to authenticate the CLI."
                        }
                    };
                    error_message.push_str(&format!("\n\n{additional_message}"));
                    if let Some(url) = run_environment.authentication_docs_url() {
                        error_message.push_str(&format!(" View more at {url}"));
                    }
                }

                debug!(
                    "Check that owner and repository are correct (case-sensitive!): {}/{}",
                    upload_metadata.run_environment_metadata.owner,
                    upload_metadata.run_environment_metadata.repository
                );

                bail!(
                    "Failed to retrieve upload data: {}\n  -> {} {}",
                    status,
                    style("Reason:").bold(),
                    // we have to manually apply the style to the error message, because nesting styles is not supported by the console crate: https://github.com/console-rs/console/issues/106
                    style(error_message).red()
                );
            }

            Ok(response.json().await?)
        }
        Err(err) => Err(err.into()),
    }
}

/// A byte range of the archive content, sent as a request body.
struct ContentRange<'a> {
    content: &'a ProfileArchiveContent,
    offset: u64,
    length: u64,
}

impl ContentRange<'_> {
    /// Attach this range as the body of `request`. The body is rebuilt on every call,
    /// since a streamed body is consumed by the request that sends it.
    async fn attach(&self, request: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder> {
        let body = self.content.body(self.offset, self.length).await?;
        Ok(request.header("Content-Length", self.length).body(body))
    }
}

/// Failure of one attempt of an upload request
enum AttemptError {
    /// Worth sending the request again
    Transient(Error),
    Permanent(Error),
}

impl From<Error> for AttemptError {
    fn from(error: Error) -> Self {
        AttemptError::Permanent(error)
    }
}

/// Run `attempt` until it succeeds, retrying transient failures with the
/// [`upload_backoff`] policy.
async fn with_upload_retry<T, Fut>(mut attempt: impl FnMut() -> Fut) -> Result<T>
where
    Fut: Future<Output = std::result::Result<T, AttemptError>>,
{
    let policy = upload_backoff();
    let start = SystemTime::now();
    let mut n_past_retries = 0;

    loop {
        let error = match attempt().await {
            Ok(value) => return Ok(value),
            Err(AttemptError::Permanent(error)) => return Err(error),
            Err(AttemptError::Transient(error)) => error,
        };
        let RetryDecision::Retry { execute_after } = policy.should_retry(start, n_past_retries)
        else {
            return Err(error);
        };
        let wait = execute_after
            .duration_since(SystemTime::now())
            .unwrap_or_default();
        debug!("Upload attempt failed (transient), retrying in {wait:?}: {error}");
        tokio::time::sleep(wait).await;
        n_past_retries += 1;
    }
}

/// Send an upload request, failing on a non-success status. Connection errors and
/// statuses such as 5xx or 429 are reported as transient.
async fn send_upload_request(
    request: reqwest::RequestBuilder,
) -> std::result::Result<reqwest::Response, AttemptError> {
    /// Bounds reading the body of a failed upload response, only used in the error message
    const ERROR_BODY_READ_TIMEOUT: Duration = Duration::from_secs(10);

    let result = request
        .send()
        .await
        .map_err(reqwest_middleware::Error::Reqwest);
    let is_transient = matches!(
        DefaultRetryableStrategy.handle(&result),
        Some(Retryable::Transient)
    );
    let error = match result {
        Ok(response) if response.status().is_success() => return Ok(response),
        Ok(response) => {
            let status = response.status();
            // A stalled error body must not keep a retryable failure from being retried
            let error_text = tokio::time::timeout(ERROR_BODY_READ_TIMEOUT, response.text())
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            anyhow!(
                "Failed to upload performance report: {}\n  -> {} {}",
                status,
                style("Reason:").bold(),
                style(error_text).red()
            )
        }
        Err(error) => error.into(),
    };
    Err(if is_transient {
        AttemptError::Transient(error)
    } else {
        AttemptError::Permanent(error)
    })
}

/// Upload the parts concurrently, then assemble them with the S3 complete request.
/// Parts of a failed upload are cleaned up by the bucket lifecycle rules.
async fn upload_multipart_profile_archive(
    multipart_upload_urls: &MultipartUploadUrls,
    multipart: &ProfileMultipart,
    content: &ProfileArchiveContent,
) -> Result<()> {
    debug!("Starting multipart upload for profile archive");
    debug!(
        "Multipart upload details: part_count={}, part_size={}, total_size={}",
        multipart.part_md5s.len(),
        multipart.part_size,
        multipart.size
    );

    let part_count = multipart.part_md5s.len();
    if multipart_upload_urls.part_urls.len() != part_count {
        bail!(
            "Received {} part upload URLs for {} parts",
            multipart_upload_urls.part_urls.len(),
            part_count
        );
    }

    let concurrency = concurrent_part_uploads();
    let upload_start = Instant::now();
    // Unordered, so that a part finishing frees its slot even while an earlier part is still
    // uploading
    let mut indexed_etags: Vec<_> = futures::stream::iter(
        multipart_upload_urls
            .part_urls
            .iter()
            .zip(&multipart.part_md5s)
            .enumerate(),
    )
    .map(|(index, (part_url, part_md5))| async move {
        let offset = index as u64 * multipart.part_size;
        let range = ContentRange {
            content,
            offset,
            length: multipart.part_size.min(multipart.size - offset),
        };
        debug!(
            "Uploading part {}/{} ({} bytes)",
            index + 1,
            part_count,
            range.length
        );
        let part_start = Instant::now();
        let etag = with_upload_retry(|| async {
            let request = s3::upload_part(&STREAMING_CLIENT, part_url, part_md5);
            let response = send_upload_request(range.attach(request).await?).await?;
            Ok(s3::part_etag(&response)?)
        })
        .await?;
        let part_elapsed = part_start.elapsed();
        debug!(
            "Uploaded part {}/{} in {:.1?} ({})",
            index + 1,
            part_count,
            part_elapsed,
            human_bytes_per_second(range.length, part_elapsed)
        );
        Ok::<_, anyhow::Error>((index, etag))
    })
    .buffer_unordered(concurrency)
    .try_collect()
    .await?;

    indexed_etags.sort_unstable_by_key(|(index, _)| *index);
    let etags: Vec<_> = indexed_etags.into_iter().map(|(_, etag)| etag).collect();

    let upload_elapsed = upload_start.elapsed();
    info!(
        "Uploaded {} part{} ({}) in {:.1?} with {} concurrent uploads ({})",
        part_count,
        if part_count > 1 { "s" } else { "" },
        human_bytes(multipart.size),
        upload_elapsed,
        concurrency,
        human_bytes_per_second(multipart.size, upload_elapsed)
    );

    with_upload_retry(|| async {
        let request = s3::complete_upload(
            &STREAMING_CLIENT,
            &multipart_upload_urls.complete_url,
            &etags,
        );
        let response = send_upload_request(request).await?;
        match s3::check_complete_upload_response(response).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) if error.is_transient() => Err(AttemptError::Transient(error.into())),
            Ok(Err(error)) => Err(AttemptError::Permanent(error.into())),
            // The connection stays open while S3 assembles the parts, and can drop
            Err(error) => Err(AttemptError::Transient(error.into())),
        }
    })
    .await
}

async fn upload_profile_archive(
    upload_data: &UploadData,
    profile_archive: ProfileArchive,
) -> Result<()> {
    upload_multipart_profile_archive(
        &upload_data.multipart_upload_urls,
        &profile_archive.multipart,
        &profile_archive.content,
    )
    .await
}

#[derive(Clone)]
pub struct UploadResult {
    pub run_id: String,
    pub owner: String,
    pub repository: String,
}

pub async fn upload(
    orchestrator: &Orchestrator,
    api_client: &CodSpeedAPIClient,
    execution_context: &ExecutionContext,
    executor_name: ExecutorName,
    run_part_suffix: BTreeMap<String, Value>,
) -> Result<UploadResult> {
    let profile_archive =
        create_profile_archive(&execution_context.profile_folder, executor_name.clone()).await?;

    debug!(
        "Run Environment provider detected: {:?}",
        orchestrator.provider.get_run_environment()
    );

    let upload_metadata = orchestrator
        .provider
        .get_upload_metadata(
            &execution_context.config,
            api_client,
            &orchestrator.system_info,
            &profile_archive,
            executor_name,
            run_part_suffix,
        )
        .await?;
    debug!("Upload metadata: {upload_metadata:#?}");
    if upload_metadata.tokenless {
        let hash = upload_metadata.get_hash();
        info!("CodSpeed Run Hash: \"{hash}\"");
    }

    debug!("Preparing upload...");
    let upload_data = retrieve_upload_data(orchestrator, api_client, &upload_metadata).await?;
    debug!("runId: {}", upload_data.run_id);

    debug!(
        "Uploading {} bytes...",
        profile_archive.content.size().await?
    );
    upload_profile_archive(&upload_data, profile_archive).await?;

    Ok(UploadResult {
        run_id: upload_data.run_id,
        owner: upload_metadata.run_environment_metadata.owner.clone(),
        repository: upload_metadata.run_environment_metadata.repository.clone(),
    })
}

#[cfg(test)]
mod tests {
    use crate::api_client::CodSpeedAPIClient;
    use temp_env::async_with_vars;
    use url::Url;

    use super::*;
    use std::path::PathBuf;

    // TODO: remove the ignore when implementing network mocking
    #[ignore]
    #[tokio::test]
    async fn test_upload() {
        use crate::executor::ExecutorConfig;
        use crate::executor::config::OrchestratorConfig;

        let orchestrator_config = OrchestratorConfig {
            upload_url: Url::parse("change me").unwrap(),
            profile_folder: Some(PathBuf::from(format!(
                "{}/src/uploader/samples/adrien-python-test",
                env!("CARGO_MANIFEST_DIR")
            ))),
            ..OrchestratorConfig::test()
        };
        let profile_folder = PathBuf::from(format!(
            "{}/src/uploader/samples/adrien-python-test",
            env!("CARGO_MANIFEST_DIR")
        ));
        let executor_config = ExecutorConfig {
            command: "pytest tests/ --codspeed".into(),
            ..ExecutorConfig::test()
        };
        async_with_vars(
            [
                ("GITHUB_ACTIONS", Some("true")),
                ("GITHUB_ACTOR_ID", Some("19605940")),
                ("GITHUB_ACTOR", Some("adriencaccia")),
                ("GITHUB_BASE_REF", Some("main")),
                ("GITHUB_EVENT_NAME", Some("pull_request")),
                (
                    "GITHUB_EVENT_PATH",
                    Some(
                        format!(
                            "{}/src/uploader/samples/pr-event.json",
                            env!("CARGO_MANIFEST_DIR")
                        )
                        .as_str(),
                    ),
                ),
                ("GITHUB_HEAD_REF", Some("feat/codspeed-runner")),
                ("GITHUB_JOB", Some("log-env")),
                ("GITHUB_REF", Some("refs/pull/22/merge")),
                ("GITHUB_REPOSITORY", Some("my-org/adrien-python-test")),
                ("GITHUB_RUN_ID", Some("6957110437")),
                (
                    "GITHUB_SHA",
                    Some("5bd77cb0da72bef094893ed45fb793ff16ecfbe3"),
                ),
                ("VERSION", Some("0.1.0")),
            ],
            async {
                let api_client = CodSpeedAPIClient::create_test_client();
                let orchestrator = Orchestrator::new(orchestrator_config, &api_client)
                    .await
                    .expect("Failed to create Orchestrator for test");
                let execution_context = ExecutionContext::new(executor_config, profile_folder);
                let run_part_suffix =
                    BTreeMap::from([("executor".to_string(), Value::from("valgrind"))]);
                upload(
                    &orchestrator,
                    &api_client,
                    &execution_context,
                    ExecutorName::Valgrind,
                    run_part_suffix,
                )
                .await
                .unwrap();
            },
        )
        .await;
    }

    const EXPECTED_ATTEMPTS: usize = crate::request_client::UPLOAD_RETRY_COUNT as usize + 1;

    /// Answers `503` to each of the next `max_conns` connections, then exits. Returns
    /// the base URL, a counter of connections received, and the server's join handle.
    fn spawn_mock_returning_503(
        max_conns: usize,
    ) -> (
        String,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
        std::thread::JoinHandle<()>,
    ) {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));

        let hits_loop = hits.clone();
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming().take(max_conns) {
                let Ok(mut stream) = stream else { continue };
                hits_loop.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let body = "transient";
                let resp = format!(
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        (url, hits, handle)
    }

    /// `send_with_retry` retries transient failures itself, since `STREAMING_CLIENT`
    /// has no retry middleware.
    #[tokio::test]
    async fn streamed_upload_is_retried() {
        use std::sync::atomic::Ordering;

        let (url, hits, server) = spawn_mock_returning_503(EXPECTED_ATTEMPTS);

        let path = tempfile::NamedTempFile::new()
            .unwrap()
            .into_temp_path()
            .keep()
            .unwrap();
        std::fs::write(&path, b"profile-archive").unwrap();
        let archive = ProfileArchive::new_uncompressed_on_disk(path)
            .await
            .unwrap();

        let result = upload_profile_archive(&multipart_upload_data_for(&url, 1), archive).await;
        server.join().unwrap();

        assert!(
            result.is_err(),
            "a 503 should surface as an error after retries"
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            EXPECTED_ATTEMPTS,
            "streamed upload should be attempted 1 + UPLOAD_RETRY_COUNT times"
        );
    }

    #[tokio::test]
    async fn in_memory_upload_is_retried() {
        use std::sync::atomic::Ordering;

        let (url, hits, server) = spawn_mock_returning_503(EXPECTED_ATTEMPTS);

        let archive = ProfileArchive::new_compressed_in_memory(b"profile-archive".to_vec())
            .await
            .unwrap();

        let result = upload_profile_archive(&multipart_upload_data_for(&url, 1), archive).await;
        server.join().unwrap();

        assert!(result.is_err(), "a 503 should surface as an error");
        assert_eq!(
            hits.load(Ordering::SeqCst),
            EXPECTED_ATTEMPTS,
            "in-memory upload should be attempted 1 + UPLOAD_RETRY_COUNT times"
        );
    }

    struct RecordedRequest {
        method: String,
        path: String,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    }

    /// Serves the next `max_conns` connections, answering each request with the
    /// response `respond` builds for it, and returns every request it received.
    fn spawn_recording_mock(
        max_conns: usize,
        respond: impl Fn(&RecordedRequest) -> String + Send + 'static,
    ) -> (String, std::thread::JoinHandle<Vec<RecordedRequest>>) {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());

        let handle = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for stream in listener.incoming().take(max_conns) {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());

                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut parts = request_line.split_whitespace();
                let method = parts.next().unwrap().to_string();
                let path = parts.next().unwrap().to_string();

                let mut headers = BTreeMap::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    let (name, value) = line.split_once(':').unwrap();
                    headers.insert(name.to_lowercase(), value.trim().to_string());
                }
                let content_length: usize = headers
                    .get("content-length")
                    .map_or(0, |value| value.parse().unwrap());
                let mut body = vec![0u8; content_length];
                reader.read_exact(&mut body).unwrap();

                let request = RecordedRequest {
                    method,
                    path,
                    headers,
                    body,
                };
                stream.write_all(respond(&request).as_bytes()).unwrap();
                requests.push(request);
            }
            requests
        });

        (base_url, handle)
    }

    fn ok_response(extra_headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn multipart_archive(content: &[u8], part_size: u64, in_memory: bool) -> ProfileArchive {
        let encode = |data: &[u8]| {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(md5::compute(data).0)
        };
        let archive_content = if in_memory {
            ProfileArchiveContent::CompressedInMemory {
                data: content.to_vec().into(),
            }
        } else {
            let path = tempfile::NamedTempFile::new()
                .unwrap()
                .into_temp_path()
                .keep()
                .unwrap();
            std::fs::write(&path, content).unwrap();
            ProfileArchiveContent::UncompressedOnDisk { path }
        };
        ProfileArchive {
            md5: encode(content),
            content: archive_content,
            multipart: ProfileMultipart {
                size: content.len() as u64,
                part_size,
                part_md5s: content.chunks(part_size as usize).map(encode).collect(),
            },
        }
    }

    fn multipart_upload_data_for(base_url: &str, part_count: usize) -> UploadData {
        UploadData {
            status: "success".to_string(),
            multipart_upload_urls: MultipartUploadUrls {
                part_urls: (1..=part_count)
                    .map(|part| format!("{base_url}/part/{part}"))
                    .collect(),
                complete_url: format!("{base_url}/complete"),
            },
            run_id: "test-run".to_string(),
        }
    }

    #[tokio::test]
    async fn multipart_upload_sends_each_part_then_completes() {
        assert_multipart_upload_sends_each_part_then_completes(false).await;
    }

    #[tokio::test]
    async fn multipart_upload_of_in_memory_archive_sends_each_part_then_completes() {
        assert_multipart_upload_sends_each_part_then_completes(true).await;
    }

    async fn assert_multipart_upload_sends_each_part_then_completes(in_memory: bool) {
        let content = b"0123456789";
        let archive = multipart_archive(content, 4, in_memory);
        let part_md5s = archive.multipart.part_md5s.clone();

        let (base_url, server) = spawn_recording_mock(4, |request| {
            if request.method == "PUT" {
                let part = request.path.trim_start_matches("/part/");
                ok_response(&format!("ETag: \"etag-{part}\"\r\n"), "")
            } else {
                ok_response("", "<CompleteMultipartUploadResult/>")
            }
        });

        upload_profile_archive(&multipart_upload_data_for(&base_url, 3), archive)
            .await
            .unwrap();
        let requests = server.join().unwrap();

        let expected_parts: [&[u8]; 3] = [b"0123", b"4567", b"89"];
        // Parts are uploaded concurrently, so they can reach the server in any order
        for (index, expected_body) in expected_parts.iter().enumerate() {
            let path = format!("/part/{}", index + 1);
            let request = requests[..3]
                .iter()
                .find(|request| request.path == path)
                .unwrap();
            assert_eq!(request.method, "PUT");
            assert_eq!(request.body, *expected_body);
            assert_eq!(request.headers["content-md5"], part_md5s[index]);
        }

        let complete = &requests[3];
        assert_eq!(complete.method, "POST");
        assert_eq!(complete.path, "/complete");
        assert_eq!(
            String::from_utf8(complete.body.clone()).unwrap(),
            "<CompleteMultipartUpload>\
             <Part><PartNumber>1</PartNumber><ETag>\"etag-1\"</ETag></Part>\
             <Part><PartNumber>2</PartNumber><ETag>\"etag-2\"</ETag></Part>\
             <Part><PartNumber>3</PartNumber><ETag>\"etag-3\"</ETag></Part>\
             </CompleteMultipartUpload>"
        );
    }

    #[tokio::test]
    async fn multipart_upload_retries_completion_error_body() {
        let archive = multipart_archive(b"0123", 4, false);

        let (base_url, server) = spawn_recording_mock(1 + EXPECTED_ATTEMPTS, |request| {
            if request.method == "PUT" {
                ok_response("ETag: \"etag-1\"\r\n", "")
            } else {
                ok_response("", "<Error><Code>InternalError</Code></Error>")
            }
        });

        let result =
            upload_profile_archive(&multipart_upload_data_for(&base_url, 1), archive).await;
        let requests = server.join().unwrap();

        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("Failed to complete the performance report upload"),
            "unexpected error: {error}"
        );
        let completion_attempts = requests
            .iter()
            .filter(|request| request.path == "/complete")
            .count();
        assert_eq!(completion_attempts, EXPECTED_ATTEMPTS);
    }

    #[tokio::test]
    async fn multipart_upload_does_not_retry_permanent_completion_error() {
        let archive = multipart_archive(b"0123", 4, false);

        let (base_url, server) = spawn_recording_mock(2, |request| {
            if request.method == "PUT" {
                ok_response("ETag: \"etag-1\"\r\n", "")
            } else {
                ok_response("", "<Error><Code>InvalidPart</Code></Error>")
            }
        });

        let result =
            upload_profile_archive(&multipart_upload_data_for(&base_url, 1), archive).await;
        let requests = server.join().unwrap();

        let error = result.unwrap_err().to_string();
        assert!(error.contains("InvalidPart"), "unexpected error: {error}");
        let completion_attempts = requests
            .iter()
            .filter(|request| request.path == "/complete")
            .count();
        assert_eq!(completion_attempts, 1);
    }

    #[tokio::test]
    async fn multipart_upload_completes_after_a_retried_error_body() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let archive = multipart_archive(b"0123", 4, false);

        let completion_attempts = AtomicUsize::new(0);
        let (base_url, server) = spawn_recording_mock(3, move |request| {
            if request.method == "PUT" {
                ok_response("ETag: \"etag-1\"\r\n", "")
            } else if completion_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                ok_response("", "<Error><Code>InternalError</Code></Error>")
            } else {
                ok_response("", "<CompleteMultipartUploadResult/>")
            }
        });

        upload_profile_archive(&multipart_upload_data_for(&base_url, 1), archive)
            .await
            .unwrap();
        server.join().unwrap();
    }
}
