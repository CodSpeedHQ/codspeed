use super::ExecAndRunSharedArgs;
use crate::api_client::CodSpeedAPIClient;
use crate::config::CodSpeedConfig;
use crate::executor;
use crate::executor::Config;
use crate::prelude::*;
use crate::project_config::ProjectConfig;
use crate::project_config::merger::ConfigMerger;
use crate::run_environment::interfaces::RepositoryProvider;
use crate::upload::UploadResult;
use crate::upload::poll_results::{PollResultsOptions, poll_results};
use clap::{Args, ValueEnum};
use std::path::Path;

pub mod helpers;
pub mod logger;

#[derive(Args, Debug)]
pub struct RunArgs {
    #[command(flatten)]
    pub shared: ExecAndRunSharedArgs,

    /// Comma-separated list of instruments to enable. Possible values: mongodb.
    #[arg(long, value_delimiter = ',')]
    pub instruments: Vec<String>,

    /// The name of the environment variable that contains the MongoDB URI to patch.
    /// If not provided, user will have to provide it dynamically through a CodSpeed integration.
    ///
    /// Only used if the `mongodb` instrument is enabled.
    #[arg(long)]
    pub mongo_uri_env_name: Option<String>,

    #[arg(long, hide = true)]
    pub message_format: Option<MessageFormat>,

    /// The bench command to run
    pub command: Vec<String>,
}

impl RunArgs {
    /// Merge CLI args with project config if available
    ///
    /// CLI arguments take precedence over config values.
    pub fn merge_with_project_config(mut self, project_config: Option<&ProjectConfig>) -> Self {
        if let Some(project_config) = project_config {
            self.shared =
                ConfigMerger::merge_shared_args(&self.shared, project_config.options.as_ref());
        }
        self
    }
}

#[derive(ValueEnum, Clone, Debug, PartialEq)]
pub enum MessageFormat {
    Json,
}

#[cfg(test)]
impl RunArgs {
    /// Constructs a new `RunArgs` with default values for testing purposes
    pub fn test() -> Self {
        use super::PerfRunArgs;
        use crate::RunnerMode;

        Self {
            shared: ExecAndRunSharedArgs {
                upload_url: None,
                token: None,
                repository: None,
                provider: None,
                working_directory: None,
                mode: vec![RunnerMode::Simulation],
                simulation_tool: None,
                profile_folder: None,
                skip_upload: false,
                skip_run: false,
                skip_setup: false,
                allow_empty: false,
                go_runner_version: None,
                perf_run_args: PerfRunArgs {
                    enable_perf: false,
                    perf_unwinding_mode: None,
                },
            },
            instruments: vec![],
            mongo_uri_env_name: None,
            message_format: None,
            command: vec![],
        }
    }
}

use crate::project_config::Target;
use crate::project_config::WalltimeOptions;
/// Determines the execution mode based on CLI args and project config
enum RunTarget<'a> {
    /// Single command from CLI args
    SingleCommand(RunArgs),
    /// Multiple targets from project config
    /// Note: for now, only `codspeed exec` targets are supported in the project config
    ConfigTargets {
        args: RunArgs,
        targets: &'a [Target],
        default_walltime: Option<&'a WalltimeOptions>,
    },
}

pub async fn run(
    args: RunArgs,
    api_client: &CodSpeedAPIClient,
    codspeed_config: &CodSpeedConfig,
    project_config: Option<&ProjectConfig>,
    setup_cache_dir: Option<&Path>,
) -> Result<()> {
    let output_json = args.message_format == Some(MessageFormat::Json);

    let args = args.merge_with_project_config(project_config);

    let run_target = if args.command.is_empty() {
        // No command provided - check for targets in project config
        let targets = project_config
            .and_then(|c| c.benchmarks.as_ref())
            .filter(|t| !t.is_empty())
            .ok_or_else(|| {
                anyhow!("No command provided and no targets defined in codspeed.yaml")
            })?;

        let default_walltime = project_config
            .and_then(|c| c.options.as_ref())
            .and_then(|o| o.walltime.as_ref());

        RunTarget::ConfigTargets {
            args,
            targets,
            default_walltime,
        }
    } else {
        RunTarget::SingleCommand(args)
    };

    match run_target {
        RunTarget::SingleCommand(args) => {
            let mut config = Config::try_from(args)?;

            let orchestrator =
                executor::Orchestrator::new(&mut config, codspeed_config, api_client).await?;

            if !orchestrator.is_local() {
                super::show_banner();
            }
            debug!("config: {config:#?}");

            let poll_opts = PollResultsOptions::for_run(output_json);
            let poll_results_fn = async |upload_result: &UploadResult| {
                poll_results(api_client, upload_result, &poll_opts).await
            };
            orchestrator
                .execute(&mut config, setup_cache_dir, poll_results_fn)
                .await?;
        }

        RunTarget::ConfigTargets {
            mut args,
            targets,
            default_walltime,
        } => {
            args.command =
                super::exec::multi_targets::build_pipe_command(targets, default_walltime)?;
            let config = Config::try_from(args)?;

            super::exec::execute_with_harness(config, api_client, codspeed_config, setup_cache_dir)
                .await?;
        }
    }

    Ok(())
}

// We have to implement this manually, because deriving the trait makes the CLI values `git-hub`
// and `git-lab`
impl clap::ValueEnum for RepositoryProvider {
    fn value_variants<'a>() -> &'a [Self] {
        &[Self::GitLab, Self::GitHub]
    }
    fn to_possible_value<'a>(&self) -> ::std::option::Option<clap::builder::PossibleValue> {
        match self {
            Self::GitLab => Some(clap::builder::PossibleValue::new("gitlab").aliases(["gl"])),
            Self::GitHub => Some(clap::builder::PossibleValue::new("github").aliases(["gh"])),
            Self::Project => None,
        }
    }
}
