use clap::Parser;
use exec_harness::prelude::*;
use exec_harness::walltime::WalltimeExecutionArgs;
use exec_harness::{
    BenchmarkCommand, MeasurementMode, execute_benchmarks, read_commands_from_stdin,
};

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

    /// Allow the benchmarked command to exit with a non-zero status code.
    ///
    /// When set, a non-zero exit from the benchmarked process is logged as a
    /// warning and measurement continues, instead of aborting.
    #[arg(short = 'i', long, default_value = "false")]
    ignore_failure: bool,

    #[command(flatten)]
    walltime_args: WalltimeExecutionArgs,

    /// The command and arguments to execute.
    /// Use "-" as the only argument to read a JSON payload from stdin.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

fn main() -> Result<()> {
    env_logger::builder()
        .parse_env(env_logger::Env::new().filter_or("CODSPEED_LOG", "info"))
        .format(|buf, record| {
            use std::io::Write;
            writeln!(buf, "{}", record.args())
        })
        .init();

    debug!("Starting exec-harness with pid {}", std::process::id());

    let args = Args::parse();
    let measurement_mode = args.measurement_mode;

    // Determine if we're in stdin mode or CLI mode
    let commands = match args.command.as_slice() {
        [single] if single == "-" => read_commands_from_stdin()?,
        [] => bail!("No command provided"),
        _ => vec![BenchmarkCommand {
            command: args.command,
            name: args.name,
            walltime_args: args.walltime_args,
            ignore_failure: args.ignore_failure,
        }],
    };

    execute_benchmarks(commands, measurement_mode)?;

    Ok(())
}
