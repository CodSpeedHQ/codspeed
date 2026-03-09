use async_trait::async_trait;
use git2::Repository;
use lazy_static::lazy_static;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use simplelog::SharedLogger;
use std::collections::BTreeMap;
use std::{env, fs};

use crate::cli::run::helpers::{find_repository_root, get_env_variable};
use crate::executor::config::{ExecutorConfig, OrchestratorConfig};
use crate::prelude::*;
use crate::request_client::OIDC_CLIENT;
use crate::run_environment::interfaces::{
    GhData, RepositoryProvider, RunEnvironmentMetadata, RunEvent, Sender,
};
use crate::run_environment::provider::{RunEnvironmentDetector, RunEnvironmentProvider};
use crate::run_environment::{RunEnvironment, RunPart};

use super::logger::GithubActionLogger;

pub struct GitHubActionsProvider {
    pub owner: String,
    pub repository: String,
    pub ref_: String,
    pub head_ref: Option<String>,
    pub base_ref: Option<String>,
    pub sender: Option<Sender>,
    pub gh_data: GhData,
    pub event: RunEvent,
    pub repository_root_path: String,

    /// Indicates whether the head repository is a fork of the base repository.
    is_head_repo_fork: bool,

    /// Indicates whether the repository is private.
    is_repository_private: bool,

    /// OIDC configuration data necessary to request an OIDC token.
    ///
    /// If None, OIDC is not configured for this run.
    oidc_config: Option<OIDCTokenRequestData>,
}

struct OIDCTokenRequestData {
    request_url: String,
    request_token: String,
}

impl GitHubActionsProvider {
    fn get_owner_and_repository() -> Result<(String, String)> {
        let owner_and_repository = get_env_variable("GITHUB_REPOSITORY")?;
        let mut owner_and_repository = owner_and_repository.split('/');
        let owner = owner_and_repository.next().unwrap();
        let repository = owner_and_repository.next().unwrap();
        Ok((owner.into(), repository.into()))
    }
}

#[derive(Deserialize)]
struct OIDCResponse {
    value: Option<String>,
}

lazy_static! {
    static ref PR_REF_REGEX: Regex = Regex::new(r"^refs/pull/(?P<pr_number>\d+)/merge$").unwrap();
}

impl TryFrom<&OrchestratorConfig> for GitHubActionsProvider {
    type Error = Error;
    fn try_from(config: &OrchestratorConfig) -> Result<Self> {
        if config.repository_override.is_some() {
            bail!("Specifying owner and repository from CLI is not supported for Github Actions");
        }
        let (owner, repository) = Self::get_owner_and_repository()?;

        let github_event_path = get_env_variable("GITHUB_EVENT_PATH")?;
        let github_event = fs::read_to_string(github_event_path)?;
        let github_event: Value =
            serde_json::from_str(&github_event).expect("GITHUB_EVENT_PATH file could not be read");

        let ref_ = get_env_variable("GITHUB_REF")?;
        let is_pr = PR_REF_REGEX.is_match(&ref_);

        let is_repository_private = github_event["repository"]["private"].as_bool().unwrap();

        let (head_ref, is_head_repo_fork) = if is_pr {
            let pull_request = github_event["pull_request"].as_object().unwrap();

            let head_repo = pull_request["head"]["repo"].as_object().unwrap();
            let base_repo = pull_request["base"]["repo"].as_object().unwrap();

            let is_head_repo_fork = head_repo["id"] != base_repo["id"];

            let head_ref = if is_head_repo_fork {
                format!(
                    "{}:{}",
                    head_repo["owner"]["login"].as_str().unwrap(),
                    pull_request["head"]["ref"].as_str().unwrap()
                )
            } else {
                pull_request["head"]["ref"].as_str().unwrap().to_owned()
            };
            (Some(head_ref), is_head_repo_fork)
        } else {
            (None, false)
        };

        let github_event_name = get_env_variable("GITHUB_EVENT_NAME")?;
        let event = serde_json::from_str(&format!("\"{github_event_name}\"")).context(format!(
            "Event {github_event_name} is not supported by CodSpeed"
        ))?;
        let repository_root_path = match find_repository_root(&std::env::current_dir()?) {
            Some(mut path) => {
                // Add a trailing slash to the path
                path.push("");
                path.to_string_lossy().to_string()
            }
            None => {
                // Fallback to GITHUB_WORKSPACE, the default repository location when using the checkout action
                // https://docs.github.com/en/actions/reference/workflows-and-actions/variables
                if let Ok(github_workspace) = env::var("GITHUB_WORKSPACE") {
                    format!("{github_workspace}/")
                } else {
                    format!("/home/runner/work/{repository}/{repository}/")
                }
            }
        };

        Ok(Self {
            owner,
            repository: repository.clone(),
            ref_,
            head_ref,
            event,
            gh_data: GhData {
                job: get_env_variable("GITHUB_JOB")?,
                run_id: get_env_variable("GITHUB_RUN_ID")?,
            },
            sender: Some(Sender {
                login: get_env_variable("GITHUB_ACTOR")?,
                id: get_env_variable("GITHUB_ACTOR_ID")?,
            }),
            base_ref: get_env_variable("GITHUB_BASE_REF").ok(),
            repository_root_path,
            is_head_repo_fork,
            is_repository_private,
            oidc_config: None,
        })
    }
}

