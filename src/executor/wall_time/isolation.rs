use std::process::Command;

use crate::executor::helpers::command::CommandBuilder;
use crate::prelude::*;

/// Paths of the machine's CPU-isolation hooks. The machine image installs them
/// and owns all cpuset logic behind them; the runner only invokes them.
const PRE_BENCH_HOOK: &str = "/usr/local/bin/codspeed-pre-bench";
const WRAP_BENCH_HOOK: &str = "/usr/local/bin/codspeed-wrap-bench";
const POST_BENCH_HOOK: &str = "/usr/local/bin/codspeed-post-bench";

/// Environment variable selecting how walltime runs isolate the benchmark.
const ISOLATION_ENV: &str = "CODSPEED_ISOLATION";

/// How the benchmark is pinned to dedicated cores, away from the rest of the
/// system, for the lifetime of one walltime run.
///
/// This owns the whole isolation lifecycle: [`resolve`](Self::resolve) runs the
/// machine's pre-bench setup, [`wrap_bench`](Self::wrap_bench) pins the benchmark
/// leaf, and [`Drop`] runs the post-bench teardown. The runner holds one of these
/// for the duration of the run and is otherwise oblivious to *how* cores are
/// attributed — that lives entirely on the machine (its hooks) or in the systemd
/// fallback below.
#[derive(Debug)]
pub enum Isolation {
    /// The benchmark runs unpinned, in the runner's own process subtree.
    None,
    /// The machine's hooks own core attribution. The image built a static cgroup
    /// skeleton and ships `codspeed-{pre,wrap,post}-bench`. `pre-bench` (already
    /// run, with the runner's PID) placed the runner — and the profiler it later
    /// spawns as a child — onto the system cores; `wrap-bench` moves the benchmark
    /// onto the bench cores; `post-bench` runs on drop. The benchmark stays a
    /// descendant of the profiler, so the profiler records it unprivileged.
    Hooks,
    /// Gen-1 fallback, used when the machine ships no `codspeed-wrap-bench` hook.
    /// `systemd-run --scope --slice=codspeed.slice` reparents the benchmark out of
    /// the profiler's subtree, so the profiler needs elevated privileges (sudo) to
    /// observe it.
    Systemd,
}

impl Isolation {
    /// Resolve how this run isolates the benchmark, running any required setup
    /// (the pre-bench hook) as a side effect. Decision, from [`ISOLATION_ENV`]:
    ///
    /// - non-Linux: [`None`](Self::None), no mechanism is available;
    /// - `false`: [`None`](Self::None);
    /// - otherwise, on Linux:
    ///   - if the machine ships an executable `codspeed-wrap-bench`: [`Hooks`](Self::Hooks)
    ///     (the forward path — the machine owns core attribution);
    ///   - else if we can elevate without a prompt (root or passwordless sudo, as
    ///     on CI / gen-1 images): [`Systemd`](Self::Systemd);
    ///   - else [`None`](Self::None), so a local run never blocks on a password prompt.
    pub fn resolve() -> Self {
        if !cfg!(target_os = "linux") {
            return Isolation::None;
        }

        if std::env::var(ISOLATION_ENV).is_ok_and(|v| v == "false") {
            return Isolation::None;
        }

        if hook_is_executable(WRAP_BENCH_HOOK) {
            run_pre_bench();
            return Isolation::Hooks;
        }

        if crate::executor::helpers::run_with_sudo::can_elevate_without_prompt() {
            return Isolation::Systemd;
        }

        if std::env::var(ISOLATION_ENV).is_ok_and(|v| !v.is_empty()) {
            info!(
                "Running without process isolation: no {WRAP_BENCH_HOOK} hook, and elevating \
                 would require a password prompt."
            );
        }
        Isolation::None
    }

    /// Whether the profiler must run under sudo. True only for [`Systemd`](Self::Systemd),
    /// where the benchmark is reparented out of the profiler's subtree.
    pub fn requires_sudo(&self) -> bool {
        matches!(self, Isolation::Systemd)
    }

