//! `node` wrapper for `codspeed exec`.
//!
//! The V8 flags we need slow node down, so they should only apply to the
//! benchmark process.
//!
//! In `codspeed run`, the runner's `introspected_nodejs` wrapper does this:
//! codspeed-node requests the flags through introspection, so non-benchmark
//! node processes are left alone.
//!
//! In `codspeed exec`, there is no codspeed-node and so no way to know which
//! process is the benchmark. This wrapper therefore adds the flags to every
//! `node` call. It mirrors codspeed-node's `getV8Flags()`.
//!
//! The two wrappers are never on PATH together (`enable_introspection` is
//! false for exec-harness targets).

use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Mutex;

const NODE_WRAPPER_SCRIPT: &str = include_str!("node.sh");
const WRAPPER_DIR_PREFIX: &str = "codspeed_node_wrapper";
const WRAPPER_FILE_NAME: &str = "node";
const EXECUTABLE_MODE: u32 = 0o755;

/// Folder of the wrapper installed by this process, once it is installed.
static INSTALLED_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Writes the `node` wrapper script and returns its folder.
///
/// The folder is a fresh temp dir with a random name, created exclusively and
/// writable only by us: a shared, predictable path could be pre-created by
/// another user, who could then replace the script that ends up first on
/// `PATH`. It is kept after the process exits, since benchmarks execute the
/// wrapper until then.
///
/// A process installs at most once. A second writer in the same process would
/// race with concurrent `fork`+`exec` calls: the child inherits the writable
/// descriptor and then fails to execute the script with `ETXTBSY`.
fn install_wrapper() -> std::io::Result<PathBuf> {
    let mut installed = INSTALLED_DIR.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(dir) = installed.as_ref() {
        return Ok(dir.clone());
    }

    let dir = tempfile::Builder::new()
        .prefix(WRAPPER_DIR_PREFIX)
        .tempdir()?
        .keep();
    let script = dir.join(WRAPPER_FILE_NAME);
    std::fs::write(&script, NODE_WRAPPER_SCRIPT)?;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(EXECUTABLE_MODE))?;

    *installed = Some(dir.clone());
    Ok(dir)
}

/// Installs the `node` wrapper and returns `path` with its folder prepended.
pub(super) fn path_with_node_wrapper(path: &OsStr) -> anyhow::Result<OsString> {
    let wrapper_dir = install_wrapper()?;
    Ok(std::env::join_paths(
        std::iter::once(wrapper_dir).chain(std::env::split_paths(path)),
    )?)
}

#[cfg(test)]
mod tests;