impl RunEnvironmentDetector for GitHubActionsProvider {
    fn detect() -> bool {
        // check if the GITHUB_ACTIONS environment variable is set and the value is truthy
        env::var("GITHUB_ACTIONS") == Ok("true".into())
    }
}

#[async_trait(?Send)]
impl RunEnvironmentProvider for GitHubActionsProvider {
    fn get_repository_provider(&self) -> RepositoryProvider {
        RepositoryProvider::GitHub
    }

    fn get_logger(&self) -> Box<dyn SharedLogger> {
        Box::new(GithubActionLogger::new())
    }

    fn get_run_environment(&self) -> RunEnvironment {
        RunEnvironment::GithubActions
    }

    fn get_run_environment_metadata(&self) -> Result<RunEnvironmentMetadata> {
        Ok(RunEnvironmentMetadata {
            base_ref: self.base_ref.clone(),
            head_ref: self.head_ref.clone(),
            event: self.event.clone(),
            gh_data: Some(self.gh_data.clone()),
            gl_data: None,
            local_data: None,
            sender: self.sender.clone(),
            owner: self.owner.clone(),
            repository: self.repository.clone(),
            ref_: self.ref_.clone(),
            repository_root_path: self.repository_root_path.clone(),
        })
    }

    /// For Github, the run environment run part is the most complicated
    /// since we support matrix jobs.
    ///
    /// Computing the `run_part_id`:
    /// - not in a matrix:
    ///   - simply take the job name
    /// - in a matrix:
    ///   - take the job name
    ///   - concatenate it with key-values from `matrix` and `strategy`
    ///
    /// `GH_MATRIX` and `GH_STRATEGY` are environment variables computed by
    /// https://github.com/CodSpeedHQ/action:
    /// - `GH_MATRIX`: ${{ toJson(matrix) }}
    /// - `GH_STRATEGY`: ${{ toJson(strategy) }}
    ///
    /// A note on parsing:
    ///
    /// The issue is these variables from Github Actions are multiline.
    /// As we need to use them compute an identifier, we need them as a single line.
    /// Plus we are interested in the content of these objects,
    /// so it makes sense to parse and re-serialize them.
    fn get_run_provider_run_part(&self) -> Option<RunPart> {
        let job_name = self.gh_data.job.clone();

        let mut metadata = BTreeMap::new();

        let gh_matrix = get_env_variable("GH_MATRIX")
            .ok()
            .and_then(|v| serde_json::from_str::<Value>(&v).ok());

        let gh_strategy = get_env_variable("GH_STRATEGY")
            .ok()
            .and_then(|v| serde_json::from_str::<Value>(&v).ok());

        let run_part_id = if let (Some(Value::Object(matrix)), Some(Value::Object(mut strategy))) =
            (gh_matrix, gh_strategy)
        {
            // remove useless values from the strategy
            strategy.remove("fail-fast");
            strategy.remove("max-parallel");

            // The re-serialization is on purpose here. We want to serialize it as a single line.
            let matrix_str = serde_json::to_string(&matrix).expect("Unable to re-serialize matrix");
            let strategy_str =
                serde_json::to_string(&strategy).expect("Unable to re-serialize strategy");

            metadata.extend(matrix);
            metadata.extend(strategy);

            format!("{job_name}-{matrix_str}-{strategy_str}")
        } else {
            job_name
        };

        Some(RunPart {
            run_id: self.gh_data.run_id.clone(),
            run_part_id,
            job_name: self.gh_data.job.clone(),
            metadata,
        })
    }

