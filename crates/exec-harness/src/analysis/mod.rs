use crate::constants::INTEGRATION_NAME;
use crate::constants::INTEGRATION_VERSION;
use crate::prelude::*;

use crate::BenchmarkCommand;
use crate::constants;
use crate::uri;
use instrument_hooks_bindings::InstrumentHooks;
use std::path::PathBuf;
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

        crate::node::set_node_options(&mut cmd);

        let mut child = cmd.spawn().context("Failed to spawn command")?;

        let status = child.wait().context("Failed to execute command")?;

        bail_if_command_spawned_subprocesses_under_valgrind(child.id())?;

        if !status.success() {
            bail!("Command exited with non-zero status: {status}");
        }
    }

    Ok(())
}

/// Checks if the benchmark process spawned subprocesses under valgrind by looking for <pid>.out
/// files in the profile folder.
///
/// The presence of <pid>.out files where <pid> is greater than the benchmark process pid indicates
/// that the benchmark process spawned subprocesses. This .out file will be almost empty, with a 0
/// cost reported due to the disabled instrumentation.
///
/// We currently do not support measuring processes that spawn subprocesses under valgrind, because
/// valgrind will not have its instrumentation in the new process.
/// The LD_PRELOAD trick that we use to inject our instrumentation into the benchmark process only
/// works for the first process.
///
/// TODO(COD-2163): Remove this once we support nested processes under valgrind
fn bail_if_command_spawned_subprocesses_under_valgrind(pid: u32) -> Result<()> {
    let Some(profile_folder) = std::env::var_os("CODSPEED_PROFILE_FOLDER") else {
        debug!("CODSPEED_PROFILE_FOLDER is not set, skipping subprocess detection");
        return Ok(());
    };

    let profile_folder = PathBuf::from(profile_folder);

    // Bail if any <pid>.out where <pid> > pid of the benchmark process exists in the profile
    // folder, which indicates that the benchmark process spawned subprocesses.
    for entry in std::fs::read_dir(profile_folder)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();

        if let Some(stripped) = file_name.strip_suffix(".out") {
            if let Ok(subprocess_pid) = stripped.parse::<u32>() {
                if subprocess_pid > pid {
                    bail!(
                        "The codspeed CLI in CPU Simulation mode does not support measuring processes that spawn other processes yet.\n\n\
                         Please either:\n\
                         - Use the walltime measurement mode, or\n\
                         - Benchmark a process that does not create subprocesses"
                    )
                }
            }
        }
    }

    Ok(())
}
