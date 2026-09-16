//! The standalone `exec-harness` binary.
//!
//! Everything it does lives in [`exec_harness::cli::run_cli`], which the
//! `codspeed` CLI also calls when exec-harness runs as a bundled subcommand.
//! All that is left here is the global logger, which only makes sense when
//! exec-harness owns the whole process.

use exec_harness::cli::run_cli;
use exec_harness::prelude::*;

fn main() -> Result<()> {
    env_logger::builder()
        .parse_env(env_logger::Env::new().filter_or("CODSPEED_LOG", "info"))
        .format(|buf, record| {
            use std::io::Write;
            writeln!(buf, "{}", record.args())
        })
        .init();

    run_cli(std::env::args_os())
}
