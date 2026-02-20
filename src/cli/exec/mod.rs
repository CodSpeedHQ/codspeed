use super::ExecAndRunSharedArgs;
use crate::api_client::CodSpeedAPIClient;
use crate::binary_installer::ensure_binary_installed;
use crate::config::CodSpeedConfig;
use crate::executor;
use crate::prelude::*;
use crate::project_config::ProjectConfig;
use crate::project_config::merger::ConfigMerger;
use crate::upload::UploadResult;
use crate::upload::poll_results::{PollResultsOptions, poll_results};
use clap::Args;
use std::path::Path;

pub mod multi_targets;

/// We temporarily force this name for all exec runs
pub const DEFAULT_REPOSITORY_NAME: &str = "local-runs";

const EXEC_HARNESS_COMMAND: &str = "exec-harness";
const EXEC_HARNESS_VERSION: &str = "1.1.0";

/// Wraps a command with exec-harness and the given walltime arguments.
///
/// This produces a shell command string like:
/// `exec-harness --warmup-time 1s --max-rounds 10 sleep 0.1`
pub fn wrap_with_exec_harness(
    walltime_args: &exec_harness::walltime::WalltimeExecutionArgs,
    command: &[String],
) -> String {
    shell_words::join(
        std::iter::once(EXEC_HARNESS_COMMAND)
            .chain(walltime_args.to_cli_args().iter().map(|s| s.as_str()))
            .chain(command.iter().map(|s| s.as_str())),
    )
}

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

pub async fn run(
    args: ExecArgs,
    api_client: &CodSpeedAPIClient,
    codspeed_config: &CodSpeedConfig,
    project_config: Option<&ProjectConfig>,
    setup_cache_dir: Option<&Path>,
) -> Result<()> {
    let merged_args = args.merge_with_project_config(project_config);
    let config = crate::executor::Config::try_from(merged_args)?;

    execute_with_harness(config, api_client, codspeed_config, setup_cache_dir).await
}

/// Core execution logic for exec-harness based runs.
///
/// This function handles exec-harness installation and benchmark execution with exec-style
/// result polling. It is used by both `codspeed exec` directly and by `codspeed run` when
/// executing targets defined in codspeed.yaml.
pub async fn execute_with_harness(
    mut config: crate::executor::Config,
    api_client: &CodSpeedAPIClient,
    codspeed_config: &CodSpeedConfig,
    setup_cache_dir: Option<&Path>,
) -> Result<()> {
    let orchestrator =
        executor::Orchestrator::new(&mut config, codspeed_config, api_client).await?;

    if !orchestrator.is_local() {
        super::show_banner();
    }

    debug!("config: {config:#?}");

    let get_exec_harness_installer_url = || {
        format!(
            "https://github.com/CodSpeedHQ/codspeed/releases/download/exec-harness-v{EXEC_HARNESS_VERSION}/exec-harness-installer.sh"
        )
    };

    // Ensure the exec-harness is installed
    ensure_binary_installed(
        EXEC_HARNESS_COMMAND,
        EXEC_HARNESS_VERSION,
        get_exec_harness_installer_url,
    )
    .await?;

    let poll_opts = PollResultsOptions::for_exec();
    let poll_results_fn = async |upload_result: &UploadResult| {
        poll_results(api_client, upload_result, &poll_opts).await
    };

    orchestrator
        .execute(&mut config, setup_cache_dir, poll_results_fn)
        .await?;

    Ok(())
}
