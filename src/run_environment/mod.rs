pub mod interfaces;
pub mod logger;
mod provider;

use buildkite::BuildkiteProvider;
use circleci::CircleCIProvider;
use github_actions::GitHubActionsProvider;
use gitlab_ci::GitLabCIProvider;
use local::LocalProvider;
use provider::RunEnvironmentDetector;

use crate::api_client::CodSpeedAPIClient;
use crate::executor::config::OrchestratorConfig;
use crate::prelude::*;

pub use self::interfaces::*;
pub use self::provider::RunEnvironmentProvider;

// RunEnvironment Provider implementations
mod buildkite;
mod circleci;
mod github_actions;
mod gitlab_ci;
mod local;

/// Detect the run environment from the process environment, without any configuration.
pub fn detect() -> RunEnvironment {
    if BuildkiteProvider::detect() {
        RunEnvironment::Buildkite
    } else if CircleCIProvider::detect() {
        RunEnvironment::Circleci
    } else if GitHubActionsProvider::detect() {
        RunEnvironment::GithubActions
    } else if GitLabCIProvider::detect() {
        RunEnvironment::GitlabCi
    } else {
        RunEnvironment::Local
    }
}

pub async fn get_provider(
    config: &OrchestratorConfig,
    api_client: &CodSpeedAPIClient,
) -> Result<Box<dyn RunEnvironmentProvider>> {
    let mut provider: Box<dyn RunEnvironmentProvider> = match detect() {
        RunEnvironment::Buildkite => Box::new(BuildkiteProvider::try_from(config)?),
        RunEnvironment::Circleci => Box::new(CircleCIProvider::try_from(config)?),
        RunEnvironment::GithubActions => Box::new(GitHubActionsProvider::try_from(config)?),
        RunEnvironment::GitlabCi => Box::new(GitLabCIProvider::try_from(config)?),
        RunEnvironment::Local => Box::new(LocalProvider::new(config, api_client).await?),
    };

    provider.check_oidc_configuration(api_client)?;

    Ok(provider)
}
