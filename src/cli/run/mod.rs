use super::ExecAndRunSharedArgs;
use crate::api_client::CodSpeedAPIClient;
use crate::config::CodSpeedConfig;
use crate::executor;
use crate::executor::config::{self, OrchestratorConfig, RepositoryOverride};
use crate::instruments::Instruments;
use crate::prelude::*;
use crate::project_config::DiscoveredProjectConfig;
use crate::run_environment::interfaces::RepositoryProvider;
use crate::upload::poll_results::PollResultsOptions;
use clap::{Args, ValueEnum};
use std::collections::HashMap;
use std::path::Path;
use url::Url;

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

#[derive(ValueEnum, Clone, Debug, PartialEq)]
pub enum MessageFormat {
    Json,
}

#[cfg(test)]
impl RunArgs {
    /// Constructs a new `RunArgs` with default values for testing purposes
    pub fn test() -> Self {
        use super::PerfRunArgs;
        use super::experimental::ExperimentalArgs;
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
                show_full_output: false,
                base: None,
                perf_run_args: PerfRunArgs {
                    enable_perf: false,
                    perf_unwinding_mode: None,
                },
                experimental: ExperimentalArgs {
                    experimental_fair_sched: false,
                },
            },
            instruments: vec![],
            mongo_uri_env_name: None,
            message_format: None,
            command: vec![],
        }
    }
}

fn build_orchestrator_config(
    args: RunArgs,
    targets: Vec<executor::BenchmarkTarget>,
    poll_results_options: PollResultsOptions,
) -> Result<OrchestratorConfig> {
    let instruments = Instruments::try_from(&args)?;
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
        targets,
        modes,
        instruments,
        perf_unwinding_mode: args.shared.perf_run_args.perf_unwinding_mode,
        enable_perf: args.shared.perf_run_args.enable_perf,
        simulation_tool: args.shared.simulation_tool.unwrap_or_default(),
        profile_folder: args.shared.profile_folder,
        skip_upload: args.shared.skip_upload,
        skip_run: args.shared.skip_run,
        skip_setup: args.shared.skip_setup,
        allow_empty: args.shared.allow_empty,
        go_runner_version: args.shared.go_runner_version,
        show_full_output: args.shared.show_full_output,
        poll_results_options,
        extra_env: HashMap::new(),
        fair_sched: args.shared.experimental.experimental_fair_sched,
    })
}

use crate::project_config::{Target, WalltimeOptions};
/// Determines the execution mode based on CLI args and project config
enum RunTarget<'a> {
    /// Single command from CLI args
    SingleCommand(RunArgs),
    /// Multiple targets from project config
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
    discovered_config: Option<&DiscoveredProjectConfig>,
    setup_cache_dir: Option<&Path>,
) -> Result<()> {
    let output_json = args.message_format == Some(MessageFormat::Json);
    let project_config = discovered_config.map(|d| &d.config);
    let base_run_id = args.shared.base.clone();

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
            // SingleCommand: working_directory comes from --working-directory CLI flag only.
            // Config file's working-directory is NOT used.
            let command = args.command.join(" ");
            let poll_opts = PollResultsOptions::new(output_json, base_run_id);
            let config = build_orchestrator_config(
                args,
                vec![executor::BenchmarkTarget::Entrypoint {
                    command,
                    name: None,
                }],
                poll_opts,
            )?;

            let orchestrator =
                executor::Orchestrator::new(config, codspeed_config, api_client).await?;

            if !orchestrator.is_local() {
                super::show_banner();
            }
            debug!("config: {:?}", orchestrator.config);

            orchestrator.execute(setup_cache_dir, api_client).await?;
        }

        RunTarget::ConfigTargets {
            args,
            targets,
            default_walltime,
        } => {
            // ConfigTargets: working_directory is resolved relative to config file dir.
            // If --working-directory CLI flag is passed, ignore it with a warning.
            if args.shared.working_directory.is_some() {
                // Intentionally using eprintln! because logger has not been initialized yet.
                eprintln!(
                    "Warning: The --working-directory flag is ignored when running targets from the config file. \
                    Use the `working-directory` option in the config file instead."
                );
            }

            // Resolve working_directory relative to config file directory
            let resolved_working_directory =
                if let Some(config_dir) = discovered_config.and_then(|d| d.config_dir()) {
                    let root_wd = project_config
                        .and_then(|c| c.options.as_ref())
                        .and_then(|o| o.working_directory.as_ref());

                    match root_wd {
                        Some(wd) => {
                            let wd_path = Path::new(wd);
                            if wd_path.is_absolute() {
                                Some(wd.clone())
                            } else {
                                Some(config_dir.join(wd).to_string_lossy().into_owned())
                            }
                        }
                        None => Some(config_dir.to_string_lossy().into_owned()),
                    }
                } else {
                    None
                };

            let benchmark_targets =
                super::exec::multi_targets::build_benchmark_targets(targets, default_walltime)?;
            let mut config = build_orchestrator_config(
                args,
                benchmark_targets,
                PollResultsOptions::new(false, base_run_id),
            )?;
            config.working_directory = resolved_working_directory;
            super::exec::execute_config(config, api_client, codspeed_config, setup_cache_dir)
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
