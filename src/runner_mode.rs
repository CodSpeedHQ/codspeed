use crate::prelude::*;
use crate::shell_session_store::{self, SessionKind};
use clap::ValueEnum;
use serde::Deserialize;
use serde::Serialize;

#[derive(ValueEnum, Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RunnerMode {
    #[deprecated(note = "Use `RunnerMode::Simulation` instead")]
    Instrumentation,
    Simulation,
    Walltime,
    Memory,
}

/// Register the active runner mode(s) for the current shell session.
pub(crate) fn register_shell_session_mode(modes: &[RunnerMode]) -> Result<()> {
    shell_session_store::register(SessionKind::Mode, &modes.to_vec())
}

/// Load the active runner mode(s) for the current shell session, or
/// an empty vector if none has been set.
pub(crate) fn load_shell_session_mode() -> Result<Vec<RunnerMode>> {
    Ok(shell_session_store::load::<Vec<RunnerMode>>(SessionKind::Mode)?.unwrap_or_default())
}
