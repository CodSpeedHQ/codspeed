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
    let env_file = create_env_file(extra_env)?;

    // Create bash command that sources the env file and runs the original command
    let original_command = cmd_builder.as_command_line();
    let bash_command = format!(
        "source {} && {}",
        env_file.path().display(),
        original_command
    );
    cmd_builder.wrap("bash", ["-c", &bash_command]);

    Ok((cmd_builder, env_file))
}

fn create_env_file(extra_env: &HashMap<String, String>) -> Result<NamedTempFile> {
    let system_env = get_exported_system_env()?;
    let base_injected_env = extra_env
        .iter()
        .map(|(k, v)| format!("export {k}={v}"))
        .collect::<Vec<_>>()
        .join("\n");

    // Create and return the environment file
    let mut env_file = NamedTempFile::new()?;
    env_file.write_all(format!("{system_env}\n{base_injected_env}").as_bytes())?;
    Ok(env_file)
}
