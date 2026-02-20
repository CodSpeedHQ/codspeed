use crate::prelude::*;

pub fn run() -> Result<()> {
    let modes = crate::runner_mode::load_shell_session_mode()?;

    if modes.is_empty() {
        info!("No mode set for this shell session");
    } else {
        let modes_str = modes
            .iter()
            .map(|m| format!("{m:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        info!("{modes_str}");
    }

    Ok(())
}
