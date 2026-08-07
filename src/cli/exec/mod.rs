use super::ExecAndRunSharedArgs;
use crate::api_client::CodSpeedAPIClient;
use crate::executor;
use crate::executor::config::{OrchestratorConfig, RepositoryOverride};
use crate::instruments::Instruments;
use crate::prelude::*;
use crate::project_config::ProjectConfig;
use crate::project_config::merger::ConfigMerger;
use crate::upload::poll_results::PollResultsOptions;
use clap::Args;
use std::collections::HashMap;
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

    /// Allow the benchmarked command to exit with a non-zero status code.
    ///
    /// When set, a non-zero exit from the benchmarked process is logged as a
    /// warning and measurement continues, instead of aborting. Mirrors
    /// hyperfine's `-i / --ignore-failure`.
    #[arg(short = 'i', long, default_value = "false")]
    pub ignore_failure: bool,

    /// The command to execute with the exec harness
    #[arg(required = true)]
    pub command: Vec<String>,
}

impl ExecArgs {
    /// Merge CLI args with project config if available
    ///
    /// CLI arguments take precedence over config values.
    pub fn merge_with_project_config(mut self, project_config: Option<&ProjectConfig>) -> Self {
        if let Some(project_config) = project_config {
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
    poll_results_options: PollResultsOptions,
) -> Result<OrchestratorConfig> {
    let modes = args.shared.resolve_modes()?;
    let raw_upload_url = args
        .shared
        .upload_url
        .unwrap_or_else(|| crate::config::DEFAULT_UPLOAD_URL.into());
    let upload_url = Url::parse(&raw_upload_url)
        .map_err(|e| anyhow!("Invalid upload URL: {raw_upload_url}, {e}"))?;

    Ok(OrchestratorConfig {
        upload_url,
        repository_override: args
            .shared
            .repository
            .map(|repo| RepositoryOverride::from_arg(repo, args.shared.provider))
            .transpose()?,
        working_directory: args.shared.working_directory,
        targets: vec![target],
        modes,
        instruments: Instruments { mongodb: None }, // exec doesn't support MongoDB
        perf_unwinding_mode: args.shared.profiler_run_args.perf.perf_unwinding_mode,
        enable_profiler: args.shared.profiler_run_args.resolve_enable_profiler(),
        walltime_profiler: args.shared.walltime_profiler,
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
        cycle_estimation: args.shared.cycle_estimation,
        exclude_allocations: args.shared.exclude_allocations,
    })
}

pub async fn run(
    args: ExecArgs,
    api_client: &mut CodSpeedAPIClient,
    project_config: Option<&ProjectConfig>,
    setup_cache_dir: Option<&Path>,
) -> Result<()> {
    let merged_args = args.merge_with_project_config(project_config);
    let base_run_id = merged_args.shared.base.clone();
    let target = executor::BenchmarkTarget::Exec {
        command: merged_args.command.clone(),
        name: merged_args.name.clone(),
        walltime_args: merged_args.walltime_args.clone(),
        ignore_failure: merged_args.ignore_failure,
    };
    let config = build_orchestrator_config(
        merged_args,
        target,
        PollResultsOptions::new(false, base_run_id),
    )?;

    execute_config(config, api_client, setup_cache_dir).await
}

/// Core execution logic shared by `codspeed exec` and `codspeed run` with config targets.
///
/// Sets up the orchestrator and drives execution. Exec-harness installation is handled
/// by the orchestrator when exec targets are present.
pub async fn execute_config(
    config: OrchestratorConfig,
    api_client: &mut CodSpeedAPIClient,
    setup_cache_dir: Option<&Path>,
) -> Result<()> {
    ensure_exec_commands_runnable(&config.targets)?;

    let orchestrator = executor::Orchestrator::new(config, api_client).await?;

    if !orchestrator.is_local() {
        super::show_banner();
    }

    debug!("config: {:#?}", orchestrator.config);

    orchestrator.execute(setup_cache_dir, api_client).await?;

    Ok(())
}

/// Rejects exec targets whose executable (first token) is missing or blank before
/// the orchestrator does any setup. A blank executable otherwise surfaces only as an
/// opaque failure deep inside exec-harness. Empty *arguments* after a real executable
/// stay valid.
fn ensure_exec_commands_runnable(targets: &[executor::BenchmarkTarget]) -> Result<()> {
    for target in targets {
        let executor::BenchmarkTarget::Exec { command, name, .. } = target else {
            continue;
        };
        if command.first().is_none_or(|exe| exe.trim().is_empty()) {
            let label = name.as_deref().unwrap_or("<unnamed>");
            bail!(
                "Empty command for exec benchmark target `{label}`. Provide a program to run \
                 (e.g. `codspeed exec -- <program> [args...]`, or set a non-empty `exec` in the config)."
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::cli::Cli;
    use crate::executor;
    use clap::Parser;

    #[test]
    fn exec_requires_a_command() {
        // `codspeed exec` with no command must be rejected at parse time instead of
        // proceeding into executor setup with an empty command.
        assert!(Cli::try_parse_from(["codspeed", "exec"]).is_err());
    }

    #[test]
    fn exec_accepts_a_command() {
        assert!(Cli::try_parse_from(["codspeed", "exec", "echo", "hello"]).is_ok());
    }

    fn exec_target(command: &[&str], name: Option<&str>) -> executor::BenchmarkTarget {
        executor::BenchmarkTarget::Exec {
            command: command.iter().map(|s| s.to_string()).collect(),
            name: name.map(str::to_string),
            walltime_args: Default::default(),
        }
    }

    #[test]
    fn rejects_missing_or_empty_executable() {
        // Empty vec (`exec: ""`), an empty first token (`codspeed exec ''` / `exec: "''"`),
        // and a whitespace-only token (`codspeed exec "   "`) are all invalid.
        assert!(super::ensure_exec_commands_runnable(&[exec_target(&[], Some("a"))]).is_err());
        assert!(super::ensure_exec_commands_runnable(&[exec_target(&["   "], Some("a"))]).is_err());
        let err = super::ensure_exec_commands_runnable(&[exec_target(&[""], Some("bench"))])
            .unwrap_err()
            .to_string();
        assert!(err.contains("bench"), "{err}");
    }

    #[test]
    fn accepts_runnable_command_with_empty_argument() {
        // An empty *argument* after a real executable stays valid (e.g. `grep '' file`).
        assert!(super::ensure_exec_commands_runnable(&[exec_target(&["grep", ""], None)]).is_ok());
    }

    #[test]
    fn ignores_entrypoint_targets() {
        let target = executor::BenchmarkTarget::Entrypoint {
            command: String::new(),
            name: None,
        };
        assert!(super::ensure_exec_commands_runnable(&[target]).is_ok());
    }
}
