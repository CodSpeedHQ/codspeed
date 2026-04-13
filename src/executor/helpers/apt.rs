use super::run_with_sudo::run_with_sudo;
use crate::prelude::*;
use crate::system::{SupportedOs, SystemInfo};
use std::path::Path;
use std::process::Command;

const METADATA_FILENAME: &str = "./tmp/codspeed-cache-metadata.txt";

fn is_system_compatible(system_info: &SystemInfo) -> bool {
    matches!(system_info.os, SupportedOs::Linux(ref distro) if distro.is_supported())
}

/// Installs packages with caching support.
///
/// This function provides a common pattern for installing tools on Ubuntu/Debian systems
/// with automatic caching to speed up subsequent installations (e.g., in CI environments).
///
/// # Arguments
///
/// * `system_info` - System information to determine compatibility
/// * `setup_cache_dir` - Optional directory to restore from/save to cache
/// * `is_installed` - Function that checks if the tool is already installed
/// * `install_packages` - Async closure that:
///   1. Performs the installation (e.g., downloads .deb files, calls `apt::install`)
///   2. Returns a Vec of package names that should be cached via `dpkg -L`
///
/// # Flow
///
/// 1. Check if already installed - if yes, skip everything
/// 2. Try to restore from cache (if cache_dir provided)
/// 3. Check again if installed - if yes, we're done
/// 4. Run the install closure to install and get package names
/// 5. Save installed packages to cache (if cache_dir provided)
///
/// # Example
///
/// ```rust,ignore
/// apt::install_cached(
///     system_info,
///     setup_cache_dir,
///     || Command::new("which").arg("perf").status().is_ok(),
///     || async {
///         let packages = vec!["linux-tools-common".to_string()];
///         let refs: Vec<&str> = packages.iter().map(|s| s.as_str()).collect();
///         apt::install(system_info, &refs)?;
///         Ok(packages) // Return package names for caching
///     },
/// ).await?;
/// ```
pub async fn install_cached<F, I, Fut>(
    system_info: &SystemInfo,
    setup_cache_dir: Option<&Path>,
    is_installed: F,
    install_packages: I,
) -> Result<()>
where
    F: Fn() -> bool,
    I: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<String>>>,
{
    if is_installed() {
        debug!("Tool already installed, skipping installation");
        return Ok(());
    }

    // Try to restore from cache first
    if let Some(cache_dir) = setup_cache_dir {
        restore_from_cache(system_info, cache_dir)?;

        if is_installed() {
            info!("Tool has been successfully restored from cache");
            return Ok(());
        }
    }

    // Install and get the package names for caching
    let cache_packages = install_packages().await?;

    info!("Installation completed successfully");

    // Save to cache after successful installation
    if let Some(cache_dir) = setup_cache_dir {
        let cache_refs: Vec<&str> = cache_packages.iter().map(|s| s.as_str()).collect();
        save_to_cache(system_info, cache_dir, &cache_refs)?;
    }

    Ok(())
}

pub fn install(system_info: &SystemInfo, packages: &[&str]) -> Result<()> {
    if !is_system_compatible(system_info) {
        bail!(
            "Package installation is not supported on this system, please install necessary packages manually"
        );
    }

    info!("Installing packages: {}", packages.join(", "));

    run_with_sudo("apt-get", ["update"])?;
    let mut install_argv = vec!["install", "-y", "--allow-downgrades"];
    install_argv.extend_from_slice(packages);
    run_with_sudo("apt-get", &install_argv)?;

    debug!("Packages installed successfully");
    Ok(())
}

/// Restore cached tools from the cache directory to the root filesystem
fn restore_from_cache(system_info: &SystemInfo, cache_dir: &Path) -> Result<()> {
    if !is_system_compatible(system_info) {
        info!("Cache restore is not supported on this system, skipping");
        return Ok(());
    }

    if !cache_dir.exists() {
        debug!("Cache directory does not exist: {}", cache_dir.display());
        return Ok(());
    }

    // Check if the directory has any contents
    let has_contents = std::fs::read_dir(cache_dir)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);

    if !has_contents {
        debug!("Cache directory is empty: {}", cache_dir.display());
        return Ok(());
    }

    debug!(
        "Restoring tools from cache directory: {}",
        cache_dir.display()
    );

    // Read and log the metadata file if it exists
    let metadata_path = cache_dir.join(METADATA_FILENAME);
    if metadata_path.exists() {
        match std::fs::read_to_string(&metadata_path) {
            Ok(content) => {
                info!(
                    "Packages restored from cache: {}",
                    content.lines().join(", ")
                );
            }
            Err(e) => {
                warn!("Failed to read metadata file: {e}");
            }
        }
    } else {
        debug!("No metadata file found in cache directory");
    }

    // Use bash to properly handle glob expansion
    let cache_dir_str = cache_dir
        .to_str()
        .ok_or_else(|| anyhow!("Invalid cache directory path"))?;

    // IMPORTANT: We have to use 'bash' here to ensure that glob patterns are expanded correctly
    let copy_cmd = format!("cp -r {cache_dir_str}/* /");
    run_with_sudo("bash", ["-c", &copy_cmd])?;

    debug!("Cache restored successfully");
    Ok(())
}

/// Save installed packages to the cache directory
fn save_to_cache(system_info: &SystemInfo, cache_dir: &Path, packages: &[&str]) -> Result<()> {
    if !is_system_compatible(system_info) {
        info!("Caching of installed package is not supported on this system, skipping");
        return Ok(());
    }

    debug!(
        "Saving installed packages to cache: {}",
        cache_dir.display()
    );

    // Create cache directory if it doesn't exist
    std::fs::create_dir_all(cache_dir).context("Failed to create cache directory")?;

    let cache_dir_str = cache_dir
        .to_str()
        .ok_or_else(|| anyhow!("Invalid cache directory path"))?;

    // Logic taken from https://stackoverflow.com/a/59277514
    // This shell command lists all the files outputted by the given packages and copy them to the cache directory
    let packages_str = packages.join(" ");
    let shell_cmd = format!(
        "dpkg -L {packages_str} | while IFS= read -r f; do if test -f \"$f\"; then echo \"$f\"; fi; done | xargs cp --parents --target-directory {cache_dir_str}",
    );

    debug!("Running cache save command: {shell_cmd}");

    let output = Command::new("sh")
        .arg("-c")
        .arg(&shell_cmd)
        .output()
        .context("Failed to execute cache save command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!("stderr: {stderr}");
        bail!("Failed to save packages to cache");
    }

    // Create metadata file containing the installed packages
    let metadata_path = cache_dir.join(METADATA_FILENAME);
    let metadata_content = packages.join("\n"); // TODO: add package versions as well, by using the output of the install command for example
    if let Ok(()) = std::fs::create_dir_all(metadata_path.parent().unwrap()) {
        if let Ok(()) = std::fs::write(&metadata_path, metadata_content)
            .context("Failed to write metadata file")
        {
            debug!("Metadata file created at: {}", metadata_path.display());
        } else {
            warn!(
                "Failed to create metadata file at: {}",
                metadata_path.display()
            );
        }
    } else {
        warn!(
            "Failed to create metadata file parent directory for: {}",
            metadata_path.display()
        );
    }

    debug!("Packages cached successfully");
    Ok(())
}
