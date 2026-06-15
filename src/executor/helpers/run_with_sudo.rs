use crate::executor::helpers::command::CommandBuilder;
use crate::{local_logger::suspend_progress_bar, prelude::*};
use std::{
    ffi::OsStr,
    io::IsTerminal,
    process::{Command, Stdio},
};

pub fn is_root_user() -> bool {
    #[cfg(unix)]
    return nix::unistd::Uid::current().is_root();
    #[cfg(not(unix))]
    return false;
}

fn is_sudo_available() -> bool {
    Command::new("sudo")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Whether `sudo` can run without prompting for a password (passwordless rule or
/// a cached, still-valid credential). Used to decide whether a command can be
/// elevated non-interactively before committing to a privileged code path.
fn sudo_runs_without_password() -> bool {
    Command::new("sudo")
        .arg("--non-interactive") // Fail if a password is required
        .arg("true")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Whether we can elevate privileges without prompting the user: either we're
/// already root, or `sudo` is installed and runs without asking for a password.
pub fn can_elevate_without_prompt() -> bool {
    is_root_user() || (is_sudo_available() && sudo_runs_without_password())
}

/// Validate sudo access, prompting the user for their password if necessary
fn validate_sudo_access() -> Result<()> {
    let needs_password =
        IsTerminal::is_terminal(&std::io::stdout()) && !sudo_runs_without_password();

    if needs_password {
        suspend_progress_bar(|| {
            info!("Sudo privileges are required to continue.");

            // Validate and cache sudo credentials
            let auth_status = Command::new("sudo")
                .arg("--validate") // Validate and extend the timeout
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()
                .map_err(|_| anyhow!("Failed to authenticate with sudo"))?;

            if !auth_status.success() {
                bail!("Failed to authenticate with sudo");
            }
            Ok(())
        })?;
    }
    Ok(())
}

/// Wrap with sudo if not running as root
pub fn wrap_with_sudo(mut cmd_builder: CommandBuilder) -> Result<CommandBuilder> {
    if is_root_user() {
        Ok(cmd_builder)
    } else if is_sudo_available() {
        debug!("Wrapping with sudo: {}", cmd_builder.as_command_line());
        validate_sudo_access()?;
        cmd_builder.wrap(
            "sudo",
            [
                // Password prompt should not appear here since it has already been validated
                "--non-interactive",
                // Forward all environment variables to the command
                "--preserve-env",
            ],
        );
        Ok(cmd_builder)
    } else {
        bail!(
            "Sudo is not available to run the command: {}",
            cmd_builder.as_command_line()
        );
    }
}

/// Run a command with sudo after validating sudo access
pub fn run_with_sudo<S, I, T>(program: S, argv: I) -> Result<()>
where
    S: AsRef<OsStr>,
    I: IntoIterator<Item = T>,
    T: AsRef<OsStr>,
{
    let mut builder = CommandBuilder::new(program);
    builder.args(argv);
    debug!("Running command with sudo: {}", builder.as_command_line());
    let mut cmd = wrap_with_sudo(builder)?.build();
    let output = cmd
        .stdout(Stdio::piped())
        .output()
        .map_err(|_| anyhow!("Failed to execute command with sudo: {cmd:?}"))?;

    if !output.status.success() {
        info!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        error!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        bail!("Failed to execute command with sudo: {cmd:?}");
    }

    Ok(())
}
