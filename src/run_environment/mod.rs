pub mod interfaces;
pub mod logger;
mod provider;

use buildkite::BuildkiteProvider;
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
mod github_actions;
mod gitlab_ci;
mod local;

pub async fn get_provider(
    config: &OrchestratorConfig,
    api_client: &CodSpeedAPIClient,
) -> Result<Box<dyn RunEnvironmentProvider>> {
    let mut provider: Box<dyn RunEnvironmentProvider> = {
        if BuildkiteProvider::detect() {
            let provider = BuildkiteProvider::try_from(config)?;
            Box::new(provider)
        } else if GitHubActionsProvider::detect() {
            let provider = GitHubActionsProvider::try_from(config)?;
            Box::new(provider)
        } else if GitLabCIProvider::detect() {
            let provider = GitLabCIProvider::try_from(config)?;
            Box::new(provider)
        } else if LocalProvider::detect() {
            let provider = LocalProvider::new(config, api_client).await?;
            Box::new(provider)
        } else {
            // By design, this should not happen as the `LocalProvider` is a fallback
            bail!("No RunEnvironment provider detected")
        }
    };

    provider.check_oidc_configuration(config)?;

    Ok(provider)
}
