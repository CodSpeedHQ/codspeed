use clap::{Parser, ValueEnum};
use prelude::*;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
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

/// Top-level CLI parser for the exec-harness entry point. The same parser is
/// used both by the standalone `exec-harness` binary (via [`run_cli`]) and by
/// `codspeed tool exec-harness` in the main CLI.
#[derive(Parser, Debug)]
#[command(name = "exec-harness")]
#[command(
    version,
    about = "CodSpeed exec harness - wraps commands with performance instrumentation"
)]
pub struct CliArgs {
    /// Optional benchmark name, else the command will be used as the name
    #[arg(long)]
    name: Option<String>,

    /// Set by the runner, should be coherent with the executor being used
    #[arg(short, long, global = true, env = "CODSPEED_RUNNER_MODE", hide = true)]
    measurement_mode: Option<MeasurementMode>,

    #[command(flatten)]
    walltime_args: walltime::WalltimeExecutionArgs,

    /// The command and arguments to execute.
    /// Use "-" as the only argument to read a JSON payload from stdin.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

/// Parse `argv` as exec-harness CLI args (first element is the program name)
/// and run the harness. Initializes `env_logger` on the way in so logs from
/// the harness reach the runner's captured output.
pub fn run_cli<I, T>(argv: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    env_logger::builder()
        .parse_env(env_logger::Env::new().filter_or("CODSPEED_LOG", "info"))
        .format(|buf, record| {
            use std::io::Write;
            writeln!(buf, "{}", record.args())
        })
        .try_init()
        .ok();

    debug!("Starting exec-harness with pid {}", std::process::id());

    let args = CliArgs::parse_from(argv);
    let measurement_mode = args.measurement_mode;

    let commands = match args.command.as_slice() {
        [single] if single == "-" => read_commands_from_stdin()?,
        [] => bail!("No command provided"),
        _ => vec![BenchmarkCommand {
            command: args.command,
            name: args.name,
            walltime_args: args.walltime_args,
        }],
    };

    execute_benchmarks(commands, measurement_mode)?;

    Ok(())
}
