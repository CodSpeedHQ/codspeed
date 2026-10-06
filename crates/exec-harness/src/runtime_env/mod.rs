//! Runtime env for benchmark processes started by exec-harness: the shared
//! [`runner_shared::runtime_env::env`] vars plus the `node` wrapper on `PATH`.

use crate::MeasurementMode;

mod node;

const PATH_ENV: &str = "PATH";

/// Applies the runtime env and the node wrapper to the current process, so
/// every child inherits them. Existing values are overwritten: `mode` may come
/// from a CLI flag that differs from an inherited `CODSPEED_RUNNER_MODE`.
///
/// # Safety
/// Must be called while the process is single-threaded.
pub(crate) unsafe fn apply(mode: MeasurementMode) -> anyhow::Result<()> {
    for (key, value) in runner_shared::runtime_env::env(mode) {
        unsafe { std::env::set_var(key, value) };
    }
    let path = std::env::var_os(PATH_ENV).unwrap_or_default();
    unsafe { std::env::set_var(PATH_ENV, node::path_with_node_wrapper(&path)?) };
    Ok(())
}
