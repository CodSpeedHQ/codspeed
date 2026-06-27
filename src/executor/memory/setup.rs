use crate::binary_installer::ensure_binary_installed;
use crate::binary_pins::{self, PinnedBinary};
use crate::executor::helpers::capabilities::binary_has_capabilities;
use crate::executor::helpers::run_with_sudo::{is_root_user, run_with_sudo};
use crate::executor::{ToolInstallStatus, ToolStatus};
use crate::prelude::*;
use caps::Capability;
use std::path::PathBuf;
use std::process::Command;

pub const MEMTRACK_COMMAND: &str = "codspeed-memtrack";
pub const MEMTRACK_CODSPEED_VERSION: &str = binary_pins::MEMTRACK_VERSION;

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

fn memtrack_path() -> Option<PathBuf> {
    which::which(MEMTRACK_COMMAND).ok()
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
    let tool_name = MEMTRACK_COMMAND.to_string();

    let is_available = Command::new("which")
        .arg(MEMTRACK_COMMAND)
        .output()
        .is_ok_and(|output| output.status.success());
    if !is_available {
        return ToolStatus {
            tool_name,
            status: ToolInstallStatus::NotInstalled,
        };
    }

    let Ok(version_output) = Command::new(MEMTRACK_COMMAND).arg("--version").output() else {
        return ToolStatus {
            tool_name,
            status: ToolInstallStatus::NotInstalled,
        };
    };

    if !version_output.status.success() {
        return ToolStatus {
            tool_name,
            status: ToolInstallStatus::NotInstalled,
        };
    }

    let version = String::from_utf8_lossy(&version_output.stdout)
        .trim()
        .to_string();

    // Parse the version number from output like "memtrack 1.2.2"
    let expected = semver::Version::parse(MEMTRACK_CODSPEED_VERSION).unwrap();
    if let Some(version_str) = version.split_once(' ').map(|(_, v)| v.trim()) {
        if let Ok(installed) = semver::Version::parse(version_str) {
            if installed < expected {
                return ToolStatus {
                    tool_name,
                    status: ToolInstallStatus::IncorrectVersion {
                        version,
                        message: format!(
                            "version too old, expecting {MEMTRACK_CODSPEED_VERSION} or higher",
                        ),
                    },
                };
            }
            return ToolStatus {
                tool_name,
                status: ToolInstallStatus::Installed { version },
            };
        }
    }

    ToolStatus {
        tool_name,
        status: ToolInstallStatus::IncorrectVersion {
            version,
            message: "could not parse version".to_string(),
        },
    }
}

pub async fn install_memtrack() -> Result<()> {
    ensure_binary_installed(
        MEMTRACK_COMMAND,
        MEMTRACK_CODSPEED_VERSION,
        PinnedBinary::MemtrackInstaller,
    )
    .await
}
