use crate::measurement_mode::MeasurementMode;

const PYTHONHASHSEED: &str = "PYTHONHASHSEED";
const PYTHON_PERF_JIT_SUPPORT: &str = "PYTHON_PERF_JIT_SUPPORT";
const PYTHONPERFSUPPORT: &str = "PYTHONPERFSUPPORT";

/// Python env:
/// - `PYTHONHASHSEED=0`: deterministic hashing in every mode.
/// - `PYTHON_PERF_JIT_SUPPORT`: perf jitdump for walltime profiles.
///   FIXME(COD-2645): Keep this disabled on macOS. Enabling it causes
///   many unresolved addresses on the stack when profiling with samply.
/// - `PYTHONPERFSUPPORT=1`: `/tmp/perf-<pid>.map` so memory flamegraphs can name Python frames.
pub(super) fn env(mode: MeasurementMode) -> Vec<(&'static str, String)> {
    let perf_jit = mode == MeasurementMode::Walltime && !cfg!(target_os = "macos");
    let mut env = vec![
        (PYTHONHASHSEED, "0".to_owned()),
        (PYTHON_PERF_JIT_SUPPORT, u8::from(perf_jit).to_string()),
    ];
    if mode == MeasurementMode::Memory {
        env.push((PYTHONPERFSUPPORT, "1".to_owned()));
    }
    env
}
