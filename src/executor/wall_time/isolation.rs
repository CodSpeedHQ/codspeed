use crate::executor::helpers::command::CommandBuilder;
use crate::prelude::*;

/// How the benchmark workload is pinned away from the rest of the system.
///
/// The macro-runner reserves a set of CPU cores for the benchmark so nothing
/// else on the host perturbs the measurement. Two mechanisms achieve this; the
/// sandbox (COD-2486) is what forces the second one to exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IsolationMode {
    /// No isolation: the benchmark runs in the runner's own process subtree and
    /// the profiler records its descendants unprivileged.
    None,
    /// `systemd-run --scope --slice=codspeed.slice` isolation, the classic
    /// macro-runner path. The scope *reparents* the benchmark out of the
    /// profiler's process subtree, so the profiler can only observe it by
    /// recording system-wide — which needs elevated privileges.
    Systemd,
    /// Delegated-cgroup isolation, the sandboxed macro-runner path. The host
    /// pre-creates a cgroup (whose `cpuset.cpus` pins the reserved cores) and
    /// delegates it to the workload user; the benchmark leaf self-places into it
    /// by writing its own PID to `<dir>/cgroup.procs` right before `exec`.
    ///
    /// Unlike the systemd scope, this does *not* reparent the benchmark: cgroup
    /// membership is independent of process parentage, so the benchmark stays a
    /// descendant of the profiler and the profiler records it unprivileged via
    /// the normal inherit path.
    Cgroup { cgroup_dir: String },
}

impl IsolationMode {
    /// Whether the profiler must record system-wide (and therefore elevate).
    ///
    /// Only the systemd scope reparents the benchmark out of the profiler's
    /// subtree; every other mode keeps it in-subtree, where the profiler's
    /// inherited events capture it without elevation.
    pub fn requires_system_wide_profiling(&self) -> bool {
        matches!(self, IsolationMode::Systemd)
    }

    pub fn is_isolated(&self) -> bool {
        !matches!(self, IsolationMode::None)
    }
}

