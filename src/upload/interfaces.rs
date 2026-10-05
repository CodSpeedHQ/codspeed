use serde::{Deserialize, Serialize};

use crate::executor::ExecutorName;
use crate::instruments::InstrumentName;
use crate::run_environment::{RepositoryProvider, RunEnvironment, RunEnvironmentMetadata, RunPart};
use crate::system::SystemInfo;

pub const LATEST_UPLOAD_METADATA_VERSION: u32 = 12;

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UploadMetadata {
    pub repository_provider: RepositoryProvider,
    pub version: Option<u32>,
    pub tokenless: bool,
    pub profile_md5: String,
    pub profile_encoding: Option<String>,
    pub profile_multipart: ProfileMultipart,
    pub runner: Runner,
    pub run_environment: RunEnvironment,
    pub run_part: Option<RunPart>,
    pub commit_hash: String,
    pub allow_empty: bool,
    #[serde(flatten)]
    pub run_environment_metadata: RunEnvironmentMetadata,
}

/// Layout of a profile archive uploaded as an S3 multipart upload, in consecutive
/// `part_size` chunks, the last one holding the remainder. S3 requires parts of 5 MiB
/// to 5 GiB (the last one excepted from the minimum), and at most 10,000 of them.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileMultipart {
    pub size: u64,
    pub part_size: u64,
    /// Base64 md5 of each part, in upload order
    pub part_md5s: Vec<String>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Runner {
    pub name: String,
    pub version: String,
    pub instruments: Vec<InstrumentName>,
    pub executor: ExecutorName,
    /// Whether memory allocation time is excluded from results. Part of the run's
    /// measurement configuration: runs with different values are not comparable.
    ///
    /// Skipped when `false` so runs that don't exclude allocations stay
    /// byte-identical to legacy runs that predate this field.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub exclude_allocations: bool,
    #[serde(flatten)]
    pub system_info: SystemInfo,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UploadData {
    pub status: String,
    pub multipart_upload_urls: MultipartUploadUrls,
    pub run_id: String,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MultipartUploadUrls {
    /// Presigned S3 `UploadPart` URLs, one per part, in upload order
    pub part_urls: Vec<String>,
    /// Presigned S3 `CompleteMultipartUpload` URL
    pub complete_url: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UploadError {
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multipart_upload_response() {
        let upload_data: UploadData = serde_json::from_str(
            r#"{
                "status": "success",
                "runId": "run-id",
                "multipartUploadUrls": {
                    "partUrls": ["https://part/1", "https://part/2"],
                    "completeUrl": "https://complete"
                }
            }"#,
        )
        .unwrap();

        assert_eq!(upload_data.run_id, "run-id");
        let urls = upload_data.multipart_upload_urls;
        assert_eq!(urls.part_urls, ["https://part/1", "https://part/2"]);
        assert_eq!(urls.complete_url, "https://complete");
    }
}
