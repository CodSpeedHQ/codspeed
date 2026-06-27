use serde::{Deserialize, Serialize};

use crate::executor::ExecutorName;
use crate::instruments::InstrumentName;
use crate::run_environment::{RepositoryProvider, RunEnvironment, RunEnvironmentMetadata, RunPart};
use crate::system::SystemInfo;

pub const LATEST_UPLOAD_METADATA_VERSION: u32 = 10;

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UploadMetadata {
    pub repository_provider: RepositoryProvider,
    pub version: Option<u32>,
    pub tokenless: bool,
    pub profile_md5: String,
    pub profile_encoding: Option<String>,
    pub runner: Runner,
    pub run_environment: RunEnvironment,
    pub run_part: Option<RunPart>,
    pub commit_hash: String,
    pub allow_empty: bool,
    #[serde(flatten)]
    pub run_environment_metadata: RunEnvironmentMetadata,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Runner {
    pub name: String,
    pub version: String,
    pub instruments: Vec<InstrumentName>,
    pub executor: ExecutorName,
    #[serde(flatten)]
    pub system_info: SystemInfo,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UploadData {
    pub status: String,
    pub upload_url: String,
    pub run_id: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UploadError {
    pub error: String,
}
