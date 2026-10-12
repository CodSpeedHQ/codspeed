//! Runtime env for benchmark processes started by exec-harness: the shared
//! [`runner_shared::runtime_env::env`] vars, `PYTHONPERFSUPPORT`, and the
//! `node` wrapper on `PATH`.

use crate::MeasurementMode;
use std::ffi::OsString;

mod node;

/// Env vars to set on benchmark processes for `mode`.
pub fn env(mode: MeasurementMode) -> anyhow::Result<Vec<(&'static str, OsString)>> {
    let mut env: Vec<(&'static str, OsString)> = runner_shared::runtime_env::env(mode)
        .into_iter()
        .map(|(key, value)| (key, value.into()))
        .collect();
    // Makes CPython write `/tmp/perf-<pid>.map` (or jitdump when
    // `PYTHON_PERF_JIT_SUPPORT=1`) so profiles can name Python frames. Only set
    // for exec-harness: integrations with their own harness (pytest-codspeed)
    // crash with the perf trampoline enabled.
    env.push(("PYTHONPERFSUPPORT", "1".into()));
    let path = std::env::var_os("PATH").unwrap_or_default();
    env.push(("PATH", node::path_with_node_wrapper(&path)?));
    Ok(env)
}

/// Applies [`env`] to the current process, so every child inherits it.
/// Existing values are overwritten: `mode` may come from a CLI flag that
/// differs from an inherited `CODSPEED_RUNNER_MODE`.
///
/// # Safety
/// Must be called while the process is single-threaded.
pub(crate) unsafe fn apply(mode: MeasurementMode) -> anyhow::Result<()> {
    for (key, value) in env(mode)? {
        unsafe { std::env::set_var(key, value) };
    }
    Ok(())
}
