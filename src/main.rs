use codspeed_runner::{clean_logger, cli, exit_code};
use console::style;
use log::log_enabled;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let res = cli::run().await;
    if let Err(err) = res {
        let code = exit_code::exit_code_for(&err);
        // Show the primary error
        let mut chain = err.chain();
        if let Some(primary) = chain.next() {
            if log_enabled!(log::Level::Error) {
                log::error!("{}", style(primary).red());
            } else {
                eprintln!("{} {}", style("Error:").bold().red(), style(primary).red());
            }
        }
        // Show causes in debug mode
        if log_enabled!(log::Level::Debug) {
            for cause in chain {
                log::debug!("Caused by: {cause}");
            }
        }
        clean_logger();
        std::process::exit(code);
    }
}
