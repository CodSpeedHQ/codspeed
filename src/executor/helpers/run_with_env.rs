//! Forwards the current environment to a command when run with sudo.

use crate::executor::helpers::command::CommandBuilder;
use crate::prelude::*;
use std::collections::HashMap;
use std::io::Write;
use std::process::Command;
use tempfile::NamedTempFile;

/// Returns a list of exported environment variables which can be loaded with `source` in a shell.
///
/// Example: `declare -x outputs="out"`
fn get_exported_system_env() -> Result<String> {
    let output = Command::new("bash")
        .arg("-c")
        .arg("export")
        .output()
        .context("Failed to run `export`")?;
    if !output.status.success() {
        bail!(
            "Failed to get system environment variables: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8(output.stdout).context("Failed to parse export output as UTF-8")
}

/// Wraps a command to run with environment variables forwarded.
///
/// # Returns
/// Returns a tuple of (CommandBuilder, NamedTempFile) where:
/// - CommandBuilder is wrapped with bash to source the environment and run the original command
/// - NamedTempFile is the environment file that must be kept alive until command execution
pub fn wrap_with_env(
    mut cmd_builder: CommandBuilder,
    extra_env: &HashMap<String, String>,
) -> Result<(CommandBuilder, NamedTempFile)> {
    let (bash_command, env_file) =
        prefix_command_with_env(&cmd_builder.as_command_line(), extra_env)?;
    cmd_builder.wrap("bash", ["-c", &bash_command]);

    Ok((cmd_builder, env_file))
}

/// Prefixes a shell command with a `source` of the forwarded environment.
///
/// Unlike [`wrap_with_env`], the returned value is the raw `source <file> && <command>`
/// snippet without a `bash -c` wrapper, for callers that already run their argument
/// through a shell.
pub fn prefix_command_with_env(
    command: &str,
    extra_env: &HashMap<String, String>,
) -> Result<(String, NamedTempFile)> {
    let env_file = create_env_file(extra_env)?;
    let wrapped = format!("source {} && {}", env_file.path().display(), command);
    Ok((wrapped, env_file))
}

fn create_env_file(extra_env: &HashMap<String, String>) -> Result<NamedTempFile> {
    let system_env = get_exported_system_env()?;
    let base_injected_env = extra_env
        .iter()
        .map(|(k, v)| format!("export {k}='{v}'"))
        .collect::<Vec<_>>()
        .join("\n");

    // Create and return the environment file
    let mut env_file = NamedTempFile::new()?;
    env_file.write_all(format!("{system_env}\n{base_injected_env}").as_bytes())?;
    Ok(env_file)
}
