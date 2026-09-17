use crate::cli::self_exe;
use crate::executor::helpers::capabilities::binary_has_capabilities;
use crate::executor::helpers::run_with_sudo::{is_root_user, run_with_sudo};
use crate::executor::{ToolInstallStatus, ToolStatus};
use crate::prelude::*;
use caps::Capability;
use std::path::PathBuf;

/// How memtrack is named in user-facing messages. It is no longer a binary to
/// look up: memtrack is bundled into this executable as a hidden subcommand.
pub const MEMTRACK_COMMAND: &str = "memtrack";

const MEMTRACK_REQUIRED_CAPS: &[Capability] = &[
    Capability::CAP_DAC_READ_SEARCH,
    Capability::CAP_SYS_ADMIN,
    Capability::CAP_PERFMON,
    Capability::CAP_BPF,
    Capability::CAP_SYS_RESOURCE,
];

fn memtrack_required_caps_mask() -> u64 {
    MEMTRACK_REQUIRED_CAPS
        .iter()
        .fold(0, |acc, c| acc | c.bitmask())
}

/// `setcap` grammar form of [`MEMTRACK_REQUIRED_CAPS`]: the lowercase cap names
/// (libcap renders them lowercase) joined with commas and the `+ep`
/// effective+permitted flag. Derived from the enum so the two never drift.
fn memtrack_setcap_spec() -> String {
    let caps = MEMTRACK_REQUIRED_CAPS
        .iter()
        .map(|c| c.to_string().to_lowercase())
        .collect::<Vec<_>>()
        .join(",");
    format!("{caps}+ep")
}

/// The binary that must carry the eBPF capabilities.
///
/// Since memtrack is bundled, that binary is *this* one. Note what that means:
/// the five capabilities below, `CAP_SYS_ADMIN` among them, end up on the
/// `codspeed` executable itself rather than on a dedicated tracker, so every
/// invocation of the CLI carries them in its permitted and effective sets.
/// They are granted `+ep` and not inheritable, so a spawned benchmark does not
/// receive them — the elevation stops at the CLI process.
fn memtrack_path() -> Option<PathBuf> {
    self_exe().ok()
}

/// Whether the installed memtrack binary already carries the required capabilities.
pub fn has_memtrack_capabilities() -> bool {
    memtrack_path()
        .is_some_and(|path| binary_has_capabilities(&path, memtrack_required_caps_mask()))
}

/// Grant memtrack the capabilities it needs to run without sudo.
///
/// Best-effort and idempotent: a no-op when running as root or when the caps are
/// already present. Otherwise runs `setcap` (a single sudo prompt) and re-verifies.
/// Failures are surfaced as warnings rather than aborting, since the run-time
/// privilege guard enforces the requirement and reports it clearly.
pub fn ensure_memtrack_capabilities() -> Result<()> {
    if is_root_user() {
        debug!("Running as root, memtrack does not need file capabilities");
        return Ok(());
    }

    let Some(path) = memtrack_path() else {
        warn!("Could not locate {MEMTRACK_COMMAND} to grant capabilities");
        return Ok(());
    };

    if binary_has_capabilities(&path, memtrack_required_caps_mask()) {
        debug!("{MEMTRACK_COMMAND} already has the required capabilities");
        return Ok(());
    }

    info!(
        "Granting {MEMTRACK_COMMAND} the capabilities it needs as a one-time setup for the \
         memory instrument (requires sudo)."
    );
    let setcap_args = [memtrack_setcap_spec(), path.to_string_lossy().into_owned()];
    if let Err(e) = run_with_sudo("setcap", setcap_args) {
        warn!(
            "Failed to grant capabilities to {MEMTRACK_COMMAND} ({e}). \
             Memory profiling will require running as root."
        );
        return Ok(());
    }

    if !binary_has_capabilities(&path, memtrack_required_caps_mask()) {
        warn!(
            "Capabilities did not stick on {}. The filesystem may not support file \
             capabilities (e.g. nosuid, overlayfs, NFS). Memory profiling will require running as root.",
            path.display()
        );
    }

    Ok(())
}

pub fn get_memtrack_status() -> ToolStatus {
    // Bundled: there is nothing to look up on PATH and no version to compare,
    // because memtrack ships inside this binary and cannot be out of step with
    // it. What is still worth reporting is whether it can actually run, which
    // is a question about privileges, not about installation.
    ToolStatus {
        tool_name: MEMTRACK_COMMAND.to_string(),
        status: ToolInstallStatus::Installed {
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    }
}

/// Nothing to install any more: memtrack is part of this binary. Kept as a
/// no-op so the setup flow keeps its shape while the other tools still install.
pub async fn install_memtrack() -> Result<()> {
    debug!("{MEMTRACK_COMMAND} is bundled into this binary, nothing to install");
    Ok(())
}
