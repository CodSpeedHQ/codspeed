use clap::ValueEnum;
use prelude::*;
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead};

pub mod analysis;
pub mod constants;
pub mod node;
pub mod prelude;
mod uri;
pub mod walltime;

#[derive(ValueEnum, Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MeasurementMode {
    Walltime,
    Memory,
    #[value(alias = "instrumentation")]
    Simulation,
}

/// A single benchmark command for stdin mode input.
///
/// This struct defines the JSON format for passing benchmark commands to exec-harness
/// via stdin (when invoked with `-`). The runner uses this same struct to serialize
/// targets from codspeed.yaml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkCommand {
    /// The command and arguments to execute
    pub command: Vec<String>,

    /// Optional benchmark name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Walltime execution options (flattened into the JSON object)
    #[serde(default)]
    pub walltime_args: walltime::WalltimeExecutionArgs,

    /// When true, a non-zero exit from the command is logged as a warning
    /// instead of aborting execution.
    #[serde(default)]
    pub ignore_failure: bool,
}

/// Read and parse benchmark commands from stdin as JSON
pub fn read_commands_from_stdin() -> Result<Vec<BenchmarkCommand>> {
    let stdin = io::stdin();
    let mut input = String::new();

    for line in stdin.lock().lines() {
        let line = line.context("Failed to read line from stdin")?;
        input.push_str(&line);
        input.push('\n');
    }

    let commands: Vec<BenchmarkCommand> =
        serde_json::from_str(&input).context("Failed to parse JSON from stdin")?;

    if commands.is_empty() {
        bail!("No commands provided in stdin input");
    }

    for cmd in &commands {
        if cmd.command.is_empty() {
            bail!("Empty command in stdin input");
        }
    }

    Ok(commands)
}

/// Execute benchmark commands
pub fn execute_benchmarks(
    commands: Vec<BenchmarkCommand>,
    measurement_mode: Option<MeasurementMode>,
) -> Result<()> {
    match measurement_mode {
        Some(MeasurementMode::Walltime) | None => {
            walltime::perform(commands)?;
        }
        Some(MeasurementMode::Memory) => {
            analysis::perform(commands)?;
        }
        Some(MeasurementMode::Simulation) => {
            analysis::perform_with_valgrind(commands)?;
        }
    }

    Ok(())
}
