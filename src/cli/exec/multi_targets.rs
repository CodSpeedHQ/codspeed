use crate::executor::config::BenchmarkTarget;
use crate::executor::orchestrator::EXEC_HARNESS_COMMAND;
use crate::prelude::*;
use crate::project_config::{Target, TargetCommand, WalltimeOptions};
use exec_harness::BenchmarkCommand;

/// Merge default walltime options with target-specific overrides
fn merge_walltime_options(
    default: Option<&WalltimeOptions>,
    target: Option<&WalltimeOptions>,
) -> exec_harness::walltime::WalltimeExecutionArgs {
    let default_args = default.map(walltime_options_to_args);
    let target_args = target.map(walltime_options_to_args);

    match (default_args, target_args) {
        (None, None) => exec_harness::walltime::WalltimeExecutionArgs::default(),
        (Some(d), None) => d,
        (None, Some(t)) => t,
        (Some(d), Some(t)) => exec_harness::walltime::WalltimeExecutionArgs {
            warmup_time: t.warmup_time.or(d.warmup_time),
            max_time: t.max_time.or(d.max_time),
            min_time: t.min_time.or(d.min_time),
            max_rounds: t.max_rounds.or(d.max_rounds),
            min_rounds: t.min_rounds.or(d.min_rounds),
        },
    }
}

/// Convert project config WalltimeOptions to exec-harness WalltimeExecutionArgs
fn walltime_options_to_args(
    opts: &WalltimeOptions,
) -> exec_harness::walltime::WalltimeExecutionArgs {
    exec_harness::walltime::WalltimeExecutionArgs {
        warmup_time: opts.warmup_time.clone(),
        max_time: opts.max_time.clone(),
        min_time: opts.min_time.clone(),
        max_rounds: opts.max_rounds,
        min_rounds: opts.min_rounds,
    }
}

/// Convert project config targets into [`BenchmarkTarget`] instances.
///
/// Exec targets are each converted to a `BenchmarkTarget::Exec`.
/// Entrypoint targets are each converted to a `BenchmarkTarget::Entrypoint`.
pub fn build_benchmark_targets(
    targets: &[Target],
    default_walltime: Option<&WalltimeOptions>,
) -> Result<Vec<BenchmarkTarget>> {
    targets
        .iter()
        .map(|target| match &target.command {
            TargetCommand::Exec { exec } => {
                let command = shell_words::split(exec)
                    .with_context(|| format!("Failed to parse command: {exec}"))?;
                let target_walltime = target.options.as_ref().and_then(|o| o.walltime.as_ref());
                let walltime_args = merge_walltime_options(default_walltime, target_walltime);
                Ok(BenchmarkTarget::Exec {
                    command,
                    name: target.name.clone(),
                    walltime_args,
                })
            }
            TargetCommand::Entrypoint { entrypoint } => Ok(BenchmarkTarget::Entrypoint {
                command: entrypoint.clone(),
                name: target.name.clone(),
            }),
        })
        .collect()
}

/// Build a shell command string that pipes BenchmarkTarget::Exec variants to exec-harness via stdin
pub fn build_exec_targets_pipe_command(
    targets: &[&crate::executor::config::BenchmarkTarget],
) -> Result<String> {
    let inputs: Vec<BenchmarkCommand> = targets
        .iter()
        .map(|target| match target {
            crate::executor::config::BenchmarkTarget::Exec {
                command,
                name,
                walltime_args,
            } => Ok(BenchmarkCommand {
                command: command.clone(),
                name: name.clone(),
                walltime_args: walltime_args.clone(),
            }),
            crate::executor::config::BenchmarkTarget::Entrypoint { .. } => {
                bail!("Entrypoint targets cannot be used with exec-harness pipe command")
            }
        })
        .collect::<Result<Vec<_>>>()?;

    let json = serde_json::to_string(&inputs).context("Failed to serialize targets to JSON")?;
    Ok(build_pipe_command_from_json(&json))
}

fn build_pipe_command_from_json(json: &str) -> String {
    format!("{EXEC_HARNESS_COMMAND} - <<'CODSPEED_EOF'\n{json}\nCODSPEED_EOF")
}
