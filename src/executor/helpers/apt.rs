use super::run_with_sudo::run_with_sudo;
use crate::prelude::*;
use crate::system::{SupportedOs, SystemInfo};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

const METADATA_FILENAME: &str = "./tmp/codspeed-cache-metadata.txt";

pub fn is_system_compatible(system_info: &SystemInfo) -> bool {
    matches!(system_info.os, SupportedOs::Linux(ref distro) if distro.is_supported())
}

/// The packages a cache entry holds, with the version each of them had when it was saved.
///
/// The cached tree is a plain file copy: nothing is recorded in the dpkg database when it is
/// applied, so `dpkg` cannot answer whether a cached package is present, let alone which
/// version. The manifest is the only description of the entry's contents available before
/// applying it to the filesystem.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CacheManifest {
    packages: BTreeMap<String, String>,
}

impl CacheManifest {
    fn of_installed_packages(packages: &[&str]) -> Self {
        let packages = packages
            .iter()
            .filter_map(|package| {
                let version = installed_package_version(package)?;
                Some((package.to_string(), version))
            })
            .collect();

        Self { packages }
    }

    fn parse(content: &str) -> Self {
        let packages = content
            .lines()
            .filter_map(|line| line.trim().split_once('='))
            .map(|(package, version)| (package.to_string(), version.to_string()))
            .collect();

        Self { packages }
    }

    /// Take in the packages of `other`, which win on conflict. A cache directory holds the files
    /// of every tool installed into it, so its manifest has to describe all of them.
    fn merge(&mut self, other: Self) {
        self.packages.extend(other.packages);
    }

    fn serialize(&self) -> String {
        self.packages
            .iter()
            .map(|(package, version)| format!("{package}={version}"))
            .join("\n")
    }

    pub fn version_of(&self, package: &str) -> Option<&str> {
        self.packages.get(package).map(String::as_str)
    }

    pub fn package_names(&self) -> impl Iterator<Item = &str> {
        self.packages.keys().map(String::as_str)
    }

    /// Whether every cached package that dpkg also knows about is at the version it had when the
    /// entry was saved, so that files coming from the cache cannot contradict the system's own
    /// packages.
    pub fn matches_installed_packages(&self) -> bool {
        self.packages.iter().all(|(package, cached_version)| {
            match installed_package_version(package) {
                Some(installed_version) if &installed_version != cached_version => {
                    debug!(
                        "Cached {package} {cached_version} does not match installed {installed_version}"
                    );
                    false
                }
                _ => true,
            }
        })
    }
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
/// * `is_installed` - Whether the tool is installed according to the system's package state.
///   Only consulted before the cache is applied, since a cached tree leaves no trace in the
///   dpkg database
/// * `is_cache_usable` - Whether a cache entry can be applied to the running system, given the
///   packages and versions it was saved with. Evaluated before any file is copied
/// * `is_functional` - Whether the tool works once the cache has been applied. Must not depend
///   on the package state, only on what the cached files provide
/// * `install_packages` - Async closure that:
///   1. Performs the installation (e.g., downloads .deb files, calls `apt::install`)
///   2. Returns a Vec of package names that should be cached via `dpkg -L`
///
/// # Flow
///
/// 1. Check if already installed - if yes, skip everything
/// 2. Read the cache manifest and, if the entry is usable, apply it and check that the tool is
///    functional - if it is, we're done
/// 3. Run the install closure to install and get package names
/// 4. Save installed packages to cache (if cache_dir provided)
///
/// # Example
///
/// ```rust,ignore
/// apt::install_cached(
///     system_info,
///     setup_cache_dir,
///     || Command::new("which").arg("perf").status().is_ok(),
///     |manifest| manifest.matches_installed_packages(),
///     || Command::new("which").arg("perf").status().is_ok(),
///     || async {
///         let packages = vec!["linux-tools-common".to_string()];
///         let refs: Vec<&str> = packages.iter().map(|s| s.as_str()).collect();
///         apt::install(system_info, &refs)?;
///         Ok(packages) // Return package names for caching
///     },
/// ).await?;
/// ```
pub async fn install_cached<F, C, R, I, Fut>(
    system_info: &SystemInfo,
    setup_cache_dir: Option<&Path>,
    is_installed: F,
    is_cache_usable: C,
    is_functional: R,
    install_packages: I,
) -> Result<()>
where
    F: Fn() -> bool,
    C: Fn(&CacheManifest) -> bool,
    R: Fn() -> bool,
    I: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<String>>>,
{
    if is_installed() {
        debug!("Tool already installed, skipping installation");
        return Ok(());
    }

    // Try to restore from cache first
    if let Some(cache_dir) = setup_cache_dir
        && let Some(manifest) = read_manifest(system_info, cache_dir)
    {
        if is_cache_usable(&manifest) {
            info!(
                "Packages restored from cache: {}",
                manifest.package_names().join(", ")
            );
            restore_from_cache(system_info, cache_dir)?;

            if is_functional() {
                info!("Tool has been successfully restored from cache");
                return Ok(());
            }

            warn!("Tool is not functional after being restored from cache, installing it instead");
        } else {
            info!("Cached packages do not apply to this system, installing instead");
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

/// Returns whether a package is currently installed according to `dpkg`.
pub fn is_package_installed(package: &str) -> bool {
    Command::new("dpkg")
        .args(["-s", package])
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Returns the version `dpkg` reports for a package, or `None` when it is not installed.
pub fn installed_package_version(package: &str) -> Option<String> {
    let output = Command::new("dpkg-query")
        .args(["-W", "-f=${db:Status-Status} ${Version}", package])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let (status, version) = stdout.trim().split_once(' ')?;
    if status != "installed" {
        return None;
    }

    Some(version.to_string())
}

/// Read the manifest of the cache entry sitting in `cache_dir`, if there is one to read.
fn read_manifest(system_info: &SystemInfo, cache_dir: &Path) -> Option<CacheManifest> {
    if !is_system_compatible(system_info) {
        info!("Cache restore is not supported on this system, skipping");
        return None;
    }

    let metadata_path = cache_dir.join(METADATA_FILENAME);
    if !metadata_path.exists() {
        debug!("No metadata file found in cache directory");
        return None;
    }

    match std::fs::read_to_string(&metadata_path) {
        Ok(content) => Some(CacheManifest::parse(&content)),
        Err(e) => {
            warn!("Failed to read metadata file: {e}");
            None
        }
    }
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

    // Create metadata file containing the cached packages and their versions
    let metadata_path = cache_dir.join(METADATA_FILENAME);
    let mut manifest = read_manifest(system_info, cache_dir).unwrap_or_default();
    manifest.merge(CacheManifest::of_installed_packages(packages));
    let metadata_content = manifest.serialize();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_of(packages: &[(&str, &str)]) -> CacheManifest {
        CacheManifest {
            packages: packages
                .iter()
                .map(|(package, version)| (package.to_string(), version.to_string()))
                .collect(),
        }
    }

    #[test]
    fn manifest_round_trips() {
        let manifest = manifest_of(&[("libc6-dbg", "2.39-0ubuntu8.8"), ("valgrind", "1:3.26.0")]);

        assert_eq!(CacheManifest::parse(&manifest.serialize()), manifest);
    }

    #[test]
    fn manifest_ignores_entries_without_a_version() {
        // Entries saved before versions were recorded hold bare package names
        let manifest = CacheManifest::parse("valgrind\nlibc6-dbg=2.39-0ubuntu8.8");

        assert_eq!(manifest.version_of("valgrind"), None);
        assert_eq!(manifest.version_of("libc6-dbg"), Some("2.39-0ubuntu8.8"));
    }
}
