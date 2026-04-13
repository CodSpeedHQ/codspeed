use crate::binary_installer::ensure_binary_installed;
use crate::executor::{ToolInstallStatus, ToolStatus};
use crate::prelude::*;
use std::process::Command;

pub const MEMTRACK_COMMAND: &str = "codspeed-memtrack";
pub const MEMTRACK_CODSPEED_VERSION: &str = "1.2.3";

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
    let get_memtrack_installer_url = || {
        format!(
            "https://github.com/CodSpeedHQ/codspeed/releases/download/memtrack-v{MEMTRACK_CODSPEED_VERSION}/memtrack-installer.sh"
        )
    };

    ensure_binary_installed(
        MEMTRACK_COMMAND,
        MEMTRACK_CODSPEED_VERSION,
        get_memtrack_installer_url,
    )
    .await
}