    /// Wrap the benchmark leaf so it runs pinned according to this mode. The
    /// profiler wraps its recorder around the result, so this must only touch the
    /// leaf and never move the profiler onto the bench cores.
    pub fn wrap_bench(&self, bench_cmd: CommandBuilder) -> Result<CommandBuilder> {
        match self {
            Isolation::None => Ok(bench_cmd),
            Isolation::Hooks => wrap_with_hook(bench_cmd),
            Isolation::Systemd => wrap_isolation_scope(bench_cmd),
        }
    }
}

impl Drop for Isolation {
    fn drop(&mut self) {
        if matches!(self, Isolation::Hooks) {
            run_hook(POST_BENCH_HOOK, &[]);
        }
    }
}

/// Whether `path` is an executable file. Used to detect machine hooks.
fn hook_is_executable(path: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        false
    }
}

/// Run a machine hook, logging (never failing the run) on error. Isolation is
/// best-effort: a missing or failing hook leaves the benchmark less isolated, but
/// must not abort the run.
fn run_hook(hook: &str, args: &[String]) {
    let output = Command::new(hook).args(args).output();
    match output {
        Ok(o) if o.status.success() => {}
        Ok(o) => debug!(
            "{hook} exited {}: {}",
            o.status,
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) => debug!("failed to run {hook}: {e}"),
    }
}

/// Run the pre-bench hook with the runner's own PID, so the hook places the runner
/// (and the profiler it later spawns) onto the system cores.
fn run_pre_bench() {
    run_hook(PRE_BENCH_HOOK, &[std::process::id().to_string()]);
}

/// Wrap the benchmark with `codspeed-wrap-bench`, which moves itself onto the
/// bench cores and `exec`s the benchmark — so the benchmark inherits the pinning
/// while staying the same PID and a descendant of the profiler. The hook is a
/// command wrapper: it takes the program and its arguments and `exec`s them.
fn wrap_with_hook(mut bench_cmd: CommandBuilder) -> Result<CommandBuilder> {
    bench_cmd.wrap(WRAP_BENCH_HOOK, [] as [&str; 0]);
    Ok(bench_cmd)
}

/// Wrap the benchmark leaf so it runs inside the systemd `codspeed.slice` scope.
///
/// Notes on the `systemd-run` flags:
/// - `--scope` (rather than a transient service) keeps the benchmark a child of
///   the runner, so the profiler can capture its events; see [`Isolation::Systemd`].
/// - `--user` would land us in `user.slice`, not `codspeed.slice`; `--uid`/`--gid`
///   instead keep the scope running as the current user.
/// - `--scope` only inherits the system environment, so the caller must have
///   already forwarded the benchmark's variables (via `wrap_with_env`) and set
///   its working directory, which `--same-dir` propagates into the scope.
#[cfg(target_os = "linux")]
fn wrap_isolation_scope(mut bench_cmd: CommandBuilder) -> Result<CommandBuilder> {
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
fn wrap_isolation_scope(bench_cmd: CommandBuilder) -> Result<CommandBuilder> {
    Ok(bench_cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use temp_env::with_vars;

    #[test]
    fn isolation_false_disables() {
        with_vars([(ISOLATION_ENV, Some("false"))], || {
            assert!(matches!(Isolation::resolve(), Isolation::None));
        });
    }

    #[test]
    fn systemd_requires_sudo_but_hooks_do_not() {
        assert!(Isolation::Systemd.requires_sudo());
        assert!(!Isolation::Hooks.requires_sudo());
        assert!(!Isolation::None.requires_sudo());
    }

    #[test]
    fn hook_wrap_prepends_wrap_bench() {
        let mut cmd = CommandBuilder::new("bash");
        cmd.arg("/tmp/bench.sh");
        let wrapped = Isolation::Hooks.wrap_bench(cmd).unwrap();
        assert_eq!(
            wrapped.as_command_line(),
            format!("{WRAP_BENCH_HOOK} bash /tmp/bench.sh")
        );
    }

    #[test]
    fn none_wrap_is_passthrough() {
        let mut cmd = CommandBuilder::new("bash");
        cmd.arg("/tmp/bench.sh");
        let wrapped = Isolation::None.wrap_bench(cmd).unwrap();
        assert_eq!(wrapped.as_command_line(), "bash /tmp/bench.sh");
    }
}
