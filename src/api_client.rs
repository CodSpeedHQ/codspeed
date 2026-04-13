use std::fmt::Display;

use crate::executor::ExecutorName;
use crate::prelude::*;
use crate::run_environment::RepositoryProvider;
use crate::{cli::Cli, config::CodSpeedConfig};
use console::style;
use gql_client::{Client as GQLClient, ClientConfig};
use nestify::nest;
use serde::{Deserialize, Serialize};

pub struct CodSpeedAPIClient {
    gql_client: GQLClient,
    unauthenticated_gql_client: GQLClient,
}

impl TryFrom<(&Cli, &CodSpeedConfig)> for CodSpeedAPIClient {
    type Error = Error;
    fn try_from((args, codspeed_config): (&Cli, &CodSpeedConfig)) -> Result<Self> {
        Ok(Self {
            gql_client: build_gql_api_client(codspeed_config, args.api_url.clone(), true),
            unauthenticated_gql_client: build_gql_api_client(
                codspeed_config,
                args.api_url.clone(),
                false,
            ),
        })
    }
}

fn build_gql_api_client(
    codspeed_config: &CodSpeedConfig,
    api_url: String,
    with_auth: bool,
) -> GQLClient {
    let headers = if with_auth && codspeed_config.auth.token.is_some() {
        let mut headers = std::collections::HashMap::new();
        headers.insert(
            "Authorization".to_string(),
            codspeed_config.auth.token.clone().unwrap(),
        );
        headers
    } else {
        Default::default()
    };

    GQLClient::new_with_config(ClientConfig {
        endpoint: api_url,
        // Slightly high to account for cold starts
        timeout: Some(20),
        headers: Some(headers),
        proxy: None,
    })
}

