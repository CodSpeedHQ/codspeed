use super::ExecAndRunSharedArgs;
use crate::api_client::CodSpeedAPIClient;
use crate::config::CodSpeedConfig;
use crate::executor;
use crate::executor::config::{self, OrchestratorConfig, RepositoryOverride};
use crate::instruments::Instruments;
use crate::prelude::*;
use crate::project_config::ProjectConfig;
use crate::project_config::merger::ConfigMerger;
use crate::upload::UploadResult;
use crate::upload::poll_results::{PollResultsOptions, poll_results};
use clap::Args;
use std::path::Path;
use url::Url;

pub mod multi_targets;

/// We temporarily force this name for all exec runs
pub const DEFAULT_REPOSITORY_NAME: &str = "local-runs";

#[derive(Args, Debug)]
pub struct ExecArgs {
    #[command(flatten)]
    pub shared: ExecAndRunSharedArgs,

    #[command(flatten)]
    pub walltime_args: exec_harness::walltime::WalltimeExecutionArgs,

    /// Optional benchmark name (defaults to command filename)
    #[arg(long)]
    pub name: Option<String>,

    /// The command to execute with the exec harness
    pub command: Vec<String>,
}

impl ExecArgs {
    /// Merge CLI args with project config if available
    ///
    /// CLI arguments take precedence over config values.
    pub fn merge_with_project_config(mut self, project_config: Option<&ProjectConfig>) -> Self {
        if let Some(project_config) = project_config {
            // Merge shared args
            self.shared =
                ConfigMerger::merge_shared_args(&self.shared, project_config.options.as_ref());
            // Merge walltime args
            self.walltime_args = ConfigMerger::merge_walltime_options(
                &self.walltime_args,
                project_config
                    .options
                    .as_ref()
                    .and_then(|o| o.walltime.as_ref()),
            );
        }
        self
    }
}

fn build_orchestrator_config(
    args: ExecArgs,
    target: executor::BenchmarkTarget,
) -> Result<OrchestratorConfig> {
    let modes = args.shared.resolve_modes()?;
    let raw_upload_url = args
        .shared
        .upload_url
        .unwrap_or_else(|| config::DEFAULT_UPLOAD_URL.into());
    let upload_url = Url::parse(&raw_upload_url)
        .map_err(|e| anyhow!("Invalid upload URL: {raw_upload_url}, {e}"))?;

    Ok(OrchestratorConfig {
        upload_url,
        token: args.shared.token,
        repository_override: args
            .shared
            .repository
            .map(|repo| RepositoryOverride::from_arg(repo, args.shared.provider))
            .transpose()?,
        working_directory: args.shared.working_directory,
        targets: vec![target],
        modes,
        instruments: Instruments { mongodb: None }, // exec doesn't support MongoDB
        perf_unwinding_mode: args.shared.perf_run_args.perf_unwinding_mode,
        enable_perf: args.shared.perf_run_args.enable_perf,
        simulation_tool: args.shared.simulation_tool.unwrap_or_default(),
        profile_folder: args.shared.profile_folder,
        skip_upload: args.shared.skip_upload,
        skip_run: args.shared.skip_run,
        skip_setup: args.shared.skip_setup,
        allow_empty: args.shared.allow_empty,
        go_runner_version: args.shared.go_runner_version,
    })
}

pub async fn run(
    args: ExecArgs,
    api_client: &CodSpeedAPIClient,
    codspeed_config: &CodSpeedConfig,
    project_config: Option<&ProjectConfig>,
    setup_cache_dir: Option<&Path>,
) -> Result<()> {
    let merged_args = args.merge_with_project_config(project_config);
    let target = executor::BenchmarkTarget::Exec {
        command: merged_args.command.clone(),
        name: merged_args.name.clone(),
        walltime_args: merged_args.walltime_args.clone(),
    };
    let config = build_orchestrator_config(merged_args, target)?;

    execute_config(config, api_client, codspeed_config, setup_cache_dir).await
}

/// Core execution logic shared by `codspeed exec` and `codspeed run` with config targets.
///
/// Sets up the orchestrator and drives execution. Exec-harness installation is handled
/// by the orchestrator when exec targets are present.
pub async fn execute_config(
    config: OrchestratorConfig,
    api_client: &CodSpeedAPIClient,
    codspeed_config: &CodSpeedConfig,
    setup_cache_dir: Option<&Path>,
) -> Result<()> {
    let orchestrator = executor::Orchestrator::new(config, codspeed_config, api_client).await?;

    if !orchestrator.is_local() {
        super::show_banner();
    }

    debug!("config: {:#?}", orchestrator.config);

    let poll_opts = PollResultsOptions::for_exec();
    let poll_results_fn = async |upload_result: &UploadResult| {
        poll_results(api_client, upload_result, &poll_opts).await
    };

    orchestrator
        .execute(setup_cache_dir, poll_results_fn)
        .await?;

    Ok(())
}