    fn get_commit_hash(&self, repository_root_path: &str) -> Result<String> {
        let repo = Repository::open(repository_root_path).context(format!(
            "Failed to open repository at path: {repository_root_path}\n\
            Make sure git is installed, and that `actions/checkout` used git to fetch the repository\n\
            If necessary, install git before running `actions/checkout`.\n\
            If you run into permission issues when running in Docker, you may need to also run \
            `git config --global --add safe.directory $GITHUB_WORKSPACE` "
        ))?;

        let commit_hash = repo
            .head()
            .and_then(|head| head.peel_to_commit())
            .context("Failed to get HEAD commit")?
            .id()
            .to_string();
        Ok(commit_hash)
    }

    /// Validate that the environment is correctly configured for OIDC usage.
    ///
    /// ## Logic
    /// - If the user has explicitly set a token in the configuration (i.e. "static token"), inform the user that OIDC is available but do nothing.
    /// - Otherwise, check if the necessary environment variables are set to use OIDC.
    ///
    /// For Github Actions, there are two necessary environment variables:
    /// - `ACTIONS_ID_TOKEN_REQUEST_TOKEN`
    /// - `ACTIONS_ID_TOKEN_REQUEST_URL`
    /// If environment variables are not set, this could be because:
    ///   - The user has misconfigured the workflow (missing `id-token` permission)
    ///   - The run is from a public fork, in which case GitHub Actions does not provide these environment variables for security reasons.
    ///
    /// ## Notes
    /// Retrieving the token requires that the workflow has the `id-token` permission enabled.
    ///
    /// Docs:
    /// - https://docs.github.com/en/actions/how-tos/secure-your-work/security-harden-deployments/oidc-with-reusable-workflows
    /// - https://docs.github.com/en/actions/concepts/security/openid-connect
    /// - https://docs.github.com/en/actions/reference/security/oidc#methods-for-requesting-the-oidc-token
    fn check_oidc_configuration(&mut self, config: &OrchestratorConfig) -> Result<()> {
        // Check if a static token is already set
        if config.token.is_some() {
            announcement!(
                "You can now authenticate your CI workflows using OpenID Connect (OIDC) tokens instead of `CODSPEED_TOKEN` secrets.\n\
                This makes integrating and authenticating jobs safer and simpler.\n\
                Learn more at https://codspeed.io/docs/integrations/ci/github-actions/configuration#oidc-recommended\n"
            );

            return Ok(());
        }

        // The `ACTIONS_ID_TOKEN_REQUEST_TOKEN` environment variable is set when the `id-token` permission is granted, which is necessary to authenticate with OIDC.
        let request_token = get_env_variable("ACTIONS_ID_TOKEN_REQUEST_TOKEN").ok();
        let request_url = get_env_variable("ACTIONS_ID_TOKEN_REQUEST_URL").ok();

        if request_token.is_none() || request_url.is_none() {
            // If the run is from a fork, it is expected that these environment variables are not set.
            // We will fall back to tokenless authentication in this case.
            if self.is_head_repo_fork {
                return Ok(());
            }

            if self.is_repository_private {
                bail!(
                    "Unable to retrieve OIDC token for authentication.\n\
                    Make sure your workflow has the `id-token: write` permission set.\n\
                    See https://codspeed.io/docs/integrations/ci/github-actions/configuration#oidc-recommended"
                )
            }

            announcement!(
                "You can now authenticate your CI workflows using OpenID Connect (OIDC).\n\
                This makes integrating and authenticating jobs safer and simpler.\n\
                Learn more at https://codspeed.io/docs/integrations/ci/github-actions/configuration#oidc-recommended\n"
            );

            return Ok(());
        }

        let request_url = request_url.unwrap();
        let request_token = request_token.unwrap();

        self.oidc_config = Some(OIDCTokenRequestData {
            request_url,
            request_token,
        });

        Ok(())
    }