nest! {
    #[derive(Debug, Deserialize, Serialize)]*
    #[serde(rename_all = "camelCase")]*
    struct CreateLoginSessionData {
        create_login_session: pub struct CreateLoginSessionPayload {
            pub callback_url: String,
            pub session_id: String,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConsumeLoginSessionVars {
    session_id: String,
}
nest! {
    #[derive(Debug, Deserialize, Serialize)]*
    #[serde(rename_all = "camelCase")]*
    struct ConsumeLoginSessionData {
        consume_login_session: pub struct ConsumeLoginSessionPayload {
            pub token: Option<String>
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FetchLocalRunVars {
    pub owner: String,
    pub name: String,
    pub run_id: String,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RunStatus {
    Completed,
    Failure,
    Pending,
    Processing,
}

// Custom deserializer to convert string values to i64
fn deserialize_i64_from_string<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;
    let s = String::deserialize(deserializer)?;
    s.parse().map_err(de::Error::custom)
}

nest! {
    #[derive(Debug, Deserialize, Serialize)]*
    #[serde(rename_all = "camelCase")]*
    pub struct FetchLocalRunRun {
        pub id: String,
        pub status: RunStatus,
        pub url: String,
        pub results: Vec<pub struct FetchLocalRunBenchmarkResult {
            pub value: f64,
            pub benchmark: pub struct FetchLocalRunBenchmark {
                pub name: String,
                pub executor: ExecutorName,
            },
            pub valgrind: Option<pub struct ValgrindResult {
                pub time_distribution: Option<pub struct TimeDistribution {
                    pub ir: f64,
                    pub l1m: f64,
                    pub llm: f64,
                    pub sys: f64,
                }>,
            }>,
            pub walltime: Option<pub struct WallTimeResult {
                pub iterations: f64,
                pub stdev: f64,
                pub total_time: f64,
            }>,
            pub memory: Option<pub struct MemoryResult {
                #[serde(deserialize_with = "deserialize_i64_from_string")]
                pub peak_memory: i64,
                #[serde(deserialize_with = "deserialize_i64_from_string")]
                pub total_allocated: i64,
                #[serde(deserialize_with = "deserialize_i64_from_string")]
                pub alloc_calls: i64,
            }>,
        }>,
    }
}

nest! {
    #[derive(Debug, Deserialize, Serialize)]*
    #[serde(rename_all = "camelCase")]*
    struct FetchLocalRunData {
        repository: struct FetchLocalRunRepository {
            run: FetchLocalRunRun,
        }
    }
}

pub struct FetchLocalRunResponse {
    pub run: FetchLocalRunRun,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CompareRunsVars {
    pub owner: String,
    pub name: String,
    pub base_run_id: String,
    pub head_run_id: String,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub enum ResultComparisonCategory {
    Acknowledged,
    Archived,
    Ignored,
    Improvement,
    New,
    Regression,
    Skipped,
    Untouched,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub enum BenchmarkReportStatus {
    Improvement,
    Missing,
    New,
    NoChange,
    Regression,
}

impl Display for BenchmarkReportStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BenchmarkReportStatus::Improvement => {
                write!(f, "{}", style("Improvement").green().bold())
            }
            BenchmarkReportStatus::Missing => write!(f, "{}", style("Missing").yellow().bold()),
            BenchmarkReportStatus::New => write!(f, "{}", style("New").cyan().bold()),
            BenchmarkReportStatus::NoChange => write!(f, "{}", style("No Change").dim()),
            BenchmarkReportStatus::Regression => write!(f, "{}", style("Regression").red().bold()),
        }
    }
}

nest! {
    #[derive(Debug, Deserialize, Serialize)]*
    #[serde(rename_all = "camelCase")]*
    pub struct CompareRunsBenchmarkResult {
        pub value: Option<f64>,
        pub base_value: Option<f64>,
        pub change: Option<f64>,
        pub category: ResultComparisonCategory,
        pub status: BenchmarkReportStatus,
        pub benchmark: pub struct CompareRunsBenchmark {
            pub name: String,
            pub executor: ExecutorName,
        },
    }
}

nest! {
    #[derive(Debug, Deserialize, Serialize)]*
    #[serde(rename_all = "camelCase")]*
    pub struct CompareRunsHeadRun {
        pub id: String,
        pub status: RunStatus,
    }
}

nest! {
    #[derive(Debug, Deserialize, Serialize)]*
    #[serde(rename_all = "camelCase")]*
    struct CompareRunsData {
        repository: struct CompareRunsRepository {
            paginated_compare_runs: pub struct CompareRunsComparison {
                pub impact: Option<f64>,
                pub url: String,
                pub head_run: CompareRunsHeadRun,
                pub result_comparisons: Vec<CompareRunsBenchmarkResult>,
            },
        }
    }
}

pub struct CompareRunsResponse {
    pub comparison: CompareRunsComparison,
}

pub enum CompareRunsOutcome {
    Success(CompareRunsResponse),
    BaseRunNotFound,
    ExecutorMismatch,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GetOrCreateProjectRepositoryVars {
    pub name: String,
}

nest! {
    #[derive(Debug, Deserialize, Serialize, Clone)]*
    #[serde(rename_all = "camelCase")]*
    struct GetOrCreateProjectRepositoryData {
        get_or_create_project_repository: pub struct GetOrCreateProjectRepositoryPayload {
            pub provider: RepositoryProvider,
            pub owner: String,
            pub name: String,
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GetRepositoryVars {
    pub owner: String,
    pub name: String,
    pub provider: RepositoryProvider,
}

nest! {
    #[derive(Debug, Deserialize, Serialize, Clone)]*
    #[serde(rename_all = "camelCase")]*
    struct GetRepositoryData {
        repository_overview: Option<pub struct GetRepositoryPayload {
            pub id: String,
        }>,
        user: Option<pub struct GetRepositoryUser {
            pub id: String,
        }>,
    }
}

nest! {
    #[derive(Debug, Deserialize, Serialize)]*
    #[serde(rename_all = "camelCase")]*
    struct CurrentUserData {
        user: Option<pub struct CurrentUserPayload {
            pub login: String,
            pub provider: RepositoryProvider,
        }>,
    }
}

impl CodSpeedAPIClient {
    pub async fn get_current_user(&self) -> Result<Option<CurrentUserPayload>> {
        let response = self
            .gql_client
            .query_unwrap::<CurrentUserData>(include_str!("queries/CurrentUser.gql"))
            .await;
        match response {
            Ok(data) => Ok(data.user),
            Err(err) => bail!("Failed to get current user: {err}"),
        }
    }

    pub async fn create_login_session(&self) -> Result<CreateLoginSessionPayload> {
        let response = self
            .unauthenticated_gql_client
            .query_unwrap::<CreateLoginSessionData>(include_str!("queries/CreateLoginSession.gql"))
            .await;
        match response {
            Ok(response) => Ok(response.create_login_session),
            Err(err) => bail!("Failed to create login session: {err}"),
        }
    }

    pub async fn consume_login_session(
        &self,
        session_id: &str,
    ) -> Result<ConsumeLoginSessionPayload> {
        let response = self
            .unauthenticated_gql_client
            .query_with_vars_unwrap::<ConsumeLoginSessionData, ConsumeLoginSessionVars>(
                include_str!("queries/ConsumeLoginSession.gql"),
                ConsumeLoginSessionVars {
                    session_id: session_id.to_string(),
                },
            )
            .await;
        match response {
            Ok(response) => Ok(response.consume_login_session),
            Err(err) => bail!("Failed to use login session: {err}"),
        }
    }

    pub async fn compare_runs(&self, vars: CompareRunsVars) -> Result<CompareRunsOutcome> {
        let response = self
            .gql_client
            .query_with_vars_unwrap::<CompareRunsData, CompareRunsVars>(
                include_str!("queries/CompareRuns.gql"),
                vars,
            )
            .await;
        match response {
            Ok(response) => Ok(CompareRunsOutcome::Success(CompareRunsResponse {
                comparison: response.repository.paginated_compare_runs,
            })),
            Err(err) if err.contains_error_code("UNAUTHENTICATED") => {
                bail!("Your session has expired, please login again using `codspeed auth login`")
            }
            Err(err) if err.contains_error_code("RUN_NOT_FOUND") => {
                Ok(CompareRunsOutcome::BaseRunNotFound)
            }
            Err(err) if err.contains_error_code("NOT_FOUND") => {
                Ok(CompareRunsOutcome::ExecutorMismatch)
            }
            Err(err) => bail!("Failed to compare runs: {err:?}"),
        }
    }

    pub async fn fetch_local_run(&self, vars: FetchLocalRunVars) -> Result<FetchLocalRunResponse> {
        let response = self
            .gql_client
            .query_with_vars_unwrap::<FetchLocalRunData, FetchLocalRunVars>(
                include_str!("queries/FetchLocalRun.gql"),
                vars,
            )
            .await;
        match response {
            Ok(response) => Ok(FetchLocalRunResponse {
                run: response.repository.run,
            }),
            Err(err) if err.contains_error_code("UNAUTHENTICATED") => {
                bail!("Your session has expired, please login again using `codspeed auth login`")
            }
            Err(err) => bail!("Failed to fetch local run: {err}"),
        }
    }

    pub async fn get_or_create_project_repository(
        &self,
        vars: GetOrCreateProjectRepositoryVars,
    ) -> Result<GetOrCreateProjectRepositoryPayload> {
        let response = self
            .gql_client
            .query_with_vars_unwrap::<
                GetOrCreateProjectRepositoryData,
                GetOrCreateProjectRepositoryVars,
            >(
                include_str!("queries/GetOrCreateProjectRepository.gql"),
                vars.clone(),
            )
            .await;
        match response {
            Ok(response) => Ok(response.get_or_create_project_repository),
            Err(err) if err.contains_error_code("UNAUTHENTICATED") => {
                bail!("Your session has expired, please login again using `codspeed auth login`")
            }
            Err(err) => bail!("Failed to get or create project repository: {err}"),
        }
    }

    /// Check if a repository exists in CodSpeed.
    /// Returns Some(payload) if the repository exists, None otherwise.
    pub async fn get_repository(
        &self,
        vars: GetRepositoryVars,
    ) -> Result<Option<GetRepositoryPayload>> {
        let response = self
            .gql_client
            .query_with_vars_unwrap::<GetRepositoryData, GetRepositoryVars>(
                include_str!("queries/GetRepository.gql"),
                vars.clone(),
            )
            .await;
        match response {
            Ok(response) => {
                if response.user.is_none() {
                    bail!(
                        "Your session has expired, please login again using `codspeed auth login`"
                    );
                }
                Ok(response.repository_overview)
            }
            Err(err) if err.contains_error_code("REPOSITORY_NOT_FOUND") => Ok(None),
            Err(err) if err.contains_error_code("UNAUTHENTICATED") => {
                bail!("Your session has expired, please login again using `codspeed auth login`")
            }
            Err(err) => {
                bail!("Failed to get repository: {err}")
            }
        }
    }
}

impl CodSpeedAPIClient {
    /// Create a test API client for use in tests
    #[cfg(test)]
    pub fn create_test_client() -> Self {
        Self::create_test_client_with_url("http://localhost:8000/graphql".to_owned())
    }

    /// Create a test API client with a custom URL for use in tests
    #[cfg(test)]
    pub fn create_test_client_with_url(api_url: String) -> Self {
        let codspeed_config = CodSpeedConfig::default();
        Self::try_from((&Cli::test_with_url(api_url), &codspeed_config)).unwrap()
    }
}
