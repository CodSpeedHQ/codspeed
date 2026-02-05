use crate::constants::INTEGRATION_NAME;
use crate::constants::INTEGRATION_VERSION;
use crate::prelude::*;

use crate::BenchmarkCommand;
use crate::constants;
use crate::uri;
use instrument_hooks_bindings::InstrumentHooks;
use std::process::Command;

mod ld_preload_check;
mod preload_lib_file;

pub fn perform(commands: Vec<BenchmarkCommand>) -> Result<()> {
    let hooks = InstrumentHooks::instance(INTEGRATION_NAME, INTEGRATION_VERSION);

    for benchmark_cmd in commands {
        let name_and_uri = uri::generate_name_and_uri(&benchmark_cmd.name, &benchmark_cmd.command);
        name_and_uri.print_executing();

        let mut cmd = Command::new(&benchmark_cmd.command[0]);
        cmd.args(&benchmark_cmd.command[1..]);
        hooks.start_benchmark().unwrap();
        let status = cmd.status();
        hooks.stop_benchmark().unwrap();
        let status = status.context("Failed to execute command")?;

        if !status.success() {
            bail!("Command exited with non-zero status: {status}");
        }

        hooks.set_executed_benchmark(&name_and_uri.uri).unwrap();
    }

    Ok(())
}

/// Executes the given benchmark commands using a preload based trick to handle valgrind control.
///
/// This function is only supported on Unix-like platforms, as it relies on the
/// `LD_PRELOAD` environment variable and Unix file permissions for shared libraries.
/// It will not work on non-Unix platforms or with statically linked binaries.
pub fn perform_with_valgrind(commands: Vec<BenchmarkCommand>) -> Result<()> {
    let preload_lib_path = preload_lib_file::get_preload_lib_path()?;

    for benchmark_cmd in commands {
        // Check if the executable will honor LD_PRELOAD before running
        ld_preload_check::check_ld_preload_compatible(&benchmark_cmd.command[0])?;

        let name_and_uri = uri::generate_name_and_uri(&benchmark_cmd.name, &benchmark_cmd.command);
        name_and_uri.print_executing();

        let mut cmd = Command::new(&benchmark_cmd.command[0]);
        cmd.args(&benchmark_cmd.command[1..]);
        // Use LD_PRELOAD to inject instrumentation into the child process
        cmd.env("LD_PRELOAD", preload_lib_path);
        // Make sure python processes output perf maps. This is usually done by `pytest-codspeed`
        cmd.env("PYTHONPERFSUPPORT", "1");
        cmd.env(constants::URI_ENV, &name_and_uri.uri);

        let status = cmd.status().context("Failed to execute command")?;

        if !status.success() {
            bail!("Command exited with non-zero status: {status}");
        }
    }

    Ok(())
}
