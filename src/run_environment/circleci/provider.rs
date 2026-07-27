use std::collections::BTreeMap;
use std::env;

use async_trait::async_trait;
use serde_json::Value;
use simplelog::SharedLogger;

use crate::api_client::CodSpeedAPIClient;
use crate::cli::run::helpers::{
    GitRemote, find_repository_root, get_env_variable, parse_git_remote,
};
use crate::executor::config::OrchestratorConfig;
use crate::prelude::*;
use crate::run_environment::interfaces::{RepositoryProvider, RunEnvironmentMetadata, RunEvent};
use crate::run_environment::provider::{RunEnvironmentDetector, RunEnvironmentProvider};
use crate::run_environment::{RunEnvironment, RunPart};

use super::logger::CircleCILogger;

#[derive(Debug)]
pub struct CircleCIProvider {
    owner: String,
    repository: String,
    ref_: String,
    head_ref: Option<String>,
    event: RunEvent,
    repository_root_path: String,
    workflow_id: String,
    job_name: String,
    /// Index of this container within a job running with `parallelism`, `0` when unset.
    node_index: u32,
    /// Number of containers the job runs on, `1` when unset.
    node_total: u32,
}

/// Returns the number of the pull request the build runs on, if any.
///
/// `CIRCLE_PULL_REQUEST` holds the URL of the pull request (`.../pull/22`) and is
/// only set for pull request builds. `CIRCLE_PR_NUMBER` is a fallback that is only
/// populated for pull requests opened from a fork.
fn get_pr_number() -> Option<u64> {
    let from_url = env::var("CIRCLE_PULL_REQUEST")
        .ok()
        .and_then(|url| url.rsplit('/').next()?.parse().ok());

    from_url.or_else(|| {
        env::var("CIRCLE_PR_NUMBER")
            .ok()
            .and_then(|number| number.parse().ok())
    })
}

fn get_ref(pr_number: Option<u64>) -> Result<String> {
    match pr_number {
        Some(pr_number) => Ok(format!("refs/pull/{pr_number}/merge")),
        None => Ok(format!("refs/heads/{}", get_env_variable("CIRCLE_BRANCH")?)),
    }
}

fn get_env_number(name: &str, default: u32) -> u32 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

impl TryFrom<&OrchestratorConfig> for CircleCIProvider {
    type Error = Error;
    fn try_from(config: &OrchestratorConfig) -> Result<Self> {
        if config.repository_override.is_some() {
            bail!("Specifying owner and repository from CLI is not supported for CircleCI");
        }

        let pr_number = get_pr_number();
        let repository_url = get_env_variable("CIRCLE_REPOSITORY_URL")?;
        let GitRemote {
            owner,
            repository,
            domain,
        } = parse_git_remote(&repository_url)?;

        // CircleCI also builds Bitbucket and GitLab repositories, which CodSpeed does not
        // support here. The domain of the remote is the only signal available at runtime:
        // the explicit `pipeline.project.type` is a pipeline value, interpolated when the
        // config is compiled, so it never reaches the job as an environment variable.
        // https://circleci.com/docs/reference/variables/
        if domain != "github.com" {
            bail!(
                "CodSpeed only supports GitHub repositories on CircleCI, but \
                CIRCLE_REPOSITORY_URL points to {domain} ({repository_url})"
            );
        }

        let repository_root_path = match find_repository_root(&std::env::current_dir()?) {
            Some(mut path) => {
                // Add a trailing slash to the path
                path.push("");
                path.to_string_lossy().to_string()
            }
            None => {
                // Fallback to the working directory of the job, where CircleCI checks the
                // repository out. Its value mirrors the `working_directory` key of the job,
                // so it can start with a `~`.
                // https://circleci.com/docs/variables/#built-in-environment-variables
                let working_directory = get_env_variable("CIRCLE_WORKING_DIRECTORY")?;
                format!("{}/", shellexpand::tilde(&working_directory))
            }
        };

        Ok(Self {
            owner,
            repository,
            ref_: get_ref(pr_number)?,
            head_ref: if pr_number.is_some() {
                Some(get_env_variable("CIRCLE_BRANCH")?)
            } else {
                None
            },
            event: if pr_number.is_some() {
                RunEvent::PullRequest
            } else {
                RunEvent::Push
            },
            repository_root_path,
            workflow_id: get_env_variable("CIRCLE_WORKFLOW_ID")?,
            job_name: get_env_variable("CIRCLE_JOB")?,
            node_index: get_env_number("CIRCLE_NODE_INDEX", 0),
            node_total: get_env_number("CIRCLE_NODE_TOTAL", 1),
        })
    }
}

impl RunEnvironmentDetector for CircleCIProvider {
    fn detect() -> bool {
        env::var("CIRCLECI") == Ok("true".into())
    }
}

#[async_trait(?Send)]
impl RunEnvironmentProvider for CircleCIProvider {
    fn get_repository_provider(&self) -> RepositoryProvider {
        RepositoryProvider::GitHub
    }

    fn get_logger(&self) -> Box<dyn SharedLogger> {
        Box::new(CircleCILogger::new())
    }

