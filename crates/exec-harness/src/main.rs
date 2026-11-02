use exec_harness::cli::run_cli;
use exec_harness::prelude::*;

fn main() -> Result<()> {
    run_cli(std::env::args_os())
}
