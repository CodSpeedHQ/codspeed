use clap::Parser;

use crate::prelude::*;

/// Run the bundled samply profiler. Arguments after `samply` are forwarded
/// verbatim to samply's own CLI parser.
#[derive(Debug, clap::Args)]
pub struct SamplyArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<std::ffi::OsString>,
}

pub fn run(args: SamplyArgs) -> Result<()> {
    use ::samply::cli;

    let argv = std::iter::once(std::ffi::OsString::from("samply")).chain(args.args);
    let opt = cli::Opt::parse_from(argv);

    // samply spins up its own tokio runtime internally, so it must run on a
    // thread that isn't already inside our `#[tokio::main]` runtime.
    std::thread::scope(|s| {
        s.spawn(|| match opt.action {
            #[cfg(any(
                target_os = "android",
                target_os = "macos",
                target_os = "linux",
                target_os = "windows"
            ))]
            cli::Action::Record(a) => ::samply::do_record_action(a),
            _ => unimplemented!("Only `samply record` is supported"),
        })
        .join()
        .map_err(|_| anyhow::anyhow!("samply thread panicked"))
    })?;

    Ok(())
}
