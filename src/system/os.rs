use std::fmt::{self, Display};

use serde::{Deserialize, Serialize};
use sysinfo::System;

use crate::prelude::*;
/// Typed representation of the host operating system.
///
/// Only operating systems that CodSpeed can run on are represented here.
/// Construction via [`SupportedOs::from_current_system`] bails on unsupported platforms
#[derive(Eq, PartialEq, Hash, Debug, Clone, Serialize)]
#[serde(into = "SupportedOsSerde")]
pub enum SupportedOs {
    Linux(LinuxDistribution),
    Macos { version: String },
}

impl SupportedOs {
    /// Build a [`SupportedOs`] from the given OS family string.
    /// Expects `std::env::consts::OS` as input
    ///
    /// For Linux, the distribution is identified via `sysinfo::System::distribution_id()`.
    /// The OS version is read from `sysinfo::System::os_version()`.
    pub fn from_os(os: &str) -> Result<Self> {
        let os_version = System::os_version().ok_or(anyhow!("Failed to get OS version"))?;
        match os {
            "linux" => {
                let os_id = System::distribution_id();
                Ok(Self::Linux(LinuxDistribution::from_id(&os_id, &os_version)))
            }
            "macos" => Ok(Self::Macos {
                version: os_version,
            }),
            unsupported => bail!("Unsupported operating system: {unsupported}"),
        }
    }

    /// The distro/OS id as it appears on the wire (matches `sysinfo::System::distribution_id()`).
    pub fn id(&self) -> &str {
        match self {
            Self::Linux(distro) => distro.id(),
            Self::Macos { .. } => "macos",
        }
    }

    pub fn version(&self) -> &str {
        match self {
            Self::Linux(distro) => distro.version(),
            Self::Macos { version } => version,
        }
    }
}

impl Display for SupportedOs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.id(), self.version())
    }
}

/// Flat `{os, osVersion}` shape we emit on the wire as part of `SystemInfo`.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SupportedOsSerde {
    os: String,
    os_version: String,
}

impl From<SupportedOs> for SupportedOsSerde {
    fn from(os: SupportedOs) -> Self {
        SupportedOsSerde {
            os: os.id().to_string(),
            os_version: os.version().to_string(),
        }
    }
}

/// Linux distribution, identified by the `sysinfo` distribution id.
#[derive(Eq, PartialEq, Hash, Debug, Clone)]
pub enum LinuxDistribution {
    Ubuntu { version: String },
    Debian { version: String },
    Other { name: String, version: String },
}

impl LinuxDistribution {
    /// Build a [`LinuxDistribution`] from the raw `(os_id, version)` strings reported by `sysinfo`.
    fn from_id(os_id: &str, version: &str) -> Self {
        match os_id {
            "ubuntu" => Self::Ubuntu {
                version: version.to_string(),
            },
            "debian" => Self::Debian {
                version: version.to_string(),
            },
            _ => Self::Other {
                name: os_id.to_string(),
                version: version.to_string(),
            },
        }
    }

    /// The distro id as it appears on the wire (matches `sysinfo::System::distribution_id()`).
    pub fn id(&self) -> &str {
        match self {
            Self::Ubuntu { .. } => "ubuntu",
            Self::Debian { .. } => "debian",
            Self::Other { name, .. } => name,
        }
    }

    pub fn version(&self) -> &str {
        match self {
            Self::Ubuntu { version } | Self::Debian { version } | Self::Other { version, .. } => {
                version
            }
        }
    }

    /// Whether this distribution has first-class support (auto-install via apt, prebuilt .debs, etc.).
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::Ubuntu { .. } | Self::Debian { .. })
    }
}

impl Display for LinuxDistribution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.id(), self.version())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_os_bails_on_unsupported() {
        let err = SupportedOs::from_os("windows").unwrap_err();
        assert_eq!(err.to_string(), "Unsupported operating system: windows");
    }
}
