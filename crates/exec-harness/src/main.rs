use exec_harness::prelude::*;

fn main() -> Result<()> {
    exec_harness::run_cli(std::env::args_os())
}
