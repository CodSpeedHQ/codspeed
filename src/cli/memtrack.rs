use crate::prelude::*;

#[derive(Debug, clap::Args)]
pub struct MemtrackArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<std::ffi::OsString>,
}

pub fn run(args: MemtrackArgs) -> Result<()> {
    // memtrack's own clap parser expects its name as `argv[0]`, not ours.
    let argv = std::iter::once(std::ffi::OsString::from("memtrack")).chain(args.args);

    // The runner reads this exit code to decide whether the benchmark failed.
    let code = ::memtrack::cli::run_cli(argv)?;
    std::process::exit(code);
}
