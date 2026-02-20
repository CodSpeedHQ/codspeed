use crate::logger::{GROUP_TARGET, OPENED_GROUP_TARGET};
use crate::prelude::*;
use crate::run_environment::RunEnvironmentProvider;
use log::LevelFilter;
use simplelog::{CombinedLogger, WriteLogger};
use std::fs::copy;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

pub struct Logger {
    log_file_path: PathBuf,
}

impl Logger {
    pub fn new(provider: &dyn RunEnvironmentProvider) -> Result<Self> {
        let provider_logger = provider.get_logger();
        let log_file = NamedTempFile::new().context("Failed to create log file")?;
        let log_file_path = log_file.path().to_path_buf();

        let file_logger_config = simplelog::ConfigBuilder::new()
            // Groups are not logged to the file
            .add_filter_ignore_str(GROUP_TARGET)
            .add_filter_ignore_str(OPENED_GROUP_TARGET)
            .build();
        let file_logger = WriteLogger::new(LevelFilter::Trace, file_logger_config, log_file);

        if let Err(_e) = CombinedLogger::init(vec![provider_logger, file_logger]) {
            // In tests, test_log already initializes a logger, so we ignore the error
            #[cfg(not(test))]
            return Err(anyhow::Error::from(_e).context("Failed to init logger"));
        }

        Ok(Self { log_file_path })
    }

    pub fn persist_log_to_profile_folder(&self, profile_folder: &Path) -> Result<()> {
        let dest_log_file_path = profile_folder.join("runner.log");
        debug!("Persisting log file to {}", dest_log_file_path.display());
        log::logger().flush();
        copy(&self.log_file_path, dest_log_file_path).context("Failed to copy log file")?;
        Ok(())
    }
}
