use memtrack::prelude::*;

fn main() -> Result<()> {
    memtrack::run_cli(std::env::args_os())
}
