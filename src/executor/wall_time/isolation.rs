use crate::executor::helpers::command::CommandBuilder;
use crate::prelude::*;

/// Whether the benchmark must run inside the privileged systemd scope, which in
/// turn requires the profiler itself to record under elevated privileges.
///
/// When isolated, the scope reparents the benchmark out of the profiler's
/// process subtree, so the profiler can only observe it by recording
/// system-wide — which needs `sudo`. When not isolated, the profiler records its
/// own descendant tree and runs unprivileged (relying on a permissive
/// `perf_event_paranoid`).
///
/// `CODSPEED_ISOLATION` overrides the decision; otherwise we isolate only when we
/// can elevate without prompting (root or passwordless sudo, as on CI), so a
/// local run never blocks on a password.
///
/// Isolation relies on `systemd-run`, so it is Linux-only; other platforms always
/// record their own descendant tree unprivileged.
pub fn requires_isolation() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    match std::env::var("CODSPEED_ISOLATION").as_deref() {
        Ok("true") => true,
        Ok("false") => false,
        _ => {
            let can_isolate = crate::executor::helpers::run_with_sudo::can_elevate_without_prompt();
            if !can_isolate {
                info!(
                    "Running without process isolation: elevating privileges would require a \
                     password prompt. Set CODSPEED_ISOLATION=true to force it."
                );
            }
            can_isolate
        }
    }
}

/// Run the benchmark command in an isolated process scope.
///
/// On Linux, the command is wrapped with `systemd-run --scope` so it runs inside the
/// `codspeed.slice` cgroup (predefined on CodSpeed CI runners to pin and isolate the
/// benchmark). Only applied when [`requires_isolation`] is true.
///
/// Remarks:
/// - We're using `--scope` so that the profiler is able to capture the events of the benchmark
///   process.
/// - We can't use `--user` here because we need to run in `codspeed.slice`, otherwise we'd run in
///   `user.slice` (which is isolated). We use `--uid` and `--gid` to keep running as the current
///   user.
/// - `--scope` only inherits the system environment, so the caller is expected to have already
///   forwarded the relevant variables (via `wrap_with_env`).
/// - The caller is expected to have already set the working directory on `bench_cmd`; it will be
///   propagated to `systemd-run` via [`CommandBuilder::wrap_with`], and `--same-dir` makes the
///   spawned scope inherit it.
#[cfg(target_os = "linux")]
pub fn wrap_isolation_scope(mut bench_cmd: CommandBuilder) -> Result<CommandBuilder> {
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
pub fn wrap_isolation_scope(bench_cmd: CommandBuilder) -> Result<CommandBuilder> {
    Ok(bench_cmd)
}
