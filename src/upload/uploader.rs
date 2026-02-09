use crate::executor::Config;
use crate::executor::ExecutionContext;
use crate::executor::ExecutorName;
use crate::run_environment::RunEnvironment;
use crate::upload::{UploadError, profile_archive::ProfileArchiveContent};
use crate::{
    prelude::*,
    request_client::{REQUEST_CLIENT, STREAMING_CLIENT},
};
use async_compression::tokio::write::GzipEncoder;
use console::style;
use reqwest::StatusCode;
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
    execution_context: &ExecutionContext,
    executor_name: ExecutorName,
) -> Result<ProfileArchive> {
    let time_start = std::time::Instant::now();
    let profile_archive = match executor_name {
        ExecutorName::Valgrind => {
            debug!("Creating compressed tar archive for Valgrind");
            let enc = GzipEncoder::new(Vec::new());
            let mut tar = Builder::new(enc);
            tar.append_dir_all(".", execution_context.profile_folder.clone())
                .await?;
            let mut gzip_encoder = tar.into_inner().await?;
            gzip_encoder.shutdown().await?;
            let data = gzip_encoder.into_inner();
            ProfileArchive::new_compressed_in_memory(data)
        }
        ExecutorName::Memory | ExecutorName::WallTime => {
            // Check folder size to decide on compression
            let folder_size_bytes =
                calculate_folder_size(&execution_context.profile_folder).await?;
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
                tar.append_dir_all(".", execution_context.profile_folder.clone())
                    .await?;
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
                tar.append_dir_all(".", execution_context.profile_folder.clone())
                    .await?;
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
    config: &Config,
    upload_metadata: &UploadMetadata,
) -> Result<UploadData> {
    let mut upload_request = REQUEST_CLIENT
        .post(config.upload_url.clone())
        .json(&upload_metadata);
    if !upload_metadata.tokenless {
        upload_request = upload_request.header("Authorization", config.token.clone().unwrap());
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
            // Use streaming client without retry middleware for file streams
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

            if let Some(encoding) = content.encoding() {
                request = request.header("Content-Encoding", encoding);
            }

            request.body(body).send().await?
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
    execution_context: &mut ExecutionContext,
    executor_name: ExecutorName,
) -> Result<UploadResult> {
    let profile_archive = create_profile_archive(execution_context, executor_name.clone()).await?;

    debug!(
        "Run Environment provider detected: {:?}",
        execution_context.provider.get_run_environment()
    );

    if !execution_context.is_local() {
        // If relevant, set the OIDC token for authentication
        // Note: OIDC tokens can expire quickly, so we set it just before the upload
        execution_context
            .provider
            .set_oidc_token(&mut execution_context.config)
            .await?;
    }

    let upload_metadata = execution_context
        .provider
        .get_upload_metadata(
            &execution_context.config,
            &execution_context.system_info,
            &profile_archive,
            executor_name,
        )
        .await?;
    debug!("Upload metadata: {upload_metadata:#?}");
    info!(
        "Linked repository: {}",
        style(format!(
            "{}/{}",
            upload_metadata.run_environment_metadata.owner,
            upload_metadata.run_environment_metadata.repository
        ))
        .bold(),
    );
    if upload_metadata.tokenless {
        let hash = upload_metadata.get_hash();
        info!("CodSpeed Run Hash: \"{hash}\"");
    }

    debug!("Preparing upload...");
    let upload_data = retrieve_upload_data(&execution_context.config, &upload_metadata).await?;
    debug!("runId: {}", upload_data.run_id);

    debug!(
        "Uploading {} bytes...",
        profile_archive.content.size().await?
    );
    upload_profile_archive(&upload_data, profile_archive).await?;
    info!(
        "{} Performance data uploaded",
        style("\u{2714}").green().bold()
    );

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
    use crate::config::CodSpeedConfig;
    use std::path::PathBuf;

    // TODO: remove the ignore when implementing network mocking
    #[ignore]
    #[tokio::test]
    async fn test_upload() {
        let config = Config {
            command: "pytest tests/ --codspeed".into(),
            upload_url: Url::parse("change me").unwrap(),
            token: Some("change me".into()),
            profile_folder: Some(PathBuf::from(format!(
                "{}/src/uploader/samples/adrien-python-test",
                env!("CARGO_MANIFEST_DIR")
            ))),
            ..Config::test()
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
                let codspeed_config = CodSpeedConfig::default();
                let api_client = CodSpeedAPIClient::create_test_client();
                let mut execution_context =
                    ExecutionContext::new(config, &codspeed_config, &api_client)
                        .await
                        .expect("Failed to create ExecutionContext for test");
                upload(&mut execution_context, ExecutorName::Valgrind)
                    .await
                    .unwrap();
            },
        )
        .await;
    }
}
