use crate::executor::helpers::command::CommandBuilder;
use crate::prelude::*;

/// Run the benchmark command in an isolated process scope.
///
/// On Linux, the command is wrapped with `systemd-run --scope` so it runs inside the
/// `codspeed.slice` cgroup (required for perf to capture the full process tree).
///
/// Remarks:
/// - We're using `--scope` so that perf is able to capture the events of the benchmark process.
/// - We can't use `--user` here because we need to run in `codspeed.slice`, otherwise we'd run in
///   `user.slice` (which is isolated). We use `--uid` and `--gid` to keep running as the current
///   user.
/// - `--scope` only inherits the system environment, so the caller is expected to have already
///   forwarded the relevant variables (via `wrap_with_env`).
/// - The caller is expected to have already set the working directory on `bench_cmd`; it will be
///   propagated to `systemd-run` via [`CommandBuilder::wrap_with`], and `--same-dir` makes the
///   spawned scope inherit it.
#[cfg(target_os = "linux")]
pub fn wrap_with_isolation(mut bench_cmd: CommandBuilder) -> Result<CommandBuilder> {
    use crate::executor::helpers::env::is_codspeed_debug_enabled;

    let mut cmd_builder = CommandBuilder::new("systemd-run");
    if !is_codspeed_debug_enabled() {
        cmd_builder.arg("--quiet");
    }
    cmd_builder.arg("--slice=codspeed.slice");
    cmd_builder.arg("--scope");
    cmd_builder.arg("--same-dir");
    cmd_builder.arg(format!("--uid={}", nix::unistd::Uid::current().as_raw()));
    cmd_builder.arg(format!("--gid={}", nix::unistd::Gid::current().as_raw()));
    cmd_builder.args(["--"]);

    bench_cmd.wrap_with(cmd_builder);
    Ok(bench_cmd)
}

/// Dummy implementation on non-Linux platforms: the benchmark command is returned as-is.
// TODO(COD-2513): implement an equivalent process-isolation mechanism on macOS
#[cfg(not(target_os = "linux"))]
pub fn wrap_with_isolation(bench_cmd: CommandBuilder) -> Result<CommandBuilder> {
    Ok(bench_cmd)
}
