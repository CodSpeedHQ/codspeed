use super::ExecutorConfig;
use std::path::PathBuf;

/// Per-mode execution context.
///
/// Contains only the mode-specific configuration and the profile folder path.
/// Shared state (provider, system_info, logger) lives in [`Orchestrator`].
pub struct ExecutionContext {
    pub config: ExecutorConfig,
    /// Directory path where profiling data and results are stored
    pub profile_folder: PathBuf,
}

impl ExecutionContext {
    pub fn new(config: ExecutorConfig, profile_folder: PathBuf) -> Self {
        ExecutionContext {
            config,
            profile_folder,
        }
    }
}
