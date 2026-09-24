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
