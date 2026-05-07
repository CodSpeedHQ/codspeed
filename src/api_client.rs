use std::fmt::Display;

use crate::executor::ExecutorName;
use crate::prelude::*;
use crate::run_environment::RepositoryProvider;
use console::style;
use gql_client::{Client as GQLClient, ClientConfig};
use nestify::nest;
use serde::{Deserialize, Serialize};

pub struct CodSpeedAPIClient {
    gql_client: GQLClient,
    unauthenticated_gql_client: GQLClient,
    api_url: String,
    /// The token this client authenticates with. Exposed so downstream
    /// consumers (the uploader's `Authorization` header, the executor's
    /// `CODSPEED_OAUTH_TOKEN` env injection) don't have to thread the
    /// token separately from the client.
    token: Option<String>,
}

impl CodSpeedAPIClient {
    /// Build a client authenticated with `token` (when `Some`).
    ///
    /// The CLI resolves the effective token at construction time, so
    /// callers downstream (the uploader, the executor's env injection,
    /// every GraphQL caller) just consume it from the client through
    /// [`Self::token`] and don't have to thread the token separately.
    pub fn new(token: Option<String>, api_url: String) -> Self {
        Self {
            gql_client: build_gql_api_client(token.as_deref(), api_url.clone()),
            unauthenticated_gql_client: build_gql_api_client(None, api_url.clone()),
            api_url,
            token,
        }
    }

    /// Returns a client that uses `token` for authentication, regardless of
    /// the token this client was built with.
    pub fn with_token(&self, token: String) -> Self {
        Self::new(Some(token), self.api_url.clone())
    }

    /// The token this client currently authenticates with, if any.
    ///
    /// Note: this is not necessarily the token the client was built with —
    /// in CI with OIDC, [`Self::set_token`] is called before each upload to
    /// rotate the credentials. See [`crate::run_environment::RunEnvironmentProvider::refresh_token`].
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Replace the token this client uses for authenticated GraphQL
    /// requests and that the uploader pulls for its `Authorization`
    /// header. The single mutation point for the credentials.
    pub fn set_token(&mut self, token: Option<String>) {
        self.gql_client = build_gql_api_client(token.as_deref(), self.api_url.clone());
        self.token = token;
    }
}

fn build_gql_api_client(token: Option<&str>, api_url: String) -> GQLClient {
    let headers = match token {
        Some(token) => {
            let mut headers = std::collections::HashMap::new();
            headers.insert("Authorization".to_string(), token.to_owned());
            headers
        }
        None => Default::default(),
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
    pub struct BenchmarkIssues {
        pub callgraph_generation_failure: Option<String>,
    }
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
            pub issues: Option<BenchmarkIssues>,
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
        pub result: Option<pub struct CompareRunsBenchmarkResultDetail {
            pub issues: Option<BenchmarkIssues>,
        }>,
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

nest! {
    #[derive(Debug, Deserialize, Serialize, Clone)]*
    #[serde(rename_all = "camelCase")]*
    pub struct SessionPayload {
        pub user: Option<pub struct SessionUser {
            pub login: String,
            pub provider: RepositoryProvider,
        }>,
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionData {
    session: SessionPayload,
}

/// Outcome of [`CodSpeedAPIClient::session`]. The CLI distinguishes
/// "no/expired token" from any other error so it can render a clear message.
pub enum SessionError {
    /// Token is missing or no longer valid.
    Unauthenticated,
    /// Anything else (network, server error, etc).
    Other(anyhow::Error),
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryOverviewPayload {
    pub owner: String,
    pub name: String,
    pub provider: RepositoryProvider,
    pub has_write_access: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionAndRepositoryOverviewVars {
    pub owner: String,
    pub name: String,
    pub provider: Option<RepositoryProvider>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionAndRepositoryOverviewData {
    session: SessionPayload,
    repository_overview: Option<RepositoryOverviewPayload>,
}

pub struct SessionAndRepositoryOverview {
    pub session: SessionPayload,
    pub repository_overview: Option<RepositoryOverviewPayload>,
}

/// Outcome of [`CodSpeedAPIClient::session_and_repository_overview`]. A
/// missing repository is folded into the success path (`repository_overview`
/// becomes `None`); only a missing/expired token or a transport-level
/// failure surfaces here.
pub enum SessionAndRepositoryOverviewError {
    Unauthenticated,
    Other(anyhow::Error),
}

impl CodSpeedAPIClient {
    /// Introspect the token currently configured on this client.
    ///
    /// Returns the linked user (when applicable). Used to verify a token's
    /// validity without conflating it with repository-level access checks —
    /// those are done with [`Self::session_and_repository_overview`].
    pub async fn session(&self) -> std::result::Result<SessionPayload, SessionError> {
        let response = self
            .gql_client
            .query_unwrap::<SessionData>(include_str!("queries/Session.gql"))
            .await;
        match response {
            Ok(data) => Ok(data.session),
            Err(err) if err.contains_error_code("UNAUTHENTICATED") => {
                Err(SessionError::Unauthenticated)
            }
            Err(err) => Err(SessionError::Other(anyhow!(
                "Failed to validate token: {err}"
            ))),
        }
    }

    /// Validate the token and look up a candidate repository in one
    /// round-trip. Used by `auth status` (with a detected git remote) and
    /// the up-front check in `run`/`exec`.
    ///
    /// `repositoryOverview` is nullable in the schema, so a missing
    /// repository surfaces as `repository_overview: None` on the success
    /// path. The server still returns a `REPOSITORY_NOT_FOUND` error in
    /// that case to avoid leaking existence info, but the partial-data
    /// payload carries the `session` field — we deserialize it from the
    /// error and treat the call as successful for the session's purposes.
    pub async fn session_and_repository_overview(
        &self,
        vars: SessionAndRepositoryOverviewVars,
    ) -> std::result::Result<SessionAndRepositoryOverview, SessionAndRepositoryOverviewError> {
        let response = self
            .gql_client
            .query_with_vars_unwrap::<
                SessionAndRepositoryOverviewData,
                SessionAndRepositoryOverviewVars,
            >(
                include_str!("queries/SessionAndRepositoryOverview.gql"),
                vars,
            )
            .await;
        match response {
            Ok(data) => Ok(SessionAndRepositoryOverview {
                session: data.session,
                repository_overview: data.repository_overview,
            }),
            Err(err) if err.contains_error_code("UNAUTHENTICATED") => {
                Err(SessionAndRepositoryOverviewError::Unauthenticated)
            }
            Err(err) if err.contains_error_code("REPOSITORY_NOT_FOUND") => {
                match err.data::<SessionAndRepositoryOverviewData>() {
                    Some(Ok(data)) => Ok(SessionAndRepositoryOverview {
                        session: data.session,
                        repository_overview: None,
                    }),
                    Some(Err(decode_err)) => Err(SessionAndRepositoryOverviewError::Other(
                        anyhow!("Failed to deserialize partial response data: {decode_err}"),
                    )),
                    None => Err(SessionAndRepositoryOverviewError::Other(anyhow!(
                        "Server returned REPOSITORY_NOT_FOUND without partial data: {err}"
                    ))),
                }
            }
            Err(err) => Err(SessionAndRepositoryOverviewError::Other(anyhow!(
                "Failed to validate token and repository: {err}"
            ))),
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
        Self::new(None, api_url)
    }
}
