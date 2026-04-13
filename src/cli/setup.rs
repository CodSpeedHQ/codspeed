use crate::executor::{ToolInstallStatus, get_all_executors};
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
        Some(SetupCommands::Status) => {
            status();
            Ok(())
        }
    }
}

async fn setup(setup_cache_dir: Option<&Path>) -> Result<()> {
    let system_info = SystemInfo::new()?;
    let executors = get_all_executors();
    start_group!("Setting up the environment for all executors");
    for executor in executors {
        info!(
            "Setting up the environment for the executor: {}",
            executor.name()
        );
        executor.setup(&system_info, setup_cache_dir).await?;
    }
    info!("Environment setup completed");
    end_group!();
    Ok(())
}

pub fn status() {
    info!("{}", style("Tools").bold());
    for executor in get_all_executors() {
        let tool_status = executor.tool_status();
        match &tool_status.status {
            ToolInstallStatus::Installed { version } => {
                info!("  {} {} ({})", check_mark(), tool_status.tool_name, version);
            }
            ToolInstallStatus::IncorrectVersion { version, message } => {
                info!(
                    "  {} {} ({}, {})",
                    warn_mark(),
                    tool_status.tool_name,
                    version,
                    message
                );
            }
            ToolInstallStatus::NotInstalled => {
                info!(
                    "  {} {} (not installed)",
                    cross_mark(),
                    tool_status.tool_name
                );
            }
        }
    }
}
