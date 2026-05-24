use crate::prelude::*;

/// Trailing args forwarded to the bundled exec-harness CLI parser.
#[derive(Debug, clap::Args)]
pub struct ExecHarnessArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<std::ffi::OsString>,
}

pub fn run(args: ExecHarnessArgs) -> Result<()> {
    let argv = std::iter::once(std::ffi::OsString::from("exec-harness")).chain(args.args);
    ::exec_harness::run_cli(argv)
}
