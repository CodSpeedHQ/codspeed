use std::fmt::{self, Display};

use serde::{Deserialize, Serialize};
use sysinfo::System;

use crate::prelude::*;

/// Version reported on the wire for distributions that expose none.
const UNKNOWN_OS_VERSION: &str = "unknown";

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
        match os {
            "linux" => {
                let os_id = System::distribution_id();
                // Rolling releases do not expose a `VERSION_ID` in `/etc/os-release`.
                let os_version = System::os_version();
                Ok(Self::Linux(LinuxDistribution::from_id(&os_id, os_version)))
            }
            "macos" => Ok(Self::Macos {
                version: System::os_version().ok_or(anyhow!("Failed to get OS version"))?,
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

    /// The OS version, absent on the distributions that report none.
    pub fn version(&self) -> Option<&str> {
        match self {
            Self::Linux(distro) => distro.version(),
            Self::Macos { version } => Some(version),
        }
    }
}

impl Display for SupportedOs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.version() {
            Some(version) => write!(f, "{} {version}", self.id()),
            None => write!(f, "{}", self.id()),
        }
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
            os_version: os.version().unwrap_or(UNKNOWN_OS_VERSION).to_string(),
        }
    }
}

/// Linux distribution, identified by the `sysinfo` distribution id.
#[derive(Eq, PartialEq, Hash, Debug, Clone)]
pub enum LinuxDistribution {
    Ubuntu {
        version: String,
    },
    Debian {
        version: String,
    },
    Other {
        name: String,
        /// Absent on rolling releases, which expose no `VERSION_ID`.
        version: Option<String>,
    },
}

impl LinuxDistribution {
    /// Build a [`LinuxDistribution`] from the raw `(os_id, version)` reported by `sysinfo`.
    ///
    /// The distributions we ship packages for all report a version, so one reporting none
    /// is by construction not one of them.
    fn from_id(os_id: &str, version: Option<String>) -> Self {
        match (os_id, version) {
            ("ubuntu", Some(version)) => Self::Ubuntu { version },
            ("debian", Some(version)) => Self::Debian { version },
            (name, version) => Self::Other {
                name: name.to_string(),
                version,
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

    /// The distribution version, absent on the ones that report none.
    pub fn version(&self) -> Option<&str> {
        match self {
            Self::Ubuntu { version } | Self::Debian { version } => Some(version),
            Self::Other { version, .. } => version.as_deref(),
        }
    }

    /// Whether this distribution has first-class support (auto-install via apt, prebuilt .debs, etc.).
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::Ubuntu { .. } | Self::Debian { .. })
    }
}

impl Display for LinuxDistribution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.version() {
            Some(version) => write!(f, "{} {version}", self.id()),
            None => write!(f, "{}", self.id()),
        }
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

    #[test]
    fn distribution_without_version_id_is_not_supported() {
        let distro = LinuxDistribution::from_id("arch", None);
        assert_eq!(distro.version(), None);
        assert_eq!(distro.to_string(), "arch");
        assert!(!distro.is_supported());
    }
}
