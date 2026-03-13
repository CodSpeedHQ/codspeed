use crate::executor::get_all_executors;
use crate::prelude::*;
use crate::system::SystemInfo;
use std::path::Path;

pub async fn setup(setup_cache_dir: Option<&Path>) -> Result<()> {
    let system_info = SystemInfo::new()?;
    let executors = get_all_executors();
    start_group!("Setting up the environment for all executors");
    for executor in executors {
        info!(
            "Setting up the environment for the executor: {}",
            executor.name()
        );
        executor.setup(&system_info, setup_cache_dir).await?;
    }
    info!("Environment setup completed");
    end_group!();
    Ok(())
}
