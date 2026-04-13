use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum RepositoryProvider {
    #[default]
    GitHub,
    GitLab,
    Project,
}

impl fmt::Display for RepositoryProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepositoryProvider::GitHub => write!(f, "Github"),
            RepositoryProvider::GitLab => write!(f, "Gitlab"),
            RepositoryProvider::Project => write!(f, "Project"),
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RunEnvironment {
    GithubActions,
    GitlabCi,
    Buildkite,
    Local,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RunEnvironmentMetadata {
    #[serde(rename = "ref")]
    pub ref_: String,
    pub head_ref: Option<String>,
    pub base_ref: Option<String>,
    pub owner: String,
    pub repository: String,
    pub event: RunEvent,
    pub sender: Option<Sender>,
    pub gh_data: Option<GhData>,
    pub gl_data: Option<GlData>,
    pub local_data: Option<LocalData>,
    pub repository_root_path: String,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunEvent {
    Push,
    PullRequest,
    WorkflowDispatch,
    Schedule,
    Local,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GhData {
    pub run_id: String,
    pub job: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GlData {
    pub run_id: String,
    pub job: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LocalData {
    pub expected_run_parts_count: u32,
}

/// Each execution of the CLI maps to a `RunPart`.
///
/// Several `RunParts` can be aggregated in a single `Run` thanks to this data.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunPart {
    /// A unique identifier of the `Run` on the run environment
    ///
    /// For example, the `runId` on Github Actions
    pub run_id: String,

    /// Uniquely identify a `RunPart` within a `Run`.
    ///
    /// This id can be the same between `RunParts` of different `Runs`.
    pub run_part_id: String,

    /// The name of the job. For example, on Github Actions, the workflow name.
    ///
    /// This is **not** unique between executions of the CLI, even between matrix jobs.
    pub job_name: String,

    /// Some relevant metadata.
    ///
    /// This can include matrix and strategy for GithubActions,
    /// some relevant env values.
    ///
    /// We use a `BTreeMap` and not a `HashMap` to keep insert order for
    /// `serde_json` serialization.
    pub metadata: BTreeMap<String, Value>,
}

impl RunPart {
    /// Returns a new `RunPart` with the given suffix appended to `run_part_id`.
    ///
    /// The suffix is a structured key-value map that gets:
    /// - serialized as JSON and appended to `run_part_id` (e.g. `job_name-{"executor":"valgrind","run-index":0}`)
    /// - merged into the `metadata` field
    ///
    /// This is used to differentiate multiple uploads within the same run
    /// (e.g., different executors, or multiple invocations in the same CI job).
    pub fn with_suffix(mut self, suffix: BTreeMap<String, Value>) -> Self {
        if suffix.is_empty() {
            return self;
        }
        let suffix_str = serde_json::to_string(&suffix).expect("Unable to serialize suffix");
        self.run_part_id = format!("{}-{}", self.run_part_id, suffix_str);
        self.metadata.extend(suffix);
        self
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Sender {
    pub id: String,
    pub login: String,
}
