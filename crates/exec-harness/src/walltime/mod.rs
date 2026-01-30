mod benchmark_loop;
mod config;

pub use config::ExecutionOptions;
pub use config::WalltimeExecutionArgs;
use runner_shared::walltime_results::Creator;
use runner_shared::walltime_results::WalltimeBenchmark;
pub use runner_shared::walltime_results::WalltimeResults;

use crate::BenchmarkCommand;
use crate::constants::INTEGRATION_NAME;
use crate::constants::INTEGRATION_VERSION;
use crate::prelude::*;
use crate::uri::NameAndUri;
use crate::uri::generate_name_and_uri;

pub fn perform(commands: Vec<BenchmarkCommand>) -> Result<()> {
    let mut walltime_benchmarks = Vec::with_capacity(commands.len());

    for cmd in commands {
        let name_and_uri = generate_name_and_uri(&cmd.name, &cmd.command);
        name_and_uri.print_executing();
        let execution_options: ExecutionOptions = cmd.walltime_args.try_into()?;

        let NameAndUri {
            name: bench_name,
            uri: bench_uri,
            ..
        } = name_and_uri;

        let times_per_round_ns =
            benchmark_loop::run_rounds(bench_uri.clone(), cmd.command, &execution_options)?;

        // Collect walltime results
        let max_time_ns = times_per_round_ns.iter().copied().max();

        let walltime_benchmark = WalltimeBenchmark::from_runtime_data(
            bench_name.clone(),
            bench_uri.clone(),
            vec![1; times_per_round_ns.len()],
            times_per_round_ns,
            max_time_ns,
        );

        walltime_benchmarks.push(walltime_benchmark);
    }

    let walltime_results = WalltimeResults::new(
        Creator {
            name: INTEGRATION_NAME.to_string(),
            version: INTEGRATION_VERSION.to_string(),
            pid: std::process::id(),
        },
        walltime_benchmarks,
    )
    .expect("Failed to create walltime results");

    walltime_results
        .save_to_file(
            std::env::var("CODSPEED_PROFILE_FOLDER")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::env::current_dir().unwrap().join(".codspeed")),
        )
        .context("Failed to save walltime results")?;

    Ok(())
}

#[cfg(test)]
mod tests;
