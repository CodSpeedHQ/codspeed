use crate::prelude::*;
use std::fs;
use std::path::Path;

mod discover;
mod interfaces;
pub mod merger;

pub use discover::*;
pub use interfaces::*;

impl ProjectConfig {
    /// Load and parse config from a specific path
    pub(crate) fn load_from_path(path: &Path) -> Result<Self> {
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
        if let Some(options) = &self.options
            && let Some(walltime) = &options.walltime
        {
            Self::validate_walltime_options(walltime, "root options")?;
        }
        Ok(())
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