/// Resolve how this run should isolate the benchmark.
///
/// - Non-Linux has neither mechanism, so it is always [`IsolationMode::None`].
/// - `CODSPEED_ISOLATION=false` disables isolation outright.
/// - `CODSPEED_CGROUP=<dir>` selects the delegated-cgroup mechanism (the
///   sandbox sets this to a writable, cpuset-pinned cgroup). It wins over the
///   systemd path because it is the only one that works without host systemd.
/// - Otherwise `CODSPEED_ISOLATION=true`, or being able to elevate without a
///   prompt (root or passwordless sudo, as on CI), selects the systemd scope.
///   A local run that would need a password stays unisolated so it never blocks.
pub fn resolve_isolation_mode() -> IsolationMode {
    if !cfg!(target_os = "linux") {
        return IsolationMode::None;
    }

    let isolation_var = std::env::var("CODSPEED_ISOLATION").ok();
    if isolation_var.as_deref() == Some("false") {
        return IsolationMode::None;
    }

    if let Some(cgroup_dir) = non_empty_env("CODSPEED_CGROUP") {
        return IsolationMode::Cgroup { cgroup_dir };
    }

    match isolation_var.as_deref() {
        Some("true") => IsolationMode::Systemd,
        _ => {
            let can_isolate = crate::executor::helpers::run_with_sudo::can_elevate_without_prompt();
            if !can_isolate {
                info!(
                    "Running without process isolation: elevating privileges would require a \
                     password prompt. Set CODSPEED_ISOLATION=true to force it."
                );
                return IsolationMode::None;
            }
            IsolationMode::Systemd
        }
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// Wrap the benchmark leaf so it runs inside the systemd `codspeed.slice` scope.
///
/// Remarks:
/// - We're using `--scope` so the profiler is able to capture the events of the
///   benchmark process (system-wide; see [`IsolationMode::Systemd`]).
/// - We can't use `--user` here because we need to run in `codspeed.slice`,
///   otherwise we'd run in `user.slice`. `--uid`/`--gid` keep the current user.
/// - `--scope` only inherits the system environment, so the caller is expected
///   to have already forwarded the relevant variables (via `wrap_with_env`).
/// - The caller is expected to have already set the working directory on
///   `bench_cmd`; `--same-dir` makes the spawned scope inherit it.
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

/// Wrap the benchmark leaf so it self-places into the delegated cgroup `cgroup_dir`.
///
/// A tiny `bash` shim writes its own PID into `<cgroup_dir>/cgroup.procs`
/// (moving itself, and the benchmark it `exec`s, into the cgroup) and then
/// `exec`s the original command — so the benchmark inherits the cgroup's
/// `cpuset.cpus` pinning while staying the same PID and the same descendant of
/// the profiler. `cgroup.procs` exists in both cgroup v1 and v2, so this works
/// regardless of the host's cgroup version; the cpuset configuration of the
/// cgroup itself is the host's responsibility.
///
/// The shim is wrapped around the *leaf* only, before the profiler wraps the
/// recorder around the whole thing, so the profiler process itself is never
/// moved into the reserved cores.
pub fn wrap_cgroup_isolation(
    mut bench_cmd: CommandBuilder,
    cgroup_dir: &str,
) -> Result<CommandBuilder> {
    // Resolve the procs file once here so the shim is a fixed, quoted literal —
    // no dependence on the workload's environment for the path.
    let procs_path = format!("{}/cgroup.procs", cgroup_dir.trim_end_matches('/'));
    let quoted = shell_words::quote(&procs_path);
    // `$0`/`$@`: the shim's own argv[0] is a throwaway "bash"; the real program
    // and its arguments follow and are re-exec'd verbatim via `exec "$@"`.
    let script = format!("echo $$ > {quoted} && exec \"$@\"");

    bench_cmd.wrap("bash", ["-c".to_string(), script, "bash".to_string()]);
    Ok(bench_cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use temp_env::with_vars;

    #[test]
    fn cgroup_dir_selects_cgroup_mode() {
        if !cfg!(target_os = "linux") {
            return;
        }
        with_vars(
            [
                ("CODSPEED_ISOLATION", None::<&str>),
                ("CODSPEED_CGROUP", Some("/sys/fs/cgroup/codspeed/bench")),
            ],
            || {
                assert_eq!(
                    resolve_isolation_mode(),
                    IsolationMode::Cgroup {
                        cgroup_dir: "/sys/fs/cgroup/codspeed/bench".to_string()
                    }
                );
            },
        );
    }

    #[test]
    fn isolation_false_disables_even_with_cgroup() {
        with_vars(
            [
                ("CODSPEED_ISOLATION", Some("false")),
                ("CODSPEED_CGROUP", Some("/sys/fs/cgroup/codspeed/bench")),
            ],
            || {
                assert_eq!(resolve_isolation_mode(), IsolationMode::None);
            },
        );
    }

    #[test]
    fn cgroup_mode_keeps_profiler_unprivileged() {
        let cgroup = IsolationMode::Cgroup {
            cgroup_dir: "/x".to_string(),
        };
        assert!(cgroup.is_isolated());
        assert!(!cgroup.requires_system_wide_profiling());
        assert!(IsolationMode::Systemd.requires_system_wide_profiling());
    }

    #[test]
    fn cgroup_wrap_places_then_execs() {
        let mut cmd = CommandBuilder::new("bash");
        cmd.arg("/tmp/bench.sh");
        let wrapped = wrap_cgroup_isolation(cmd, "/sys/fs/cgroup/codspeed/bench").unwrap();
        assert_eq!(
            wrapped.as_command_line(),
            "bash -c 'echo $$ > /sys/fs/cgroup/codspeed/bench/cgroup.procs && exec \"$@\"' bash bash /tmp/bench.sh"
        );
    }
}
