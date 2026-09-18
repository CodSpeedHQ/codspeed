use crate::prelude::*;
use crate::walltime::WalltimeExecutionArgs;
use crate::{BenchmarkCommand, MeasurementMode, execute_benchmarks, read_commands_from_stdin};
use clap::Parser;
use std::ffi::OsString;

#[derive(Parser, Debug)]
#[command(name = "exec-harness")]
#[command(
    version,
    about = "CodSpeed exec harness - wraps commands with performance instrumentation"
)]
struct Args {
    /// Optional benchmark name, else the command will be used as the name
    #[arg(long)]
    name: Option<String>,

    /// Set by the runner, should be coherent with the executor being used
    #[arg(short, long, global = true, env = "CODSPEED_RUNNER_MODE", hide = true)]
    measurement_mode: Option<MeasurementMode>,

    #[command(flatten)]
    walltime_args: WalltimeExecutionArgs,

    /// The command and arguments to execute.
    /// Use "-" as the only argument to read a JSON payload from stdin.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

/// Parse `argv` and run the requested benchmarks.
pub fn run_cli<I, T>(argv: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    debug!("Starting exec-harness with pid {}", std::process::id());

    let args = Args::parse_from(argv);
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