    fn get_run_environment(&self) -> RunEnvironment {
        RunEnvironment::Circleci
    }

    fn get_run_environment_metadata(&self) -> Result<RunEnvironmentMetadata> {
        Ok(RunEnvironmentMetadata {
            // CircleCI exposes no base branch variable. CodSpeed resolves it from the pull request.
            base_ref: None,
            head_ref: self.head_ref.clone(),
            event: self.event.clone(),
            owner: self.owner.clone(),
            repository: self.repository.clone(),
            ref_: self.ref_.clone(),
            repository_root_path: self.repository_root_path.clone(),
            gh_data: None,
            gl_data: None,
            local_data: None,
            sender: None,
        })
    }

    /// `CIRCLE_WORKFLOW_ID` is shared by every job and every parallel container of a
    /// workflow, which makes it the key that groups run parts together. The per-job
    /// `CIRCLE_WORKFLOW_JOB_ID` would instead split one workflow into unrelated runs.
    ///
    /// A workflow fans out along two axes, so the part id has to cover both: its jobs
    /// (`CIRCLE_JOB`) and, within a job declaring `parallelism`, its containers
    /// (`CIRCLE_NODE_INDEX`).
    fn get_run_provider_run_part(&self) -> Option<RunPart> {
        Some(RunPart {
            run_id: self.workflow_id.clone(),
            run_part_id: format!("{}-{}", self.job_name, self.node_index),
            job_name: self.job_name.clone(),
            metadata: BTreeMap::from([
                ("node-index".to_string(), Value::from(self.node_index)),
                ("node-total".to_string(), Value::from(self.node_total)),
            ]),
        })
    }

