use crate::prelude::*;

/// Trailing args forwarded to the bundled memtrack CLI parser.
#[derive(Debug, clap::Args)]
pub struct MemtrackArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<std::ffi::OsString>,
}

#[cfg(target_os = "linux")]
pub fn run(args: MemtrackArgs) -> Result<()> {
    let argv = std::iter::once(std::ffi::OsString::from("memtrack")).chain(args.args);
    ::memtrack::run_cli(argv)
}

#[cfg(not(target_os = "linux"))]
pub fn run(_args: MemtrackArgs) -> Result<()> {
    // memtrack is eBPF-based and Linux-only. We still expose the subcommand on
    // every platform so the CLI surface is uniform and re-exec attempts on the
    // wrong host fail loudly rather than as a clap "unknown subcommand" error.
    bail!("`codspeed tool memtrack` is only supported on Linux (memtrack uses eBPF)");
}
