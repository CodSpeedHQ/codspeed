use std::{
    fmt::{self, Display},
    fs,
};

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
    /// The OS version is read from `sysinfo::System::os_version()`, falling back to
    /// `VERSION_ID` or `BUILD_ID` from os-release.
    pub fn from_os(os: &str) -> Result<Self> {
        match os {
            "linux" => {
                let os_id = System::distribution_id();
                let os_version = linux_os_version()?;
                Ok(Self::Linux(LinuxDistribution::from_id(&os_id, &os_version)))
            }
            "macos" => {
                let os_version = System::os_version().ok_or(anyhow!("Failed to get OS version"))?;
                Ok(Self::Macos {
                    version: os_version,
                })
            }
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

fn linux_os_version() -> Result<String> {
    System::os_version()
        .or_else(read_linux_os_release_version)
        .ok_or(anyhow!(
            "Failed to get Linux OS version from sysinfo or os-release"
        ))
}

fn read_linux_os_release_version() -> Option<String> {
    ["/etc/os-release", "/usr/lib/os-release"]
        .into_iter()
        .find_map(|path| {
            fs::read_to_string(path)
                .ok()
                .and_then(|contents| parse_os_release_version(&contents))
        })
}

fn parse_os_release_version(contents: &str) -> Option<String> {
    os_release_value(contents, "VERSION_ID").or_else(|| os_release_value(contents, "BUILD_ID"))
}

fn os_release_value(contents: &str, key: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }

        let (candidate, value) = line.split_once('=')?;
        if candidate.trim() != key {
            return None;
        }

        Some(unquote_os_release_value(value.trim()).to_string())
    })
}

fn unquote_os_release_value(value: &str) -> &str {
    if let Some(value) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    {
        return value;
    }

    if let Some(value) = value
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
    {
        return value;
    }

    value
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

    #[test]
    fn parse_os_release_version_prefers_version_id() {
        let contents = r#"
            ID=ubuntu
            VERSION_ID="24.04"
            BUILD_ID=rolling
        "#;

        assert_eq!(parse_os_release_version(contents).as_deref(), Some("24.04"));
    }

    #[test]
    fn parse_os_release_version_falls_back_to_build_id() {
        let contents = r#"
            NAME="Arch Linux"
            ID=arch
            BUILD_ID=rolling
        "#;

        assert_eq!(
            parse_os_release_version(contents).as_deref(),
            Some("rolling")
        );
    }

    #[test]
    fn parse_os_release_version_handles_single_quoted_values() {
        let contents = "ID=example\nVERSION_ID='1.2'\n";

        assert_eq!(parse_os_release_version(contents).as_deref(), Some("1.2"));
    }

    #[test]
    fn parse_os_release_version_returns_none_without_version_fields() {
        let contents = r#"
            NAME="Unknown Linux"
            ID=unknown
        "#;

        assert_eq!(parse_os_release_version(contents), None);
    }
}
