use crate::prelude::*;

#[derive(Debug, clap::Args)]
pub struct ExecHarnessArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<std::ffi::OsString>,
}

pub fn run(args: ExecHarnessArgs) -> Result<()> {
    // exec-harness's own clap parser expects its name as `argv[0]`, not ours.
    let argv = std::iter::once(std::ffi::OsString::from("exec-harness")).chain(args.args);

    ::exec_harness::cli::run_cli(argv)
}
