use crate::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};

mod interfaces;
pub mod merger;

pub use interfaces::*;

/// Config file names in priority order
const CONFIG_FILENAMES: &[&str] = &[
    "codspeed.yaml",
    "codspeed.yml",
    ".codspeed.yaml",
    ".codspeed.yml",
];

impl ProjectConfig {
    /// Discover and load project configuration file
    ///
    /// # Search Strategy
    /// 1. If `config_path_override` is provided, load from that path only (error if not found)
    /// 2. Otherwise, search for config files in current directory and upward to git root
    /// 3. Try filenames in priority order: codspeed.yaml, codspeed.yml, .codspeed.yaml, .codspeed.yml
    /// 4. If a config is found in a parent directory, changes the working directory to that location
    ///
    /// # Arguments
    /// * `config_path_override` - Explicit path to config file (from --config flag)
    /// * `current_dir` - Directory to start searching from
    ///
    /// # Returns
    /// * `Ok(Some(config))` - Config found and loaded successfully
    /// * `Ok(None)` - No config file found
    /// * `Err(_)` - Error loading or parsing config
    pub fn discover_and_load(
        config_path_override: Option<&Path>,
        current_dir: &Path,
    ) -> Result<Option<ProjectConfig>> {
        // Case 1: Explicit --config path provided
        if let Some(config_path) = config_path_override {
            let config = Self::load_from_path(config_path)
                .with_context(|| format!("Failed to load config from {}", config_path.display()))?;
            let canonical_path = config_path
                .canonicalize()
                .unwrap_or_else(|_| config_path.to_path_buf());

            // Change working directory if config was found in a different directory
            Self::change_to_config_directory(&canonical_path, current_dir)?;

            return Ok(Some(config));
        }

        // Case 2: Search for config files
        let search_dirs = Self::get_search_directories(current_dir)?;

        for dir in search_dirs {
            for filename in CONFIG_FILENAMES {
                let candidate_path = dir.join(filename);
                if candidate_path.exists() {
                    debug!("Found config file at {}", candidate_path.display());
                    let config = Self::load_from_path(&candidate_path)?;
                    let canonical_path = candidate_path.canonicalize().unwrap_or(candidate_path);

                    // Change working directory if config was found in a different directory
                    Self::change_to_config_directory(&canonical_path, current_dir)?;

                    return Ok(Some(config));
                }
            }
        }

        // No config found - this is OK
        Ok(None)
    }

    /// Get list of directories to search for config files
    ///
    /// Returns directories from current_dir upward to git root (if in a git repo)
    fn get_search_directories(current_dir: &Path) -> Result<Vec<PathBuf>> {
        let mut dirs = vec![current_dir.to_path_buf()];

        // Try to find git repository root
        if let Some(git_root) = crate::cli::run::helpers::find_repository_root(current_dir) {
            // Add parent directories up to git root
            let mut dir = current_dir.to_path_buf();
            while let Some(parent) = dir.parent() {
                if parent == git_root {
                    if !dirs.contains(&git_root) {
                        dirs.push(git_root.clone());
                    }
                    break;
                }
                if !dirs.contains(&parent.to_path_buf()) {
                    dirs.push(parent.to_path_buf());
                }
                dir = parent.to_path_buf();
            }
        }

        Ok(dirs)
    }

    /// Change working directory to the directory containing the config file
    fn change_to_config_directory(config_path: &Path, original_dir: &Path) -> Result<()> {
        let config_dir = config_path
            .parent()
            .context("Config file has no parent directory")?;

        if config_dir != original_dir {
            std::env::set_current_dir(config_dir)?;
            debug!(
                "Changed working directory from {} to {}",
                original_dir.display(),
                config_dir.display()
            );
        }

        Ok(())
    }

    /// Load and parse config from a specific path
    fn load_from_path(path: &Path) -> Result<Self> {
        let config_content = fs::read(path)
            .with_context(|| format!("Failed to read config file at {}", path.display()))?;

        let config: Self = serde_yaml::from_slice(&config_content).with_context(|| {
            format!(
                "Failed to parse CodSpeed project config at {}",
                path.display()
            )
        })?;

        // Validate the config
        config.validate()?;

        Ok(config)
    }

    /// Validate the configuration
    ///
    /// Checks for invalid combinations of options, particularly in walltime config
    fn validate(&self) -> Result<()> {
        if let Some(options) = &self.options {
            if let Some(walltime) = &options.walltime {
                Self::validate_walltime_options(walltime, "root options")?;
            }
        }
        if let Some(benchmarks) = &self.benchmarks {
            for target in benchmarks {
                Self::validate_target(target)?;
            }
        }
        Ok(())
    }

    /// Validate a single benchmark target
    fn validate_target(target: &Target) -> Result<()> {
        match (&target.exec, &target.entrypoint) {
            (None, None) => bail!("Benchmark target must specify either 'exec' or 'entrypoint'"),
            (Some(_), Some(_)) => bail!(
                "Benchmark target cannot specify both 'exec' and 'entrypoint' (they are mutually exclusive)"
            ),
            _ => Ok(()),
        }
    }

    /// Validate walltime options for conflicting constraints
    fn validate_walltime_options(opts: &WalltimeOptions, context: &str) -> Result<()> {
        // Check for explicitly forbidden combinations
        if opts.min_time.is_some() && opts.max_rounds.is_some() {
            bail!(
                "Invalid walltime configuration in {context}: cannot use both min_time and max_rounds"
            );
        }

        if opts.max_time.is_some() && opts.min_rounds.is_some() {
            bail!(
                "Invalid walltime configuration in {context}: cannot use both max_time and min_rounds"
            );
        }

        // Note: We don't parse durations here or check min < max relationships
        // That validation happens later in WalltimeExecutionArgs::try_from(ExecutionOptions)

        Ok(())
    }
}

#[cfg(test)]
mod tests;
