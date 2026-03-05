use super::Config;
use std::path::PathBuf;

use super::create_profile_folder;

/// Per-mode execution context.
///
/// Contains only the mode-specific configuration and the profile folder path.
/// Shared state (provider, system_info, logger) lives in [`Orchestrator`].
pub struct ExecutionContext {
    pub config: Config,
    /// Directory path where profiling data and results are stored
    pub profile_folder: PathBuf,
}

impl ExecutionContext {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let profile_folder = if let Some(profile_folder) = &config.profile_folder {
            profile_folder.clone()
        } else {
            create_profile_folder()?
        };

        Ok(ExecutionContext {
            config,
            profile_folder,
        })
    }
}
