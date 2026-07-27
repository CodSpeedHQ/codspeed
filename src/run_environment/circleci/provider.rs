use std::env;

use async_trait::async_trait;
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

        if domain != "github.com" {
            bail!(
                "Only GitHub repositories are supported by CodSpeed CircleCI integration for now."
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

    /// For CircleCI, we don't support multipart uploads
    fn get_run_provider_run_part(&self) -> Option<RunPart> {
        None
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
            ],
            || {
                let config = OrchestratorConfig {
                    ..OrchestratorConfig::test()
                };
                assert!(CircleCIProvider::try_from(&config).is_err());
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
            ],
            || {
                let config = OrchestratorConfig {
                    ..OrchestratorConfig::test()
                };
                let provider = CircleCIProvider::try_from(&config).unwrap();
                let run_environment_metadata = provider.get_run_environment_metadata().unwrap();

                assert_json_snapshot!(run_environment_metadata);
            },
        );
    }
}
