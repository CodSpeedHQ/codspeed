#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use memtrack::cli::run_cli;
use memtrack::prelude::*;

fn main() -> Result<()> {
    let code = run_cli(std::env::args_os())?;
    std::process::exit(code);
}
