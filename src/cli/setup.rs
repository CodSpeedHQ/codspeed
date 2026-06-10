use crate::executor::{
    Executor, ExecutorSupport, ToolInstallStatus, get_all_executors, get_executor_from_mode,
};
use crate::prelude::*;
use crate::runner_mode::RunnerMode;
use crate::system::SystemInfo;
use clap::{Args, Subcommand};
use console::style;
use std::path::Path;

use super::status::{check_mark, cross_mark, warn_mark};

#[derive(Debug, Default, Args)]
pub struct SetupArgs {
    /// The modes to set up. If omitted, the environment is set up for all supported executors.
    #[arg(
        short,
        long,
        value_enum,
        env = "CODSPEED_RUNNER_MODE",
        value_delimiter = ','
    )]
    mode: Vec<RunnerMode>,

    #[command(subcommand)]
    command: Option<SetupCommands>,
}

#[derive(Debug, Subcommand)]
enum SetupCommands {
    /// Show the installation status of CodSpeed tools
    Status,
}

pub async fn run(args: SetupArgs, setup_cache_dir: Option<&Path>) -> Result<()> {
    match args.command {
        Some(SetupCommands::Status) => status(&args.mode),
        None => setup(&args.mode, setup_cache_dir).await,
    }
}

/// Resolve the executors to operate on from the requested modes.
///
/// An empty list of modes means "every executor".
fn get_executors_from_modes(modes: &[RunnerMode]) -> Vec<Box<dyn Executor>> {
    if modes.is_empty() {
        get_all_executors()
    } else {
        modes
            .iter()
            .map(|mode| get_executor_from_mode(mode, None))
            .collect()
    }
}

async fn setup(modes: &[RunnerMode], setup_cache_dir: Option<&Path>) -> Result<()> {
    let system_info = SystemInfo::new()?;
    let executors = get_executors_from_modes(modes);
    start_group!("Setting up the environment");
    for executor in executors {
        setup_executor(executor.as_ref(), &system_info, setup_cache_dir).await?;
    }
    info!("Environment setup completed");
    end_group!();
    Ok(())
}

/// Set up a single executor based on its support level on the current system.
///
/// Unsupported executors or executors that require manual installation are
/// skipped, not treated as fatal.
async fn setup_executor(
    executor: &dyn Executor,
    system_info: &SystemInfo,
    setup_cache_dir: Option<&Path>,
) -> Result<()> {
    match executor.support_level(system_info) {
        ExecutorSupport::Unsupported => {
            info!(
                "Skipping setup for the {} executor: not supported on {}",
                executor.name(),
                system_info.os
            );
        }
        ExecutorSupport::RequiresManualInstallation => {
            info!(
                "Skipping automatic setup for the {} executor on {}; install required tooling manually.",
                executor.name(),
                system_info.os
            );
        }
        ExecutorSupport::FullySupported => {
            info!(
                "Setting up the environment for the executor: {}",
                executor.name()
            );
            executor.setup(system_info, setup_cache_dir).await?;
        }
    }
    Ok(())
}

pub fn status(modes: &[RunnerMode]) -> Result<()> {
    let system_info = SystemInfo::new()?;
    info!("{}", style("Tools").bold());
    for executor in get_executors_from_modes(modes) {
        // Don't probe for tooling that can't be used on this OS anyway.
        if executor.support_level(&system_info) == ExecutorSupport::Unsupported {
            continue;
        }
        match executor.tool_status() {
            Some(tool_status) => match &tool_status.status {
                ToolInstallStatus::Installed { version } => {
                    info!(
                        "  {} {} executor: {} ({})",
                        check_mark(),
                        executor.name(),
                        tool_status.tool_name,
                        version
                    );
                }
                ToolInstallStatus::IncorrectVersion { version, message } => {
                    info!(
                        "  {} {} executor: {} ({}, {})",
                        warn_mark(),
                        executor.name(),
                        tool_status.tool_name,
                        version,
                        message
                    );
                }
                ToolInstallStatus::NotInstalled => {
                    info!(
                        "  {} {} executor: {} (not installed)",
                        cross_mark(),
                        executor.name(),
                        tool_status.tool_name
                    );
                }
            },
            None => {
                info!(
                    "  {} {} executor: No tool to install",
                    check_mark(),
                    executor.name()
                );
            }
        }
    }
    Ok(())
}