    /// Request the OIDC token from GitHub Actions if necessary.
    ///
    /// All the validation has already been performed in `check_oidc_configuration`.
    /// So if the oidc_config is None, we simply return.
    async fn set_oidc_token(&self, config: &mut ExecutorConfig) -> Result<()> {
        if let Some(oidc_config) = &self.oidc_config {
            let request_url = format!(
                "{}&audience={}",
                oidc_config.request_url,
                self.get_oidc_audience()
            );

            let token = match OIDC_CLIENT
                .get(request_url)
                .header("Accept", "application/json")
                .header(
                    "Authorization",
                    format!("Bearer {}", oidc_config.request_token),
                )
                .send()
                .await
            {
                Ok(response) => match response.json::<OIDCResponse>().await {
                    Ok(oidc_response) => oidc_response.value,
                    Err(_) => None,
                },
                Err(_) => None,
            };

            if token.is_some() {
                debug!("Successfully retrieved OIDC token for authentication.");
                config.set_token(token);
            } else if self.is_repository_private {
                bail!(
                    "Unable to retrieve OIDC token for authentication. \n\
                    Make sure your workflow has the `id-token: write` permission set. \n\
                    See https://codspeed.io/docs/integrations/ci/github-actions/configuration#oidc-recommended"
                )
            } else {
                warn!("Failed to retrieve OIDC token for authentication.");
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_json_snapshot;
    use temp_env::{with_var, with_vars};

    use crate::VERSION;

    use super::*;

    #[test]
    fn test_detect() {
        with_var("GITHUB_ACTIONS", Some("true"), || {
            assert!(GitHubActionsProvider::detect());
        });
    }

    #[test]
    fn test_get_owner_and_repository() {
        with_var("GITHUB_REPOSITORY", Some("owner/repository"), || {
            let (owner, repository) = GitHubActionsProvider::get_owner_and_repository().unwrap();
            assert_eq!(owner, "owner");
            assert_eq!(repository, "repository");
        });
    }

    #[test]
    fn test_try_from_push_main() {
        with_vars(
            [
                ("GITHUB_ACTOR_ID", Some("1234567890")),
                ("GITHUB_ACTOR", Some("actor")),
                ("GITHUB_BASE_REF", Some("main")),
                ("GITHUB_EVENT_NAME", Some("push")),
                (
                    "GITHUB_EVENT_PATH",
                    Some(
                        format!(
                            "{}/src/run_environment/github_actions/samples/push-event.json",
                            env!("CARGO_MANIFEST_DIR")
                        )
                        .as_str(),
                    ),
                ),
                ("GITHUB_JOB", Some("job")),
                ("GITHUB_REF", Some("refs/heads/main")),
                ("GITHUB_REPOSITORY", Some("owner/repository")),
                ("GITHUB_RUN_ID", Some("1234567890")),
            ],
            || {
                let config = OrchestratorConfig {
                    token: Some("token".into()),
                    ..OrchestratorConfig::test()
                };
                let github_actions_provider = GitHubActionsProvider::try_from(&config).unwrap();
                assert_eq!(github_actions_provider.owner, "owner");
                assert_eq!(github_actions_provider.repository, "repository");
                assert_eq!(github_actions_provider.ref_, "refs/heads/main");
                assert_eq!(github_actions_provider.base_ref, Some("main".into()));
                assert_eq!(github_actions_provider.head_ref, None);
                assert_eq!(github_actions_provider.event, RunEvent::Push);
                assert_eq!(github_actions_provider.gh_data.job, "job");
                assert_eq!(github_actions_provider.gh_data.run_id, "1234567890");
                assert_eq!(
                    github_actions_provider.sender.as_ref().unwrap().login,
                    "actor"
                );
                assert_eq!(
                    github_actions_provider.sender.as_ref().unwrap().id,
                    "1234567890"
                );
                assert!(!github_actions_provider.is_head_repo_fork);
                assert!(!github_actions_provider.is_repository_private);
            },
        )
    }

    #[test]
    fn test_pull_request_run_environment_metadata() {
        with_vars(
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
                            "{}/src/run_environment/github_actions/samples/pr-event.json",
                            env!("CARGO_MANIFEST_DIR")
                        )
                        .as_str(),
                    ),
                ),
                ("GITHUB_HEAD_REF", Some("feat/codspeed-runner")),
                ("GITHUB_JOB", Some("log-env")),
                ("GITHUB_REF", Some("refs/pull/22/merge")),
                (
                    "GITHUB_WORKSPACE",
                    Some("/home/runner/work/adrien-python-test/adrien-python-test"),
                ),
                ("GITHUB_REPOSITORY", Some("my-org/adrien-python-test")),
                ("GITHUB_RUN_ID", Some("6957110437")),
                ("VERSION", Some("0.1.0")),
            ],
            || {
                let config = OrchestratorConfig {
                    token: Some("token".into()),
                    ..OrchestratorConfig::test()
                };
                let github_actions_provider = GitHubActionsProvider::try_from(&config).unwrap();
                assert!(!github_actions_provider.is_head_repo_fork);
                assert!(github_actions_provider.is_repository_private);

                let run_environment_metadata = github_actions_provider
                    .get_run_environment_metadata()
                    .unwrap();
                let run_part = github_actions_provider.get_run_provider_run_part().unwrap();

                assert_json_snapshot!(run_environment_metadata, {
                    ".runner.version" => insta::dynamic_redaction(|value,_path| {
                        assert_eq!(value.as_str().unwrap(), VERSION.to_string());
                        "[version]"
                    }),
                });
                assert_json_snapshot!(run_part);
            },
        );
    }

    #[test]
    fn test_fork_pull_request_run_environment_metadata() {
        with_vars(
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
                            "{}/src/run_environment/github_actions/samples/fork-pr-event.json",
                            env!("CARGO_MANIFEST_DIR")
                        )
                        .as_str(),
                    ),
                ),
                ("GITHUB_HEAD_REF", Some("feat/codspeed-runner")),
                ("GITHUB_JOB", Some("log-env")),
                ("GITHUB_REF", Some("refs/pull/22/merge")),
                ("GITHUB_REPOSITORY", Some("my-org/adrien-python-test")),
                (
                    "GITHUB_WORKSPACE",
                    Some("/home/runner/work/adrien-python-test/adrien-python-test"),
                ),
                ("GITHUB_RUN_ID", Some("6957110437")),
                ("VERSION", Some("0.1.0")),
                ("GH_MATRIX", Some("null")),
            ],
            || {
                let config = OrchestratorConfig {
                    token: Some("token".into()),
                    ..OrchestratorConfig::test()
                };
                let github_actions_provider = GitHubActionsProvider::try_from(&config).unwrap();
                assert!(github_actions_provider.is_head_repo_fork);
                assert!(!github_actions_provider.is_repository_private);

                let run_environment_metadata = github_actions_provider
                    .get_run_environment_metadata()
                    .unwrap();
                let run_part = github_actions_provider.get_run_provider_run_part().unwrap();

                assert_eq!(run_environment_metadata.owner, "my-org");
                assert_eq!(run_environment_metadata.repository, "adrien-python-test");
                assert_eq!(run_environment_metadata.base_ref, Some("main".into()));
                assert_eq!(
                    run_environment_metadata.head_ref,
                    Some("fork-owner:feat/codspeed-runner".into())
                );

                assert_json_snapshot!(run_environment_metadata, {
                    ".runner.version" => insta::dynamic_redaction(|value,_path| {
                        assert_eq!(value.as_str().unwrap(), VERSION.to_string());
                        "[version]"
                    }),
                });
                assert_json_snapshot!(run_part);
            },
        );
    }

    #[test]
    fn test_matrix_job_run_environment_metadata() {
        with_vars(
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
                            "{}/src/run_environment/github_actions/samples/pr-event.json",
                            env!("CARGO_MANIFEST_DIR")
                        )
                        .as_str(),
                    ),
                ),
                ("GITHUB_HEAD_REF", Some("feat/codspeed-runner")),
                ("GITHUB_JOB", Some("log-env")),
                ("GITHUB_REF", Some("refs/pull/22/merge")),
                (
                    "GITHUB_WORKSPACE",
                    Some("/home/runner/work/adrien-python-test/adrien-python-test"),
                ),
                ("GITHUB_REPOSITORY", Some("my-org/adrien-python-test")),
                ("GITHUB_RUN_ID", Some("6957110437")),
                ("VERSION", Some("0.1.0")),
                (
                    "GH_MATRIX",
                    Some(
                        r#"{
    "runner-version":"3.2.1",
    "numeric-value":123456789
}"#,
                    ),
                ),
                (
                    "GH_STRATEGY",
                    Some(
                        r#"{
    "fail-fast":true,
    "job-index":1,
    "job-total":2,
    "max-parallel":2
}"#,
                    ),
                ),
            ],
            || {
                let config = OrchestratorConfig {
                    token: Some("token".into()),
                    ..OrchestratorConfig::test()
                };
                let github_actions_provider = GitHubActionsProvider::try_from(&config).unwrap();
                assert!(!github_actions_provider.is_head_repo_fork);
                assert!(github_actions_provider.is_repository_private);

                let run_environment_metadata = github_actions_provider
                    .get_run_environment_metadata()
                    .unwrap();
                let run_part = github_actions_provider.get_run_provider_run_part().unwrap();

                assert_json_snapshot!(run_environment_metadata, {
                    ".runner.version" => insta::dynamic_redaction(|value,_path| {
                        assert_eq!(value.as_str().unwrap(), VERSION.to_string());
                        "[version]"
                    }),
                });
                assert_json_snapshot!(run_part);
            },
        );
    }

    #[test]
    fn test_get_run_part_no_matrix() {
        with_vars([("GITHUB_ACTIONS", Some("true"))], || {
            let github_actions_provider = GitHubActionsProvider {
                owner: "owner".into(),
                repository: "repository".into(),
                ref_: "refs/head/my-branch".into(),
                head_ref: Some("my-branch".into()),
                base_ref: None,
                sender: None,
                gh_data: GhData {
                    job: "my_job".into(),
                    run_id: "123789".into(),
                },
                event: RunEvent::Push,
                repository_root_path: "/home/work/my-repo".into(),
                is_head_repo_fork: false,
                is_repository_private: false,
                oidc_config: None,
            };

            let run_part = github_actions_provider.get_run_provider_run_part().unwrap();

            assert_eq!(run_part.run_id, "123789");
            assert_eq!(run_part.job_name, "my_job");
            assert_eq!(run_part.run_part_id, "my_job");
            assert_json_snapshot!(run_part.metadata, @"{}");
        })
    }

    #[test]
    fn test_get_run_part_null_matrix() {
        with_vars(
            [
                ("GH_MATRIX", Some("null")),
                (
                    "GH_STRATEGY",
                    Some(
                        r#"{
    "fail-fast":true,
    "job-index":0,
    "job-total":1,
    "max-parallel":1
}"#,
                    ),
                ),
            ],
            || {
                let github_actions_provider = GitHubActionsProvider {
                    owner: "owner".into(),
                    repository: "repository".into(),
                    ref_: "refs/head/my-branch".into(),
                    head_ref: Some("my-branch".into()),
                    base_ref: None,
                    sender: None,
                    gh_data: GhData {
                        job: "my_job".into(),
                        run_id: "123789".into(),
                    },
                    event: RunEvent::Push,
                    repository_root_path: "/home/work/my-repo".into(),
                    is_head_repo_fork: false,
                    is_repository_private: false,
                    oidc_config: None,
                };

                let run_part = github_actions_provider.get_run_provider_run_part().unwrap();

                assert_eq!(run_part.run_id, "123789");
                assert_eq!(run_part.job_name, "my_job");
                assert_eq!(run_part.run_part_id, "my_job");
                assert_json_snapshot!(run_part.metadata, @"{}");
            },
        )
    }

    #[test]
    fn test_get_matrix_run_part() {
        with_vars(
            [
                (
                    "GH_MATRIX",
                    Some(
                        r#"{
    "runner-version":"3.2.1",
    "numeric-value":123456789
}"#,
                    ),
                ),
                (
                    "GH_STRATEGY",
                    Some(
                        r#"{
    "fail-fast":true,
    "job-index":1,
    "job-total":2,
    "max-parallel":2
}"#,
                    ),
                ),
            ],
            || {
                let github_actions_provider = GitHubActionsProvider {
                    owner: "owner".into(),
                    repository: "repository".into(),
                    ref_: "refs/head/my-branch".into(),
                    head_ref: Some("my-branch".into()),
                    base_ref: None,
                    sender: None,
                    gh_data: GhData {
                        job: "my_job".into(),
                        run_id: "123789".into(),
                    },
                    event: RunEvent::Push,
                    repository_root_path: "/home/work/my-repo".into(),
                    is_head_repo_fork: false,
                    is_repository_private: false,
                    oidc_config: None,
                };

                let run_part = github_actions_provider.get_run_provider_run_part().unwrap();

                assert_eq!(run_part.run_id, "123789");
                assert_eq!(run_part.job_name, "my_job");
                assert_eq!(
                    run_part.run_part_id,
                    "my_job-{\"runner-version\":\"3.2.1\",\"numeric-value\":123456789}-{\"job-total\":2,\"job-index\":1}"
                );
                assert_json_snapshot!(run_part.metadata, @r#"
                {
                  "job-index": 1,
                  "job-total": 2,
                  "numeric-value": 123456789,
                  "runner-version": "3.2.1"
                }
                "#);
            },
        )
    }

    #[test]
    fn test_get_inline_matrix_run_part() {
        with_vars(
            [
                (
                    "GH_MATRIX",
                    Some("{\"runner-version\":\"3.2.1\",\"numeric-value\":123456789}"),
                ),
                (
                    "GH_STRATEGY",
                    Some("{\"fail-fast\":true,\"job-index\":1,\"job-total\":2,\"max-parallel\":2}"),
                ),
            ],
            || {
                let github_actions_provider = GitHubActionsProvider {
                    owner: "owner".into(),
                    repository: "repository".into(),
                    ref_: "refs/head/my-branch".into(),
                    head_ref: Some("my-branch".into()),
                    base_ref: None,
                    sender: None,
                    gh_data: GhData {
                        job: "my_job".into(),
                        run_id: "123789".into(),
                    },
                    event: RunEvent::Push,
                    repository_root_path: "/home/work/my-repo".into(),
                    is_head_repo_fork: false,
                    is_repository_private: false,
                    oidc_config: None,
                };

                let run_part = github_actions_provider.get_run_provider_run_part().unwrap();

                assert_eq!(run_part.run_id, "123789");
                assert_eq!(run_part.job_name, "my_job");
                assert_eq!(
                    run_part.run_part_id,
                    "my_job-{\"runner-version\":\"3.2.1\",\"numeric-value\":123456789}-{\"job-total\":2,\"job-index\":1}"
                );
                assert_json_snapshot!(run_part.metadata, @r#"
                {
                  "job-index": 1,
                  "job-total": 2,
                  "numeric-value": 123456789,
                  "runner-version": "3.2.1"
                }
                "#);
            },
        )
    }
}
