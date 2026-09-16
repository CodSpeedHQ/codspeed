//! The standalone `codspeed-memtrack` binary.
//!
//! Everything it does lives in [`memtrack::cli::run_cli`], which the `codspeed`
//! CLI also calls when memtrack runs as a bundled subcommand. All that is left
//! here is what only makes sense when memtrack owns the whole process: the
//! global logger, and turning the tracked command's exit code into our own.

use memtrack::cli::run_cli;
use memtrack::prelude::*;

fn main() -> Result<()> {
    env_logger::builder()
        .parse_env(env_logger::Env::new().filter_or("CODSPEED_LOG", "info"))
        .format_timestamp(None)
        .init();

    let code = run_cli(std::env::args_os())?;
    std::process::exit(code);
}
