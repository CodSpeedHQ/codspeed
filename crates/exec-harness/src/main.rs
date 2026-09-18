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
