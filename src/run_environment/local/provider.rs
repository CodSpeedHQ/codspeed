use async_trait::async_trait;
use git2::Repository;
use simplelog::SharedLogger;
use uuid::Uuid;

use crate::api_client::{CodSpeedAPIClient, GetOrCreateProjectRepositoryVars, GetRepositoryVars};
use crate::cli::run::helpers::{GitRemote, find_repository_root, parse_git_remote};
use crate::executor::config::OrchestratorConfig;
use crate::executor::config::RepositoryOverride;
use crate::local_logger::get_local_logger;
use crate::prelude::*;
use crate::run_environment::interfaces::{
    LocalData, RepositoryProvider, RunEnvironmentMetadata, RunEvent,
};
use crate::run_environment::provider::{RunEnvironmentDetector, RunEnvironmentProvider};
use crate::run_environment::{RunEnvironment, RunPart};
use serde_json::Value;
use std::collections::BTreeMap;

static FAKE_COMMIT_REF: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[derive(Debug)]
pub struct LocalProvider {
    repository_provider: RepositoryProvider,
    owner: String,
    repository: String,
    ref_: String,
    head_ref: Option<String>,
    pub event: RunEvent,
    pub repository_root_path: String,
    run_id: String,
    expected_run_parts_count: u32,
}

/// Information about the git repository root path
struct GitContext {
    /// Path to the repository root (with trailing slash)
    root_path: String,
}

/// Repository information resolved from git or API
struct ResolvedRepository {
    provider: RepositoryProvider,
    owner: String,
    name: String,
    ref_: String,
    head_ref: Option<String>,
}

impl LocalProvider {
    pub async fn new(config: &OrchestratorConfig, api_client: &CodSpeedAPIClient) -> Result<Self> {
        let current_dir = std::env::current_dir()?;
        let git_context = Self::find_git_context(&current_dir);

        let repository_root_path = git_context
            .as_ref()
            .map(|ctx| ctx.root_path.clone())
            .unwrap_or_else(|| current_dir.to_string_lossy().to_string());

        let resolved = Self::resolve_repository(config, api_client, git_context.as_ref()).await?;

        let expected_run_parts_count = config.expected_run_parts_count();

        Ok(Self {
            repository_provider: resolved.provider,
            owner: resolved.owner,
            repository: resolved.name,
            ref_: resolved.ref_,
            head_ref: resolved.head_ref,
            repository_root_path,
            event: RunEvent::Local,
            run_id: Uuid::new_v4().to_string(),
            expected_run_parts_count,
        })
    }

    /// Find the git repository context if we're inside a git repo
    fn find_git_context(current_dir: &std::path::Path) -> Option<GitContext> {
        find_repository_root(current_dir).map(|mut path| {
            path.push(""); // Add trailing slash
            GitContext {
                root_path: path.to_string_lossy().to_string(),
            }
        })
    }

    /// Resolve repository information from override, git remote, or API fallback
    async fn resolve_repository(
        config: &OrchestratorConfig,
        api_client: &CodSpeedAPIClient,
        git_context: Option<&GitContext>,
    ) -> Result<ResolvedRepository> {
        // Priority 1: Use explicit repository override
        if let Some(repo_override) = &config.repository_override {
            return Self::resolve_from_override(repo_override, git_context);
        }

        // Priority 2: Try to use git remote if repository exists in CodSpeed
        if let Some(ctx) = git_context {
            if let Some(resolved) =
                Self::try_resolve_from_codspeed_repository(api_client, ctx).await?
            {
                return Ok(resolved);
            }
        }

        // Priority 3: Fallback to project repository
        Self::resolve_as_project_repository(api_client).await
    }

    /// Resolve repository from explicit override configuration
    fn resolve_from_override(
        repo_override: &RepositoryOverride,
        git_context: Option<&GitContext>,
    ) -> Result<ResolvedRepository> {
        let (ref_, head_ref) = git_context
            .map(|ctx| Self::get_git_ref_info(&ctx.root_path))
            .transpose()?
            .unwrap_or_else(|| (FAKE_COMMIT_REF.to_string(), None));

        Ok(ResolvedRepository {
            provider: repo_override.repository_provider.clone(),
            owner: repo_override.owner.clone(),
            name: repo_override.repository.clone(),
            ref_,
            head_ref,
        })
    }

    /// Try to resolve repository from git remote, validating it exists in CodSpeed
    async fn try_resolve_from_codspeed_repository(
        api_client: &CodSpeedAPIClient,
        git_context: &GitContext,
    ) -> Result<Option<ResolvedRepository>> {
        let git_repository = Repository::open(&git_context.root_path).context(format!(
            "Failed to open repository at path: {}",
            git_context.root_path
        ))?;

        let remote = git_repository.find_remote("origin")?;
        let (provider, owner, name) =
            extract_provider_owner_and_repository_from_remote_url(remote.url().unwrap())?;

        // Check if repository exists in CodSpeed
        // Note: we only check existence here, we don't check that
        // - the provider is properly setup
        // - the provider has access to the repository
        //
        // If the repo exists, but these two conditions are not satisfied, the upload will fail
        // later on, but by checking repository existence here we catch most of the cases where the
        // user would run their benchmarks, but fail to upload afterwards.
        let exists = api_client
            .get_repository(GetRepositoryVars {
                owner: owner.clone(),
                name: name.clone(),
                provider: provider.clone(),
            })
            .await?
            .is_some();

        if !exists {
            return Ok(None);
        }

        let (ref_, head_ref) = Self::get_git_ref_info(&git_context.root_path)?;

        Ok(Some(ResolvedRepository {
            provider,
            owner,
            name,
            ref_,
            head_ref,
        }))
    }

