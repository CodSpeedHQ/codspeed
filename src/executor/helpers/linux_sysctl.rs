use crate::executor::helpers::run_with_sudo::run_with_sudo;
use crate::prelude::*;
use anyhow::Context;
use std::process::Command;

/// Restores a sysctl to its initial value when dropped.
#[derive(Debug)]
#[must_use = "the sysctl is restored when this guard is dropped"]
pub(crate) struct LinuxSysctl {
    name: &'static str,
    previous: Option<i64>,
}

impl LinuxSysctl {
    pub(crate) fn set(name: &'static str, target_value: i64) -> Result<Self> {
        let previous = ensure_sysctl(name, target_value)?;

        Ok(Self { name, previous })
    }

    pub(crate) fn is_changed(&self) -> bool {
        self.previous.is_some()
    }
}

impl Drop for LinuxSysctl {
    fn drop(&mut self) {
        let Some(value) = self.previous else {
            return;
        };

        if let Err(error) = ensure_sysctl(self.name, value) {
            warn!("Failed to restore {}={value}: {error}", self.name);
        }
    }
}

pub fn ensure_linux_profiling_sysctls() -> Result<Vec<LinuxSysctl>> {
    if !cfg!(target_os = "linux") {
        return Ok(Vec::new());
    }

    let mut sysctls = Vec::new();

    for (name, target_value) in [
        ("kernel.kptr_restrict", 0),
        ("kernel.perf_event_paranoid", -1),
    ] {
        let sysctl = LinuxSysctl::set(name, target_value)?;
        if sysctl.is_changed() {
            sysctls.push(sysctl);
        }
    }

    Ok(sysctls)
}

/// Sets a sysctl, returning the value it held before, or `None` when it was
/// already at `target_value` and nothing was written.
pub(crate) fn ensure_sysctl(name: &str, target_value: i64) -> Result<Option<i64>> {
    let current_value = sysctl_read(name)?;
    if current_value == target_value {
        return Ok(None);
    }

    let assignment = format!("{name}={target_value}");
    run_with_sudo("sysctl", ["-w", assignment.as_str()])?;

    Ok(Some(current_value))
}

fn sysctl_read(name: &str) -> Result<i64> {
    let output = Command::new("sysctl").arg(name).output()?;
    let output = String::from_utf8(output.stdout)?;

    parse_sysctl_value(&output)
}

fn parse_sysctl_value(output: &str) -> Result<i64> {
    let (_, value) = output
        .split_once('=')
        .context("Couldn't find the value in sysctl output")?;

    Ok(value.trim().parse::<i64>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sysctl_value() {
        assert_eq!(parse_sysctl_value("kernel.kptr_restrict = 0\n").unwrap(), 0);
    }

    #[test]
    fn parses_negative_sysctl_value() {
        assert_eq!(
            parse_sysctl_value("kernel.perf_event_paranoid = -1\n").unwrap(),
            -1
        );
    }

    #[test]
    fn rejects_sysctl_output_without_value_separator() {
        assert!(parse_sysctl_value("kernel.kptr_restrict 0\n").is_err());
    }
}
