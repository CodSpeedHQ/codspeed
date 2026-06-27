use crate::binary_pins::PinnedBinary;
use crate::cli::run::helpers::download_pinned_file;
use crate::prelude::*;
use semver::Version;
use std::process::Command;
use tempfile::NamedTempFile;

mod versions;

/// Ensure a binary is installed, or install it from a `PinnedBinary` installer script.
///
/// This function checks if the binary is already installed with the correct version.
/// If not, it downloads and executes the pinned installer script.
///
/// # Arguments
/// * `binary_name` - The binary command name (e.g., "codspeed-memtrack", "codspeed-exec-harness")
/// * `version` - The version to install (e.g., "4.4.2-alpha.2")
/// * `installer` - The `PinnedBinary` installer to download.
pub async fn ensure_binary_installed(
    binary_name: &str,
    version: &str,
    installer: PinnedBinary,
) -> Result<()> {
    if is_command_installed(
        binary_name,
        Version::parse(version).context("Invalid version format")?,
    ) {
        debug!("{binary_name} version {version} is already installed");
        return Ok(());
    }

    debug!("Downloading installer for {binary_name}");

    // Download the installer script to a temporary file (with sha256 verification)
    let temp_file = NamedTempFile::new().context("Failed to create temporary file")?;
    download_pinned_file(installer, temp_file.path()).await?;

    // Execute the installer script
    let output = Command::new("sh")
        .arg(temp_file.path())
        .output()
        .context("Failed to execute installer command")?;

    if !output.status.success() {
        bail!(
            "Failed to install {binary_name} version {version}. Installer exited with output: {output:?}",
        );
    }

    if !is_command_installed(
        binary_name,
        Version::parse(version).context("Invalid version format")?,
    ) {
        bail!(
            "Could not veryfy installation of {binary_name} version {version} after running installer"
        );
    }

    info!("Successfully installed {binary_name} version {version}");
    Ok(())
}

/// Check if the given command is installed and its version matches the expected version.
///
/// Expects the command to support the `--version` flag and return a version string.
fn is_command_installed(command: &str, expected_version: Version) -> bool {
    let is_command_installed = Command::new("which")
        .arg(command)
        .output()
        .is_ok_and(|output| output.status.success());

    if !is_command_installed {
        debug!("{command} is not installed");
        return false;
    }

    let Ok(version_output) = Command::new(command).arg("--version").output() else {
        return false;
    };

    if !version_output.status.success() {
        debug!(
            "Failed to get command version. stderr: {}",
            String::from_utf8_lossy(&version_output.stderr)
        );
        return false;
    }

    let version_string = String::from_utf8_lossy(&version_output.stdout);
    let Ok(version) = versions::parse_from_output(&version_string) else {
        return false;
    };

    debug!("Found {command} version: {version}");

    versions::is_compatible(command, &version, &expected_version)
}
