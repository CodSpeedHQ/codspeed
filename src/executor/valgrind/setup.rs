use crate::binary_pins::{
    Arch, DistroVersion, PinnedBinary, VALGRIND_CODSPEED_ITERATION, VALGRIND_CODSPEED_VERSION,
    VALGRIND_CODSPEED_VERSION_STRING, ValgrindTarget,
};
use crate::cli::run::helpers::download_pinned_file;
use crate::executor::helpers::apt;
use crate::executor::{ToolInstallStatus, ToolStatus};
use crate::prelude::*;
use crate::system::{LinuxDistribution, SupportedOs, SystemInfo};
use semver::Version;
use std::{env, path::Path, process::Command};

fn get_codspeed_valgrind_target(system_info: &SystemInfo) -> Result<ValgrindTarget> {
    let SupportedOs::Linux(distro) = &system_info.os else {
        bail!("Unsupported system");
    };

    let (distro_version, arch) = match (distro, system_info.arch.as_str()) {
        (LinuxDistribution::Ubuntu { version }, "x86_64")
        | (LinuxDistribution::Debian { version }, "x86_64")
            if version == "22.04" || version == "12" =>
        {
            (DistroVersion::Ubuntu2204, Arch::Amd64)
        }
        (LinuxDistribution::Ubuntu { version }, "x86_64") if version == "24.04" => {
            (DistroVersion::Ubuntu2404, Arch::Amd64)
        }
        (LinuxDistribution::Ubuntu { version }, "aarch64")
        | (LinuxDistribution::Debian { version }, "aarch64")
            if version == "22.04" || version == "12" =>
        {
            (DistroVersion::Ubuntu2204, Arch::Arm64)
        }
        (LinuxDistribution::Ubuntu { version }, "aarch64") if version == "24.04" => {
            (DistroVersion::Ubuntu2404, Arch::Arm64)
        }
        _ => bail!("Unsupported system"),
    };

    Ok(ValgrindTarget {
        distro_version,
        arch,
    })
}

fn get_codspeed_valgrind_binary(system_info: &SystemInfo) -> Result<PinnedBinary> {
    Ok(PinnedBinary::ValgrindDeb(get_codspeed_valgrind_target(
        system_info,
    )?))
}

