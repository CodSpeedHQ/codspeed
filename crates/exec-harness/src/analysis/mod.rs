use crate::MeasurementMode;
use crate::constants::INTEGRATION_NAME;
use crate::constants::INTEGRATION_VERSION;
use crate::prelude::*;

use crate::BenchmarkCommand;
use crate::uri;
use instrument_hooks_bindings::InstrumentHooks;
use std::process::Command;

/// Executes the given benchmark commands, measuring each one through the
/// instrument hooks.
///
/// Instrumentation is toggled in *this* process, around the spawn of each
/// benchmark command. Under Valgrind, the benchmarked child inherits the live
/// instrumentation state across `fork`/`exec`, and callgrind records the spawn
/// edge on the dump part that is live at fork time — the same part that
/// [`InstrumentHooks::set_executed_benchmark`] then names with the benchmark
/// URI. The backend walks that edge to attribute the child's trace to the
/// benchmark, so the measurement covers the whole spawned process tree.
///
/// This replaces the previous `LD_PRELOAD` shared library, which started
/// instrumentation from inside the benchmark process because the state did not
/// use to propagate across `fork`. Dropping it means statically linked
/// executables are now supported, since nothing has to be injected into them.
pub fn perform(commands: Vec<BenchmarkCommand>, mode: MeasurementMode) -> Result<()> {
    let hooks = InstrumentHooks::instance(INTEGRATION_NAME, INTEGRATION_VERSION);

    if !hooks.is_instrumented() {
        // Every way this mode can go wrong is silent: the harness runs, the
        // benchmark completes, and the measurement is empty. Fail loudly
        // instead.
        //
        // Note this only catches the absence of *any* instrument (no
        // instrument-hooks support compiled in, or nothing to attach to). It
        // cannot tell whether Valgrind will actually honour the instrumentation
        // toggles, which depends on the `--instr-atstart` the runner passes.
        bail!(
            "exec-harness found no instrument to report to, so nothing would be measured.\n\
             This binary is meant to be run by the CodSpeed CLI, which sets up the \
             instrumentation around it."
        );
    }

    for benchmark_cmd in commands {
        let name_and_uri = uri::generate_name_and_uri(&benchmark_cmd.name, &benchmark_cmd.command);
        name_and_uri.print_executing();

        let mut cmd = Command::new(&benchmark_cmd.command[0]);
        cmd.args(&benchmark_cmd.command[1..]);

        if mode == MeasurementMode::Simulation {
            // Make sure python and node processes output perf maps, so the
            // runner can resolve JIT-ed frames afterwards. For python this is
            // usually done by `pytest-codspeed`.
            cmd.env("PYTHONPERFSUPPORT", "1");
            crate::node::set_node_options(&mut cmd);
        }

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
