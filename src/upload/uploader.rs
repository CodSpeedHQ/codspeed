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
use reqwest::StatusCode;
use reqwest_retry::{
    DefaultRetryableStrategy, RetryDecision, RetryPolicy, Retryable, RetryableStrategy,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::SystemTime;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio_tar::Builder;

use super::interfaces::{UploadData, UploadMetadata};
use super::profile_archive::ProfileArchive;

fn bytes_to_mib(bytes: u64) -> u64 {
    bytes / (1024 * 1024)
}

/// Maximum uncompressed profile folder size in MiB before compression is required
const MAX_UNCOMPRESSED_PROFILE_SIZE_BYTES: u64 = 1024 * 1024 * 1024 * 5; // 5 GiB

/// Calculate the total size of a directory in bytes
async fn calculate_folder_size(path: &std::path::Path) -> Result<u64> {
    let mut total_size = 0u64;
    let mut dirs_to_process = vec![path.to_path_buf()];

    while let Some(current_dir) = dirs_to_process.pop() {
        let mut entries = tokio::fs::read_dir(&current_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let metadata = entry.metadata().await?;
            if metadata.is_file() {
                total_size += metadata.len();
            } else if metadata.is_dir() {
                dirs_to_process.push(entry.path());
            }
        }
    }

    Ok(total_size)
}

/// Create a profile archive from the profile folder and return its md5 hash encoded in base64
///
/// For Valgrind, we create a gzip-compressed tar archive of the entire profile folder.
/// For WallTime, we check the folder size and create either a compressed or uncompressed tar archive
/// based on the MAX_UNCOMPRESSED_PROFILE_SIZE_BYTES threshold.
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
            ProfileArchive::new_compressed_in_memory(data)
        }
        ExecutorName::Memory | ExecutorName::WallTime => {
            // Check folder size to decide on compression
            let folder_size_bytes = calculate_folder_size(profile_folder).await?;
            let should_compress = folder_size_bytes >= MAX_UNCOMPRESSED_PROFILE_SIZE_BYTES;

            let temp_file = tempfile::NamedTempFile::new()?;
            let temp_path = temp_file.path().to_path_buf();

            // Create a tokio File handle to the temporary file
            let file = File::create(&temp_path).await?;

            // Persist the temporary file to prevent deletion when temp_file goes out of scope
            let persistent_path = temp_file.into_temp_path().keep()?;

            if should_compress {
                debug!(
                    "Profile folder size ({} MiB) exceeds threshold ({} MiB), creating compressed tar.gz archive on disk",
                    bytes_to_mib(folder_size_bytes),
                    bytes_to_mib(MAX_UNCOMPRESSED_PROFILE_SIZE_BYTES)
                );
                let enc = GzipEncoder::new(file);
                let mut tar = Builder::new(enc);
                tar.append_dir_all(".", profile_folder).await?;
                let mut gzip_encoder = tar.into_inner().await?;
                gzip_encoder.shutdown().await?;
                gzip_encoder.into_inner().sync_all().await?;

                ProfileArchive::new_compressed_on_disk(persistent_path)?
            } else {
                debug!(
                    "Profile folder size ({} MiB) is below threshold ({} MiB), creating uncompressed tar archive on disk",
                    bytes_to_mib(folder_size_bytes),
                    bytes_to_mib(MAX_UNCOMPRESSED_PROFILE_SIZE_BYTES)
                );
                let mut tar = Builder::new(file);
                tar.append_dir_all(".", profile_folder).await?;
                tar.into_inner().await?.sync_all().await?;

                ProfileArchive::new_uncompressed_on_disk(persistent_path)?
            }
        }
    };

    debug!(
        "Created archive ({} bytes) in {:.2?}",
        profile_archive.content.size().await?,
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
                    let additional_message = match upload_metadata.run_environment {
                        RunEnvironment::GithubActions => {
                            "Check that the workflow is correctly authenticated. View more at https://codspeed.io/docs/integrations/ci/github-actions/configuration#authentication"
                        }
                        RunEnvironment::GitlabCi => {
                            "Check that the CI job is correctly authenticated. View more at https://codspeed.io/docs/integrations/ci/gitlab-ci/configuration#authentication"
                        }
                        RunEnvironment::Buildkite => {
                            "Check that CODSPEED_TOKEN is set and has the correct value"
                        }
                        RunEnvironment::Local => {
                            "Run `codspeed auth login` to authenticate the CLI"
                        }
                    };
                    error_message.push_str(&format!("\n\n{additional_message}"));
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

/// The retry middleware can't replay a consumed stream, so we rebuild the body from
/// disk on each attempt. Response-level errors (4xx/5xx) are left for the caller.
async fn send_streamed_with_retry(
    upload_data: &UploadData,
    path: &std::path::Path,
    archive_size: u64,
    archive_hash: &str,
    encoding: Option<String>,
) -> Result<reqwest::Response> {
    let policy = upload_backoff();
    let start = SystemTime::now();
    let mut n_past_retries = 0;

    loop {
        let file = File::open(path)
            .await
            .context(format!("Failed to open file at path: {}", path.display()))?;
        let stream = tokio_util::io::ReaderStream::new(file);
        let body = reqwest::Body::wrap_stream(stream);

        let mut request = STREAMING_CLIENT
            .put(upload_data.upload_url.clone())
            .header("Content-Type", "application/x-tar")
            .header("Content-Length", archive_size)
            .header("Content-MD5", archive_hash);
        if let Some(encoding) = &encoding {
            request = request.header("Content-Encoding", encoding);
        }

        let result = request
            .body(body)
            .send()
            .await
            .map_err(reqwest_middleware::Error::Reqwest);

        let is_transient = matches!(
            DefaultRetryableStrategy.handle(&result),
            Some(Retryable::Transient)
        );
        if is_transient {
            if let RetryDecision::Retry { execute_after } =
                policy.should_retry(start, n_past_retries)
            {
                let wait = execute_after
                    .duration_since(SystemTime::now())
                    .unwrap_or_default();
                debug!("Streamed upload attempt failed (transient), retrying in {wait:?}");
                tokio::time::sleep(wait).await;
                n_past_retries += 1;
                continue;
            }
        }

        return Ok(result?);
    }
}

async fn upload_profile_archive(
    upload_data: &UploadData,
    profile_archive: ProfileArchive,
) -> Result<()> {
    let archive_size = profile_archive.content.size().await?;
    let archive_hash = profile_archive.hash;

    let response = match &profile_archive.content {
        content @ ProfileArchiveContent::CompressedInMemory { data } => {
            // Use regular client with retry middleware for compressed data
            let mut request = REQUEST_CLIENT
                .put(upload_data.upload_url.clone())
                .header("Content-Type", "application/x-tar")
                .header("Content-Length", archive_size)
                .header("Content-MD5", archive_hash);

            if let Some(encoding) = content.encoding() {
                request = request.header("Content-Encoding", encoding);
            }

            request.body(data.clone()).send().await?
        }
        content @ ProfileArchiveContent::UncompressedOnDisk { path }
        | content @ ProfileArchiveContent::CompressedOnDisk { path } => {
            send_streamed_with_retry(
                upload_data,
                path,
                archive_size,
                &archive_hash,
                content.encoding(),
            )
            .await?
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await?;
        bail!(
            "Failed to upload performance report: {}\n  -> {} {}",
            status,
            style("Reason:").bold(),
            style(error_text).red()
        );
    }

    Ok(())
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
    /// the URL, a counter of connections received, and the server's join handle.
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
        let url = format!("http://{}/upload", listener.local_addr().unwrap());
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

    fn upload_data_for(url: String) -> UploadData {
        UploadData {
            status: "success".to_string(),
            upload_url: url,
            run_id: "test-run".to_string(),
        }
    }

    /// On-disk archives stream through `send_streamed_with_retry`, which retries
    /// transient failures itself since `STREAMING_CLIENT` has no retry middleware.
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
        let archive = ProfileArchive::new_uncompressed_on_disk(path).unwrap();

        let result = upload_profile_archive(&upload_data_for(url), archive).await;
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

    /// In-memory archives go through `REQUEST_CLIENT`, whose retry middleware handles
    /// transient failures.
    #[tokio::test]
    async fn in_memory_upload_is_retried() {
        use std::sync::atomic::Ordering;

        let (url, hits, server) = spawn_mock_returning_503(EXPECTED_ATTEMPTS);

        let archive = ProfileArchive::new_compressed_in_memory(b"profile-archive".to_vec());

        let result = upload_profile_archive(&upload_data_for(url), archive).await;
        server.join().unwrap();

        assert!(result.is_err(), "a 503 should surface as an error");
        assert_eq!(
            hits.load(Ordering::SeqCst),
            EXPECTED_ATTEMPTS,
            "in-memory upload should be attempted 1 + UPLOAD_RETRY_COUNT times"
        );
    }
}
