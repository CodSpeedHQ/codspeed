use anyhow::{Result, bail};
use std::ffi::{OsStr, OsString};
use std::process::{Child, Command};

/// Build a shell stub that stops itself with `SIGSTOP` right after the shell
/// execs, then `exec`s the real target in the same pid.
///
/// A `pre_exec` hook that raised `SIGSTOP` cannot be used: it blocks the child
/// before `execve`, and `Command::spawn` blocks reading its exec-status pipe
/// until the child execs, so the two deadlock. The stub sidesteps that — the
/// shell execs immediately (spawn returns), then stops itself. The caller arms
/// tracking while it is stopped and resumes it; the target's own `execve` then
/// happens in the already-tracked pid, so the watcher observes its mappings.
pub fn stopped_command(program: impl AsRef<OsStr>, args: &[OsString]) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(r#"kill -STOP "$$"; exec "$@""#)
        .arg("sh")
        .arg(program.as_ref())
        .args(args);
    cmd
}

/// Re-wrap an already-configured command as a self-stopping shell stub,
/// preserving its program, args, explicit env, and working directory.
///
/// `uid`/`gid` cannot be read back from a `Command`; apply those to the returned
/// command if needed.
pub fn wrap_stopped(command: &Command) -> Command {
    let args: Vec<OsString> = command.get_args().map(OsStr::to_os_string).collect();
    let mut wrapped = stopped_command(command.get_program(), &args);

    for (key, val) in command.get_envs() {
        match val {
            Some(val) => {
                wrapped.env(key, val);
            }
            None => {
                wrapped.env_remove(key);
            }
        }
    }
    if let Some(dir) = command.get_current_dir() {
        wrapped.current_dir(dir);
    }

    wrapped
}

/// Spawn a command built by [`stopped_command`] / [`wrap_stopped`] and block
/// until the shell stub group-stops itself.
pub fn spawn_stopped(cmd: &mut Command) -> Result<Child> {
    let child = cmd.spawn()?;
    let pid = child.id() as i32;

    let mut status: libc::c_int = 0;
    let ret = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) };
    if ret != pid || !libc::WIFSTOPPED(status) {
        bail!("child {pid} exited before tracking was armed (status {status:#x})");
    }

    Ok(child)
}

/// Resume a process previously stopped by [`spawn_stopped`].
///
/// A vanished process (`ESRCH`) is treated as success: the stop is moot.
pub fn resume(pid: i32) -> Result<()> {
    let ret = unsafe { libc::kill(pid, libc::SIGCONT) };
    if ret == 0 {
        return Ok(());
    }

    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    bail!("failed to SIGCONT {pid}: {err}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression for the pre_exec-SIGSTOP deadlock: this must not hang, and the
    /// stub must be group-stopped before we resume it to completion.
    #[test]
    fn spawn_stopped_stops_then_resumes() {
        let mut cmd = stopped_command("true", &[]);
        let mut child = spawn_stopped(&mut cmd).expect("spawn_stopped should not hang");
        let pid = child.id() as i32;

        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let state = stat
            .rsplit(')')
            .next()
            .and_then(|s| s.trim_start().chars().next())
            .unwrap();
        assert!(
            matches!(state, 'T' | 't'),
            "expected stopped, got {state:?}"
        );

        resume(pid).expect("resume");
        let status = child.wait().expect("wait");
        assert!(status.success(), "stub-exec'd target should exit 0");
    }
}
