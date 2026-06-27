use crate::prelude::*;

#[cfg(target_os = "linux")]
use crate::executor::helpers::run_with_sudo::run_with_sudo;
#[cfg(any(test, target_os = "linux"))]
use anyhow::Context;
#[cfg(target_os = "linux")]
use std::process::Command;

pub fn ensure_linux_profiling_sysctls() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        ensure_sysctl("kernel.kptr_restrict", 0)?;
        ensure_sysctl("kernel.perf_event_paranoid", -1)?;
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn ensure_sysctl(name: &str, target_value: i64) -> Result<()> {
    if sysctl_read(name)? == target_value {
        return Ok(());
    }

    let assignment = format!("{name}={target_value}");
    run_with_sudo("sysctl", ["-w", assignment.as_str()])
}

#[cfg(target_os = "linux")]
fn sysctl_read(name: &str) -> Result<i64> {
    let output = Command::new("sysctl").arg(name).output()?;
    let output = String::from_utf8(output.stdout)?;

    parse_sysctl_value(&output)
}

#[cfg(any(test, target_os = "linux"))]
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
