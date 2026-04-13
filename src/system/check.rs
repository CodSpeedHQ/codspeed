use crate::prelude::*;

use super::SystemInfo;

pub fn check_system(system_info: &SystemInfo) -> Result<()> {
    debug!("System info: {system_info:#?}");
    Ok(())
}