    /// Resolve repository by creating/getting a project repository
    async fn resolve_as_project_repository(
        api_client: &CodSpeedAPIClient,
    ) -> Result<ResolvedRepository> {
        let project_name = crate::cli::exec::DEFAULT_REPOSITORY_NAME;

        let repo_info = api_client
            .get_or_create_project_repository(GetOrCreateProjectRepositoryVars {
                name: project_name.to_string(),
            })
            .await?;

        Ok(ResolvedRepository {
            provider: repo_info.provider,
            owner: repo_info.owner,
            name: repo_info.name,
            ref_: FAKE_COMMIT_REF.to_string(),
            head_ref: None,
        })
    }

    /// Extract commit hash and branch name from a git repository
    fn get_git_ref_info(repo_path: &str) -> Result<(String, Option<String>)> {
        let git_repository = Repository::open(repo_path)
            .context(format!("Failed to open repository at path: {repo_path}"))?;

        let head = git_repository.head().context("Failed to get HEAD")?;
        let ref_ = head
            .peel_to_commit()
            .context("Failed to get HEAD commit")?
            .id()
            .to_string();

        let head_ref = if head.is_branch() {
            head.shorthand()
                .context("Failed to get HEAD branch name")
                .map(|s| s.to_string())
                .ok()
        } else {
            None
        };

        Ok((ref_, head_ref))
    }
}

impl RunEnvironmentDetector for LocalProvider {
    fn detect() -> bool {
        true
    }
}

#[async_trait(?Send)]
impl RunEnvironmentProvider for LocalProvider {
    fn get_repository_provider(&self) -> RepositoryProvider {
        self.repository_provider.clone()
    }

    fn get_logger(&self) -> Box<dyn SharedLogger> {
        get_local_logger()
    }

    fn get_run_environment(&self) -> RunEnvironment {
        RunEnvironment::Local
    }

    fn get_run_environment_metadata(&self) -> Result<RunEnvironmentMetadata> {
        Ok(RunEnvironmentMetadata {
            base_ref: None,
            head_ref: self.head_ref.clone(),
            event: self.event.clone(),
            gh_data: None,
            gl_data: None,
            local_data: Some(LocalData {
                expected_run_parts_count: self.expected_run_parts_count,
            }),
            sender: None,
            owner: self.owner.clone(),
            repository: self.repository.clone(),
            ref_: self.ref_.clone(),
            repository_root_path: self.repository_root_path.clone(),
        })
    }

    fn get_run_provider_run_part(&self) -> Option<RunPart> {
        Some(RunPart {
            run_id: self.run_id.clone(),
            run_part_id: "local-job".into(),
            job_name: "local-job".into(),
            metadata: Default::default(),
        })
    }

    /// Local runs don't need run-index because each invocation gets a fresh `run_id`.
    fn build_run_part_suffix(
        &self,
        _run_part: &RunPart,
        _repository_root_path: &str,
        orchestrator_suffix: BTreeMap<String, Value>,
    ) -> BTreeMap<String, Value> {
        orchestrator_suffix
    }
}

fn extract_provider_owner_and_repository_from_remote_url(
    remote_url: &str,
) -> Result<(RepositoryProvider, String, String)> {
    let GitRemote {
        domain,
        owner,
        repository,
    } = parse_git_remote(remote_url)?;
    let repository_provider = match domain.as_str() {
        "github.com" => RepositoryProvider::GitHub,
        "gitlab.com" => RepositoryProvider::GitLab,
        domain => bail!("Repository provider {domain} is not supported by CodSpeed"),
    };

    Ok((
        repository_provider,
        owner.to_string(),
        repository.to_string(),
    ))
}

#[cfg(test)]
mod tests {
    // use crate::VERSION;
    // use insta::assert_json_snapshot;

    use super::*;

    #[test]
    fn test_extract_provider_owner_and_repository_from_remote_url() {
        let remote_urls = [
            (
                "git@github.com:CodSpeedHQ/codspeed.git",
                RepositoryProvider::GitHub,
                "CodSpeedHQ",
                "codspeed",
            ),
            (
                "https://github.com/CodSpeedHQ/codspeed.git",
                RepositoryProvider::GitHub,
                "CodSpeedHQ",
                "codspeed",
            ),
            (
                "git@gitlab.com:codspeed/runner.git",
                RepositoryProvider::GitLab,
                "codspeed",
                "runner",
            ),
            (
                "https://gitlab.com/codspeed/runner.git",
                RepositoryProvider::GitLab,
                "codspeed",
                "runner",
            ),
        ];
        for (remote_url, expected_provider, expected_owner, expected_repository) in
            remote_urls.into_iter()
        {
            let (repository_provider, owner, repository) =
                extract_provider_owner_and_repository_from_remote_url(remote_url).unwrap();
            assert_eq!(repository_provider, expected_provider);
            assert_eq!(owner, expected_owner);
            assert_eq!(repository, expected_repository);
        }
    }

    #[test]
    fn fake_commit_hash_ref() {
        assert_eq!(FAKE_COMMIT_REF.len(), 40);
    }

    // TODO: uncomment later when we have a way to mock git repository
    // #[test]
    // fn test_provider_metadata() {
    //     let config = OrchestratorConfig {
    //         token: Some("token".into()),
    //         ..OrchestratorConfig::test()
    //     };
    //     let local_provider = LocalProvider::try_from(&config).unwrap();
    //     let provider_metadata = local_provider.get_provider_metadata().unwrap();

    //     assert_json_snapshot!(provider_metadata, {
    //         ".runner.version" => insta::dynamic_redaction(|value,_path| {
    //             assert_eq!(value.as_str().unwrap(), VERSION.to_string());
    //             "[version]"
    //         }),
    //     });
    // }
}
