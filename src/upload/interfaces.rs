use std::collections::BTreeMap;

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
    pub profile_archive_metadata: ProfileArchiveMetadata,
    pub runner: Runner,
    pub run_environment: RunEnvironment,
    pub run_part: Option<RunPart>,
    pub commit_hash: String,
    pub allow_empty: bool,
    #[serde(flatten)]
    pub run_environment_metadata: RunEnvironmentMetadata,
}

/// Metadata of the profile archive, uploaded as an S3 multipart upload in consecutive
/// `part_size` chunks, the last one holding the remainder. S3 requires parts of 5 MiB to 5 GiB
/// (the last one excepted from the minimum), and at most 10,000 of them.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileArchiveMetadata {
    /// `Content-Encoding` of the archive, such as `gzip`
    pub encoding: Option<String>,
    pub size: u64,
    /// Base64 big-endian CRC64NVME of the whole archive
    pub crc64nvme: String,
    pub part_size: u64,
    /// Base64 big-endian CRC64NVME of each part, in upload order
    pub part_crc64nvmes: Vec<String>,
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
    pub multipart_upload: MultipartUpload,
    pub run_id: String,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MultipartUpload {
    /// Presigned S3 `UploadPart` requests, one per part, in upload order
    pub parts: Vec<PresignedRequest>,
    /// Presigned S3 `CompleteMultipartUpload` request
    pub complete: PresignedRequest,
}

/// Request presigned by the API, to send to `url` with `headers` as is: they are
/// part of the signature
#[derive(Deserialize, Serialize, Debug)]
pub struct PresignedRequest {
    pub url: String,
    pub headers: BTreeMap<String, String>,
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
                "multipartUpload": {
                    "parts": [
                        { "url": "https://part/1", "headers": { "x-amz-checksum-crc64nvme": "nq48tdaL2no=" } },
                        { "url": "https://part/2", "headers": { "x-amz-checksum-crc64nvme": "XMXoclwBfLo=" } }
                    ],
                    "complete": {
                        "url": "https://complete",
                        "headers": { "x-amz-checksum-type": "FULL_OBJECT" }
                    }
                }
            }"#,
        )
        .unwrap();

        assert_eq!(upload_data.run_id, "run-id");
        let upload = upload_data.multipart_upload;
        let part_urls: Vec<_> = upload.parts.iter().map(|part| part.url.as_str()).collect();
        assert_eq!(part_urls, ["https://part/1", "https://part/2"]);
        assert_eq!(
            upload.parts[1].headers,
            BTreeMap::from([("x-amz-checksum-crc64nvme".into(), "XMXoclwBfLo=".into())])
        );
        assert_eq!(upload.complete.url, "https://complete");
        assert_eq!(
            upload.complete.headers,
            BTreeMap::from([("x-amz-checksum-type".into(), "FULL_OBJECT".into())])
        );
    }
}
