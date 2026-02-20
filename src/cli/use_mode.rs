//! Named like this because `use` is a keyword

use crate::prelude::*;
use crate::runner_mode::RunnerMode;
use clap::Args;

#[derive(Debug, Args)]
pub struct UseArgs {
    /// Set the CodSpeed runner mode(s) for this shell session.
    /// Multiple modes can be provided as separate arguments (e.g. `simulation walltime`)
    /// or comma-separated (e.g. `simulation,walltime`).
    #[arg(value_delimiter = ',', required = true)]
    pub mode: Vec<RunnerMode>,
}

pub fn run(args: UseArgs) -> Result<()> {
    crate::runner_mode::register_shell_session_mode(&args.mode)?;
    debug!(
        "Registered codspeed use mode '{:?}' for this shell session (parent PID)",
        args.mode
    );
    Ok(())
}