pub(super) fn is_codspeed_valgrind_installation_supported(system_info: &SystemInfo) -> bool {
    get_codspeed_valgrind_target(system_info).is_ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ValgrindVersion {
    semver: Version,
    codspeed_iteration: Option<u32>,
}

/// Parse a valgrind version string and extract the upstream semver plus the
/// optional CodSpeed iteration number.
///
/// Accepted formats:
/// - `valgrind-3.25.1.codspeed`  (legacy, no iteration)
/// - `valgrind-3.25.1.codspeed2` (with iteration)
/// - same forms without the `valgrind-` prefix
fn parse_valgrind_codspeed_version(version_str: &str) -> Option<ValgrindVersion> {
    let stripped = version_str
        .trim()
        .strip_prefix("valgrind-")
        .unwrap_or(version_str.trim());

    let (semver_part, iteration_part) = stripped.split_once(".codspeed")?;
    let semver = Version::parse(semver_part).ok()?;

    let codspeed_iteration = if iteration_part.is_empty() {
        None
    } else {
        Some(iteration_part.parse::<u32>().ok()?)
    };

    Some(ValgrindVersion {
        semver,
        codspeed_iteration,
    })
}

const TOOL_NAME: &str = "valgrind";

pub fn get_valgrind_status() -> ToolStatus {
    let tool_name = TOOL_NAME.to_string();

    let is_available = Command::new("which")
        .arg("valgrind")
        .output()
        .is_ok_and(|output| output.status.success());
    if !is_available {
        debug!("valgrind is not installed");
        return ToolStatus {
            tool_name,
            status: ToolInstallStatus::NotInstalled,
        };
    }

    let Ok(version_output) = Command::new("valgrind").arg("--version").output() else {
        return ToolStatus {
            tool_name,
            status: ToolInstallStatus::NotInstalled,
        };
    };

    if !version_output.status.success() {
        debug!(
            "Failed to get valgrind version. stderr: {}",
            String::from_utf8_lossy(&version_output.stderr)
        );
        return ToolStatus {
            tool_name,
            status: ToolInstallStatus::NotInstalled,
        };
    }

    let version = String::from_utf8_lossy(&version_output.stdout)
        .trim()
        .to_string();
    debug!(
        "Found installed valgrind version: {version} (expecting {} or higher)",
        VALGRIND_CODSPEED_VERSION_STRING.as_str()
    );

    ToolStatus {
        tool_name,
        status: classify_valgrind_version(version),
    }
}

/// Classify a trimmed `valgrind --version` output against the pinned CodSpeed
/// valgrind version.
fn classify_valgrind_version(version: String) -> ToolInstallStatus {
    // Check if it's a codspeed version
    if !version.contains(".codspeed") {
        return ToolInstallStatus::IncorrectVersion {
            version,
            message: "not a CodSpeed build".to_string(),
        };
    }

    // Parse the installed version
    let Some(installed_version) = parse_valgrind_codspeed_version(&version) else {
        return ToolInstallStatus::IncorrectVersion {
            version,
            message: "could not parse version".to_string(),
        };
    };

    // Legacy `.codspeed` builds (no iteration suffix) predate iteration
    // tracking, so they count as iteration 0.
    let installed_iteration = installed_version.codspeed_iteration.unwrap_or(0);
    let is_version_outdated = (installed_version.semver, installed_iteration)
        < (VALGRIND_CODSPEED_VERSION, VALGRIND_CODSPEED_ITERATION);
    if is_version_outdated {
        return ToolInstallStatus::IncorrectVersion {
            version,
            message: format!(
                "version too old, expecting {} or higher",
                VALGRIND_CODSPEED_VERSION_STRING.as_str()
            ),
        };
    }

    ToolInstallStatus::Installed { version }
}

fn is_valgrind_installed() -> bool {
    matches!(
        get_valgrind_status().status,
        ToolInstallStatus::Installed { .. }
    )
}

pub async fn install_valgrind(
    system_info: &SystemInfo,
    setup_cache_dir: Option<&Path>,
) -> Result<()> {
    apt::install_cached(
        system_info,
        setup_cache_dir,
        is_valgrind_installed,
        || async {
            debug!("Installing valgrind");
            let binary = get_codspeed_valgrind_binary(system_info)?;
            let deb_path = env::temp_dir().join("valgrind-codspeed.deb");
            download_pinned_file(binary, &deb_path).await?;
            apt::install(system_info, &[deb_path.to_str().unwrap()])?;

            // Return package names for caching
            Ok(vec!["valgrind".to_string()])
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::*;

    #[test]
    fn test_system_info_to_codspeed_valgrind_url_ubuntu() {
        let system_info = SystemInfo {
            os: SupportedOs::Linux(LinuxDistribution::Ubuntu {
                version: "22.04".into(),
            }),
            arch: "x86_64".to_string(),
            ..SystemInfo::test()
        };
        assert_snapshot!(
            get_codspeed_valgrind_binary(&system_info).unwrap().url(),
            @"https://github.com/CodSpeedHQ/valgrind-codspeed/releases/download/3.26.0-0codspeed3/valgrind_3.26.0-0codspeed3_ubuntu-22.04_amd64.deb"
        );
    }

    #[test]
    fn test_system_info_to_codspeed_valgrind_url_ubuntu_24() {
        let system_info = SystemInfo {
            os: SupportedOs::Linux(LinuxDistribution::Ubuntu {
                version: "24.04".into(),
            }),
            arch: "x86_64".to_string(),
            ..SystemInfo::test()
        };
        assert_snapshot!(
            get_codspeed_valgrind_binary(&system_info).unwrap().url(),
            @"https://github.com/CodSpeedHQ/valgrind-codspeed/releases/download/3.26.0-0codspeed3/valgrind_3.26.0-0codspeed3_ubuntu-24.04_amd64.deb"
        );
    }

    #[test]
    fn test_system_info_to_codspeed_valgrind_url_debian() {
        let system_info = SystemInfo {
            os: SupportedOs::Linux(LinuxDistribution::Debian {
                version: "12".into(),
            }),
            arch: "x86_64".to_string(),
            ..SystemInfo::test()
        };
        assert_snapshot!(
            get_codspeed_valgrind_binary(&system_info).unwrap().url(),
            @"https://github.com/CodSpeedHQ/valgrind-codspeed/releases/download/3.26.0-0codspeed3/valgrind_3.26.0-0codspeed3_ubuntu-22.04_amd64.deb"
        );
    }

    #[test]
    fn test_system_info_to_codspeed_valgrind_url_ubuntu_arm() {
        let system_info = SystemInfo {
            os: SupportedOs::Linux(LinuxDistribution::Ubuntu {
                version: "22.04".into(),
            }),
            arch: "aarch64".to_string(),
            ..SystemInfo::test()
        };
        assert_snapshot!(
            get_codspeed_valgrind_binary(&system_info).unwrap().url(),
            @"https://github.com/CodSpeedHQ/valgrind-codspeed/releases/download/3.26.0-0codspeed3/valgrind_3.26.0-0codspeed3_ubuntu-22.04_arm64.deb"
        );
    }

    #[test]
    fn test_codspeed_valgrind_unsupported_os() {
        let system_info = SystemInfo {
            os: SupportedOs::Macos {
                version: "14.0".into(),
            },
            ..SystemInfo::test()
        };
        assert!(get_codspeed_valgrind_binary(&system_info).is_err());
    }

    #[test]
    fn test_codspeed_valgrind_unsupported_distro() {
        let system_info = SystemInfo {
            os: SupportedOs::Linux(LinuxDistribution::Ubuntu {
                version: "20.04".into(),
            }),
            ..SystemInfo::test()
        };
        assert!(get_codspeed_valgrind_binary(&system_info).is_err());
    }

    #[test]
    fn test_parse_valgrind_codspeed_version_with_prefix() {
        let version = parse_valgrind_codspeed_version("valgrind-3.25.1.codspeed").unwrap();
        assert_eq!(version.semver, Version::new(3, 25, 1));
        assert_eq!(version.codspeed_iteration, None);
    }

    #[test]
    fn test_parse_valgrind_codspeed_version_without_prefix() {
        let version = parse_valgrind_codspeed_version("3.25.1.codspeed").unwrap();
        assert_eq!(version.semver, Version::new(3, 25, 1));
        assert_eq!(version.codspeed_iteration, None);
    }

    #[test]
    fn test_parse_valgrind_codspeed_version_higher_patch() {
        let version = parse_valgrind_codspeed_version("valgrind-3.25.2.codspeed").unwrap();
        assert_eq!(version.semver, Version::new(3, 25, 2));
    }

    #[test]
    fn test_parse_valgrind_codspeed_version_with_newline() {
        let version = parse_valgrind_codspeed_version("valgrind-3.25.1.codspeed\n").unwrap();
        assert_eq!(version.semver, Version::new(3, 25, 1));
    }

    #[test]
    fn test_parse_valgrind_codspeed_version_without_codspeed_suffix() {
        assert_eq!(parse_valgrind_codspeed_version("valgrind-3.25.1"), None);
    }

    #[test]
    fn test_parse_valgrind_codspeed_version_iterations_are_differentiated() {
        let legacy = parse_valgrind_codspeed_version("valgrind-3.26.0.codspeed").unwrap();
        let v1 = parse_valgrind_codspeed_version("valgrind-3.26.0.codspeed1").unwrap();
        let v2 = parse_valgrind_codspeed_version("valgrind-3.26.0.codspeed2").unwrap();

        assert_eq!(legacy.semver, v1.semver);
        assert_eq!(legacy.codspeed_iteration, None);
        assert_eq!(v1.codspeed_iteration, Some(1));
        assert_eq!(v2.codspeed_iteration, Some(2));

        assert_ne!(legacy, v1);
        assert_ne!(v1, v2);
    }

    #[test]
    fn test_classify_same_version_older_iteration_is_rejected() {
        // Pinned is 3.26.0-0codspeed3: a cached 3.26.0.codspeed2 build is the
        // same upstream valgrind but an older repackaging, so it must be
        // rejected and reinstalled.
        let status = classify_valgrind_version("valgrind-3.26.0.codspeed2".to_string());
        assert!(
            matches!(status, ToolInstallStatus::IncorrectVersion { .. }),
            "stale codspeed iteration must be rejected, got: Installed"
        );
    }

    #[test]
    fn test_classify_legacy_suffix_with_pinned_semver_is_rejected() {
        // Old builds report just `.codspeed` (no iteration); for the pinned
        // upstream version they predate the pinned iteration.
        let version = format!("valgrind-{VALGRIND_CODSPEED_VERSION}.codspeed");
        let status = classify_valgrind_version(version);
        assert!(matches!(status, ToolInstallStatus::IncorrectVersion { .. }));
    }

    #[test]
    fn test_classify_pinned_iteration_is_installed() {
        let version =
            format!("valgrind-{VALGRIND_CODSPEED_VERSION}.codspeed{VALGRIND_CODSPEED_ITERATION}");
        let status = classify_valgrind_version(version);
        assert!(matches!(status, ToolInstallStatus::Installed { .. }));
    }

    #[test]
    fn test_classify_newer_iteration_is_installed() {
        let version = format!(
            "valgrind-{VALGRIND_CODSPEED_VERSION}.codspeed{}",
            VALGRIND_CODSPEED_ITERATION + 1
        );
        let status = classify_valgrind_version(version);
        assert!(matches!(status, ToolInstallStatus::Installed { .. }));
    }

    #[test]
    fn test_classify_newer_semver_with_legacy_suffix_is_installed() {
        let status = classify_valgrind_version("valgrind-3.99.0.codspeed".to_string());
        assert!(matches!(status, ToolInstallStatus::Installed { .. }));
    }

    #[test]
    fn test_classify_non_codspeed_build_is_rejected() {
        let status = classify_valgrind_version("valgrind-3.26.0".to_string());
        assert!(matches!(status, ToolInstallStatus::IncorrectVersion { .. }));
    }

    #[test]
    fn test_parse_valgrind_codspeed_version_invalid_format() {
        assert_eq!(
            parse_valgrind_codspeed_version("valgrind-3.25.codspeed"),
            None
        );
    }
}
