use crate::MeasurementMode;
use std::process::Command;

/// Options needed in every mode: emit `/tmp/perf-<pid>.map` for JIT symbols.
const COMMON_NODE_OPTIONS: &[&str] = &["--perf-basic-prof"];

// FIXME(COD-3766): pass the full codspeed-node V8 flag set as CLI args, like
// `codspeed run` does via its introspected `node` wrapper.

/// Analysis-only options, taken from codspeed-node's analysis flags. Only the
/// subset that Node.js accepts in `NODE_OPTIONS` is passed.
///
/// `--interpreted-frames-native-stack`: without it, all interpreted JS frames
/// share V8's interpreter trampoline, so stacks cannot name the JS function.
/// It adds overhead, so it is kept out of walltime runs.
const ANALYSIS_NODE_OPTIONS: &[&str] = &[
    "--interpreted-frames-native-stack",
    "--max-old-space-size=4096",
];

/// Appends CodSpeed-required Node.js options to `NODE_OPTIONS` on a [`Command`],
/// preserving any existing value from the environment.
pub fn set_node_options(cmd: &mut Command, mode: MeasurementMode) {
    let existing = std::env::var("NODE_OPTIONS").unwrap_or_default();
    let mut parts: Vec<&str> = existing.split_whitespace().collect();

    let mode_options = match mode {
        MeasurementMode::Memory | MeasurementMode::Simulation => ANALYSIS_NODE_OPTIONS,
        MeasurementMode::Walltime => &[],
    };
    for opt in COMMON_NODE_OPTIONS.iter().chain(mode_options) {
        if !parts.contains(opt) {
            parts.push(opt);
        }
    }

    cmd.env("NODE_OPTIONS", parts.join(" "));
}
