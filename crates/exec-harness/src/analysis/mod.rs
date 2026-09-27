use crate::MeasurementMode;
use crate::constants::INTEGRATION_NAME;
use crate::constants::INTEGRATION_VERSION;
use crate::prelude::*;

use crate::BenchmarkCommand;
use crate::uri;
use instrument_hooks_bindings::InstrumentHooks;
use std::process::Command;

pub fn perform(commands: Vec<BenchmarkCommand>, mode: MeasurementMode) -> Result<()> {
    let hooks = InstrumentHooks::instance(INTEGRATION_NAME, INTEGRATION_VERSION);

    for benchmark_cmd in commands {
        let name_and_uri = uri::generate_name_and_uri(&benchmark_cmd.name, &benchmark_cmd.command);
        name_and_uri.print_executing();

        let mut cmd = Command::new(&benchmark_cmd.command[0]);
        cmd.args(&benchmark_cmd.command[1..]);

        if mode == MeasurementMode::Simulation {
            // Perf maps, so the runner can resolve JIT-ed frames afterwards.
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
