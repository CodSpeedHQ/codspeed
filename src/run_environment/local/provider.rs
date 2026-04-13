use async_trait::async_trait;
use git2::Repository;
use simplelog::SharedLogger;
use uuid::Uuid;

use crate::api_client::{
    CodSpeedAPIClient, GetOrCreateProjectRepositoryPayload, GetOrCreateProjectRepositoryVars,
    GetRepositoryPayload, GetRepositoryVars,
};
use crate::cli::run::helpers::{find_repository_root, parse_repository_from_remote};
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
    pub async fn new(
        config: &OrchestratorConfig,
        api_client: &impl LocalProviderApiClient,
    ) -> Result<Self> {
        let current_dir = std::env::current_dir()?;
        let git_context = Self::find_git_context(&current_dir);

        let repository_root_path = git_context
            .as_ref()
            .map(|ctx| ctx.root_path.clone())
            .unwrap_or_else(|| current_dir.to_string_lossy().to_string());

        let resolved = if !config.skip_upload {
            Self::resolve_repository(config, api_client, git_context.as_ref()).await?
        } else {
            Self::dummy_resolved_repository(git_context.as_ref())
        };

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

    /// Create a dummy resolved repository, resolved offline when --skip-upload is used and we don't need to resolve the actual repository information from the API
    fn dummy_resolved_repository(git_context: Option<&GitContext>) -> ResolvedRepository {
        let (ref_, head_ref) = git_context
            .and_then(|ctx| Self::get_git_ref_info(&ctx.root_path).ok())
            .unwrap_or_else(|| (FAKE_COMMIT_REF.to_string(), None));

        ResolvedRepository {
            provider: RepositoryProvider::GitHub,
            owner: "local".to_string(),
            name: "local".to_string(),
            ref_,
            head_ref,
        }
    }

    /// Resolve repository information from override, git remote, or API fallback
    ///
    /// When there is no explicit repository override, this flow also makes sure the user is logged in with a valid token
    /// 1. Repo found
    ///    a. Logged in: user is set, repositoryOverview is set — repo found
    ///    b. NOT logged in: user is null, repositoryOverview is set => bails with "session expired"
    ///
    /// 2. REPOSITORY_NOT_FOUND => falls through to `get_or_create_project_repository`
    ///    a. Logged in: `get_or_create_project_repository` succeeds
    ///    b. NOT logged in: `get_or_create_project_repository` fails, bail with "session expired"
    async fn resolve_repository(
        config: &OrchestratorConfig,
        api_client: &impl LocalProviderApiClient,
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
        api_client: &impl LocalProviderApiClient,
        git_context: &GitContext,
    ) -> Result<Option<ResolvedRepository>> {
        let git_repository = Repository::open(&git_context.root_path).context(format!(
            "Failed to open repository at path: {}",
            git_context.root_path
        ))?;

        let remote = git_repository.find_remote("origin")?;
        let parsed = parse_repository_from_remote(remote.url().unwrap())?;
        let (provider, owner, name) = (parsed.provider, parsed.owner, parsed.name);

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
        api_client: &impl LocalProviderApiClient,
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

    fn get_commit_hash(&self, _repository_root_path: &str) -> Result<String> {
        Ok(self.ref_.clone())
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

#[async_trait(?Send)]
pub trait LocalProviderApiClient {
    async fn get_repository(&self, vars: GetRepositoryVars)
    -> Result<Option<GetRepositoryPayload>>;

    async fn get_or_create_project_repository(
        &self,
        vars: GetOrCreateProjectRepositoryVars,
    ) -> Result<GetOrCreateProjectRepositoryPayload>;
}

#[async_trait(?Send)]
impl LocalProviderApiClient for CodSpeedAPIClient {
    async fn get_repository(
        &self,
        vars: GetRepositoryVars,
    ) -> Result<Option<GetRepositoryPayload>> {
        self.get_repository(vars).await
    }

    async fn get_or_create_project_repository(
        &self,
        vars: GetOrCreateProjectRepositoryVars,
    ) -> Result<GetOrCreateProjectRepositoryPayload> {
        self.get_or_create_project_repository(vars).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_commit_hash_ref() {
        assert_eq!(FAKE_COMMIT_REF.len(), 40);
    }

    fn create_git_repo_with_remote(dir: &std::path::Path, remote_url: &str) -> String {
        std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(dir)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["remote", "add", "origin", remote_url])
            .current_dir(dir)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(dir)
            .output()
            .unwrap();

        format!("{}/", dir.to_string_lossy())
    }

    /// A mock API client that returns a found repository for `get_repository`
    /// and a fixed project repository for `get_or_create_project_repository`.
    struct MockApiClientRepoFound;

    impl MockApiClientRepoFound {
        fn new() -> Self {
            Self
        }
    }

    #[async_trait(?Send)]
    impl LocalProviderApiClient for MockApiClientRepoFound {
        async fn get_repository(
            &self,
            _vars: GetRepositoryVars,
        ) -> Result<Option<GetRepositoryPayload>> {
            Ok(Some(GetRepositoryPayload {
                id: "my-repo-id".into(),
            }))
        }

        async fn get_or_create_project_repository(
            &self,
            _vars: GetOrCreateProjectRepositoryVars,
        ) -> Result<GetOrCreateProjectRepositoryPayload> {
            unreachable!("should not be called when repo is found")
        }
    }

    /// A mock API client that returns no repository for `get_repository`,
    /// falling back to a fixed project repository.
    struct MockApiClientRepoNotFound;

    impl MockApiClientRepoNotFound {
        fn new() -> Self {
            Self
        }
    }

    #[async_trait(?Send)]
    impl LocalProviderApiClient for MockApiClientRepoNotFound {
        async fn get_repository(
            &self,
            _vars: GetRepositoryVars,
        ) -> Result<Option<GetRepositoryPayload>> {
            Ok(None)
        }

        async fn get_or_create_project_repository(
            &self,
            _vars: GetOrCreateProjectRepositoryVars,
        ) -> Result<GetOrCreateProjectRepositoryPayload> {
            Ok(GetOrCreateProjectRepositoryPayload {
                provider: RepositoryProvider::GitHub,
                owner: "CodSpeedHQ".into(),
                name: "local-runs".into(),
            })
        }
    }

    async fn build_provider_for_test(
        config: &OrchestratorConfig,
        api_client: &impl LocalProviderApiClient,
        root_path: &str,
    ) -> LocalProvider {
        let git_context = Some(GitContext {
            root_path: root_path.to_string(),
        });

        let resolved = LocalProvider::resolve_repository(config, api_client, git_context.as_ref())
            .await
            .unwrap();

        LocalProvider {
            repository_provider: resolved.provider,
            owner: resolved.owner,
            repository: resolved.name,
            ref_: resolved.ref_,
            head_ref: resolved.head_ref,
            repository_root_path: root_path.to_string(),
            event: RunEvent::Local,
            run_id: "test-run-id".to_string(),
            expected_run_parts_count: config.expected_run_parts_count(),
        }
    }

    #[tokio::test]
    async fn test_new_with_github_remote_found_on_codspeed() {
        let dir = tempfile::tempdir().unwrap();
        let root_path =
            create_git_repo_with_remote(dir.path(), "git@github.com:my-repo/my-owner.git");

        let config = OrchestratorConfig::test();
        let provider =
            build_provider_for_test(&config, &MockApiClientRepoFound::new(), &root_path).await;

        let run_environment_metadata = provider.get_run_environment_metadata().unwrap();
        let run_part = provider.get_run_provider_run_part().unwrap();

        insta::assert_json_snapshot!(run_environment_metadata, {
            ".ref" => "[commit_hash]",
            ".repositoryRootPath" => "[root_path]",
        });
        insta::assert_json_snapshot!(run_part);
    }

    #[tokio::test]
    async fn test_new_falls_back_to_project_repository() {
        let dir = tempfile::tempdir().unwrap();
        let root_path = create_git_repo_with_remote(dir.path(), "git@github.com:foobar/baz.git");

        let config = OrchestratorConfig::test();
        let provider =
            build_provider_for_test(&config, &MockApiClientRepoNotFound::new(), &root_path).await;

        let run_environment_metadata = provider.get_run_environment_metadata().unwrap();
        let run_part = provider.get_run_provider_run_part().unwrap();

        insta::assert_json_snapshot!(run_environment_metadata, {
            ".repositoryRootPath" => "[root_path]",
        });
        insta::assert_json_snapshot!(run_part);
    }

    #[tokio::test]
    async fn test_new_without_git_repository() {
        let dir = tempfile::tempdir().unwrap();
        let root_path = format!("{}/", dir.path().to_string_lossy());

        let config = OrchestratorConfig::test();
        let git_context: Option<GitContext> = None;
        let resolved = LocalProvider::resolve_repository(
            &config,
            &MockApiClientRepoNotFound::new(),
            git_context.as_ref(),
        )
        .await
        .unwrap();
        let provider = LocalProvider {
            repository_provider: resolved.provider,
            owner: resolved.owner,
            repository: resolved.name,
            ref_: resolved.ref_,
            head_ref: resolved.head_ref,
            repository_root_path: root_path.clone(),
            event: RunEvent::Local,
            run_id: "test-run-id".to_string(),
            expected_run_parts_count: config.expected_run_parts_count(),
        };

        let run_environment_metadata = provider.get_run_environment_metadata().unwrap();
        let run_part = provider.get_run_provider_run_part().unwrap();

        insta::assert_json_snapshot!(run_environment_metadata, {
            ".repositoryRootPath" => "[root_path]",
        });
        insta::assert_json_snapshot!(run_part);
    }
}
