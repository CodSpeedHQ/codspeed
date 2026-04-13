use crate::executor::{ExecutorSupport, ToolInstallStatus, get_all_executors};
use crate::prelude::*;
use crate::system::SystemInfo;
use clap::{Args, Subcommand};
use console::style;
use std::path::Path;

use super::status::{check_mark, cross_mark, warn_mark};

#[derive(Debug, Default, Args)]
pub struct SetupArgs {
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
        None => setup(setup_cache_dir).await,
        Some(SetupCommands::Status) => status(),
    }
}

async fn setup(setup_cache_dir: Option<&Path>) -> Result<()> {
    let system_info = SystemInfo::new()?;
    let executors = get_all_executors();
    start_group!("Setting up the environment for all executors");
    for executor in executors {
        match executor.support_level(&system_info) {
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
                executor.setup(&system_info, setup_cache_dir).await?;
            }
        }
    }
    info!("Environment setup completed");
    end_group!();
    Ok(())
}

pub fn status() -> Result<()> {
    let system_info = SystemInfo::new()?;
    info!("{}", style("Tools").bold());
    for executor in get_all_executors() {
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
