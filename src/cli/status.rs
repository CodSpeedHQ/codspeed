use crate::VERSION;
use crate::api_client::CodSpeedAPIClient;
use crate::prelude::*;
use crate::system::SystemInfo;
use console::style;

pub fn check_mark() -> console::StyledObject<&'static str> {
    style("✓").green()
}

pub fn cross_mark() -> console::StyledObject<&'static str> {
    style("✗").red()
}

pub fn warn_mark() -> console::StyledObject<&'static str> {
    style("!").yellow()
}

pub async fn run(api_client: &CodSpeedAPIClient) -> Result<()> {
    // Auth status
    super::auth::status(api_client).await?;
    info!("");

    // Setup/tools status
    super::setup::status();
    info!("");

    // System info
    info!("{}", style("System").bold());
    info!("  codspeed {VERSION}");
    let system_info = SystemInfo::new()?;
    info!(
        "  {} {} ({})",
        system_info.os, system_info.os_version, system_info.arch
    );
    info!(
        "  {} ({}C / {}GB)",
        system_info.cpu_brand, system_info.cpu_cores, system_info.total_memory_gb
    );

    Ok(())
}
