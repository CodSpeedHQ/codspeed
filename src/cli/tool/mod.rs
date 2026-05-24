//! Hidden `codspeed tool <name>` subcommand group.
//!
//! Used by the runner to re-exec itself for bundled helper binaries (samply,
//! exec-harness, memtrack). Not user-facing — `codspeed tool` is hidden from
//! the top-level help.

pub(crate) mod exec_harness;
pub(crate) mod memtrack;
pub(crate) mod samply;

use clap::Subcommand;

use crate::executor::helpers::command::CommandBuilder;
use crate::prelude::*;

#[derive(clap::Args, Debug)]
pub struct ToolArgs {
    #[command(subcommand)]
    pub command: ToolCommand,
}

#[derive(Subcommand, Debug)]
pub(crate) enum ToolCommand {
    /// Run the bundled samply profiler. Args are forwarded to samply.
    #[command(disable_help_flag = true, disable_help_subcommand = true)]
    Samply(samply::SamplyArgs),
    /// Run the bundled exec-harness. Args are forwarded verbatim.
    #[command(name = "exec-harness", disable_help_flag = true)]
    ExecHarness(exec_harness::ExecHarnessArgs),
    /// Run the bundled memtrack (Linux-only; errors on other platforms). Args
    /// are forwarded verbatim.
    #[command(disable_help_flag = true)]
    Memtrack(memtrack::MemtrackArgs),
}

impl ToolCommand {
    /// Build a [`CommandBuilder`] that re-execs the current binary into this
    /// tool subcommand.
    pub fn get_command_builder(&self) -> Result<CommandBuilder> {
        let current_exe = std::env::current_exe()
            .context("failed to resolve current executable for tool subcommand")?;
        let mut builder = CommandBuilder::new(current_exe);
        builder.arg("tool");
        match self {
            ToolCommand::Samply(args) => {
                builder.arg("samply");
                builder.args(args.args.iter().cloned());
            }
            ToolCommand::ExecHarness(args) => {
                builder.arg("exec-harness");
                builder.args(args.args.iter().cloned());
            }
            ToolCommand::Memtrack(args) => {
                builder.arg("memtrack");
                builder.args(args.args.iter().cloned());
            }
        }
        Ok(builder)
    }
}

pub fn run(args: ToolArgs) -> Result<()> {
    match args.command {
        ToolCommand::Samply(args) => samply::run(args),
        ToolCommand::ExecHarness(args) => exec_harness::run(args),
        ToolCommand::Memtrack(args) => memtrack::run(args),
    }
}