    /// CircleCI requires a static `CODSPEED_TOKEN`. We don't yet support OIDC
    /// tokens here (could be added via `CIRCLE_OIDC_TOKEN_V2`:
    /// <https://circleci.com/docs/openid-connect-tokens>), so this just enforces
    /// token presence.
    fn check_oidc_configuration(&mut self, api_client: &CodSpeedAPIClient) -> Result<()> {
        if api_client.token().is_none() {
            bail!("Token authentication is required for CircleCI");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_json_snapshot;
    use temp_env::{with_var, with_vars};

    use super::*;

    #[test]
    fn test_detect() {
        with_var("CIRCLECI", Some("true"), || {
            assert!(CircleCIProvider::detect());
        });
    }

    #[test]
    fn test_try_from_push_main() {
        with_vars(
            [
                ("CIRCLECI", Some("true")),
                ("CIRCLE_BRANCH", Some("main")),
                (
                    "CIRCLE_REPOSITORY_URL",
                    Some("git@github.com:my-org/adrien-python-test.git"),
                ),
                ("CIRCLE_WORKING_DIRECTORY", Some("/home/circleci/project")),
                (
                    "CIRCLE_WORKFLOW_ID",
                    Some("8d8f0b2a-1f3e-4b6a-9c2d-0f1e2a3b4c5d"),
                ),
                ("CIRCLE_JOB", Some("benchmarks")),
            ],
            || {
                let config = OrchestratorConfig {
                    ..OrchestratorConfig::test()
                };
                let provider = CircleCIProvider::try_from(&config).unwrap();

                assert_eq!(provider.owner, "my-org");
                assert_eq!(provider.repository, "adrien-python-test");
                assert_eq!(provider.ref_, "refs/heads/main");
                assert_eq!(provider.head_ref, None);
                assert_eq!(provider.event, RunEvent::Push);
                assert_eq!(provider.repository_root_path, "/home/circleci/project/");
            },
        );
    }

    #[test]
    fn test_try_from_pull_request() {
        with_vars(
            [
                ("CIRCLECI", Some("true")),
                ("CIRCLE_BRANCH", Some("feat/codspeed-runner")),
                (
                    "CIRCLE_PULL_REQUEST",
                    Some("https://github.com/my-org/adrien-python-test/pull/22"),
                ),
                (
                    "CIRCLE_REPOSITORY_URL",
                    Some("https://github.com/my-org/adrien-python-test"),
                ),
                ("CIRCLE_WORKING_DIRECTORY", Some("/home/circleci/project")),
                (
                    "CIRCLE_WORKFLOW_ID",
                    Some("8d8f0b2a-1f3e-4b6a-9c2d-0f1e2a3b4c5d"),
                ),
                ("CIRCLE_JOB", Some("benchmarks")),
            ],
            || {
                let config = OrchestratorConfig {
                    ..OrchestratorConfig::test()
                };
                let provider = CircleCIProvider::try_from(&config).unwrap();

                assert_eq!(provider.owner, "my-org");
                assert_eq!(provider.repository, "adrien-python-test");
                assert_eq!(provider.ref_, "refs/pull/22/merge");
                assert_eq!(provider.head_ref, Some("feat/codspeed-runner".into()));
                assert_eq!(provider.event, RunEvent::PullRequest);
            },
        );
    }

    /// On pull requests opened from a fork, `CIRCLE_PULL_REQUEST` is not set and
    /// `CIRCLE_BRANCH` is the `pull/{number}` ref CircleCI checks out.
    #[test]
    fn test_try_from_fork_pull_request() {
        with_vars(
            [
                ("CIRCLECI", Some("true")),
                ("CIRCLE_BRANCH", Some("pull/22")),
                ("CIRCLE_PR_NUMBER", Some("22")),
                (
                    "CIRCLE_REPOSITORY_URL",
                    Some("git@github.com:my-org/adrien-python-test.git"),
                ),
                ("CIRCLE_WORKING_DIRECTORY", Some("/home/circleci/project")),
                (
                    "CIRCLE_WORKFLOW_ID",
                    Some("8d8f0b2a-1f3e-4b6a-9c2d-0f1e2a3b4c5d"),
                ),
                ("CIRCLE_JOB", Some("benchmarks")),
            ],
            || {
                let config = OrchestratorConfig {
                    ..OrchestratorConfig::test()
                };
                let provider = CircleCIProvider::try_from(&config).unwrap();

                assert_eq!(provider.ref_, "refs/pull/22/merge");
                assert_eq!(provider.event, RunEvent::PullRequest);
            },
        );
    }

    #[test]
    fn test_try_from_non_github_repository() {
        with_vars(
            [
                ("CIRCLECI", Some("true")),
                ("CIRCLE_BRANCH", Some("main")),
                (
                    "CIRCLE_REPOSITORY_URL",
                    Some("git@bitbucket.org:my-org/adrien-python-test.git"),
                ),
                ("CIRCLE_WORKING_DIRECTORY", Some("/home/circleci/project")),
                (
                    "CIRCLE_WORKFLOW_ID",
                    Some("8d8f0b2a-1f3e-4b6a-9c2d-0f1e2a3b4c5d"),
                ),
                ("CIRCLE_JOB", Some("benchmarks")),
            ],
            || {
                let config = OrchestratorConfig {
                    ..OrchestratorConfig::test()
                };
                let error = CircleCIProvider::try_from(&config).unwrap_err();

                assert_eq!(
                    error.to_string(),
                    "CodSpeed only supports GitHub repositories on CircleCI, but \
                    CIRCLE_REPOSITORY_URL points to bitbucket.org \
                    (git@bitbucket.org:my-org/adrien-python-test.git)"
                );
            },
        );
    }

    #[test]
    fn test_pull_request_run_environment_metadata() {
        with_vars(
            [
                ("CIRCLECI", Some("true")),
                ("CIRCLE_BRANCH", Some("feat/codspeed-runner")),
                (
                    "CIRCLE_PULL_REQUEST",
                    Some("https://github.com/my-org/adrien-python-test/pull/22"),
                ),
                (
                    "CIRCLE_REPOSITORY_URL",
                    Some("git@github.com:my-org/adrien-python-test.git"),
                ),
                ("CIRCLE_WORKING_DIRECTORY", Some("/home/circleci/project")),
                (
                    "CIRCLE_WORKFLOW_ID",
                    Some("8d8f0b2a-1f3e-4b6a-9c2d-0f1e2a3b4c5d"),
                ),
                ("CIRCLE_JOB", Some("benchmarks")),
            ],
            || {
                let config = OrchestratorConfig {
                    ..OrchestratorConfig::test()
                };
                let provider = CircleCIProvider::try_from(&config).unwrap();
                let run_environment_metadata = provider.get_run_environment_metadata().unwrap();
                let run_part = provider.get_run_provider_run_part().unwrap();

                assert_json_snapshot!(run_environment_metadata);
                assert_json_snapshot!(run_part);
            },
        );
    }

    /// A job declaring `parallelism` runs on several containers that share
    /// `CIRCLE_JOB`, so the node index is what keeps their part ids apart.
    #[test]
    fn test_run_part_of_parallel_job() {
        with_vars(
            [
                ("CIRCLECI", Some("true")),
                ("CIRCLE_BRANCH", Some("main")),
                (
                    "CIRCLE_REPOSITORY_URL",
                    Some("git@github.com:my-org/adrien-python-test.git"),
                ),
                ("CIRCLE_WORKING_DIRECTORY", Some("/home/circleci/project")),
                (
                    "CIRCLE_WORKFLOW_ID",
                    Some("8d8f0b2a-1f3e-4b6a-9c2d-0f1e2a3b4c5d"),
                ),
                ("CIRCLE_JOB", Some("benchmarks")),
                ("CIRCLE_NODE_INDEX", Some("2")),
                ("CIRCLE_NODE_TOTAL", Some("4")),
            ],
            || {
                let config = OrchestratorConfig {
                    ..OrchestratorConfig::test()
                };
                let provider = CircleCIProvider::try_from(&config).unwrap();
                let run_part = provider.get_run_provider_run_part().unwrap();

                assert_eq!(run_part.run_id, "8d8f0b2a-1f3e-4b6a-9c2d-0f1e2a3b4c5d");
                assert_eq!(run_part.job_name, "benchmarks");
                assert_eq!(run_part.run_part_id, "benchmarks-2");
                assert_json_snapshot!(run_part.metadata, @r#"
                {
                  "node-index": 2,
                  "node-total": 4
                }
                "#);
            },
        );
    }
}
