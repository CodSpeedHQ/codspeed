use runner_shared::perf_event::PerfEvent;

use crate::prelude::*;
use std::path::Path;

use std::{ffi::OsString, process::Command};

const FIND_PERF_CMD: &str =
    "find /usr/lib -executable -path \"/usr/lib/linux-tools-*/perf\" | sort | tail -n1";

/// Attempts to find the path to the `perf` executable that is installed and working.
/// Returns None if `perf` is not installed or not functioning correctly.
pub fn get_working_perf_executable() -> Option<OsString> {
    let is_installed = Command::new("which")
        .arg("perf")
        .output()
        .is_ok_and(|output| output.status.success());
    if !is_installed {
        debug!("perf is not installed");
        return None;
    }

    debug!("perf is installed, checking if it is functioning correctly");
    if Command::new("perf")
        .arg("--version") // here we use --version to check if perf is working
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return Some("perf".into());
    } else {
        // The following is a workaround for this outstanding Ubuntu issue: https://bugs.launchpad.net/ubuntu/+source/linux-hwe-6.14/+bug/2117159/
        debug!(
            "perf command is not functioning correctly, trying to find alternative path using \"{FIND_PERF_CMD}\""
        );
        if let Ok(perf_path) = Command::new("sh").arg("-c").arg(FIND_PERF_CMD).output() {
            if perf_path.status.success() {
                let path = String::from_utf8_lossy(&perf_path.stdout)
                    .trim()
                    .to_string();
                if path.is_empty() {
                    debug!("No alternative perf path found");
                    return None;
                }
                debug!("Found perf path: {path}");
                // Check if this perf is working by getting its version
                if let Ok(version_output) = Command::new(&path).arg("--version").output() {
                    if !version_output.status.success() {
                        debug!(
                            "Failed to get perf version from alternative path. stderr: {}",
                            String::from_utf8_lossy(&version_output.stderr)
                        );
                        return None;
                    }

                    let version = String::from_utf8_lossy(&version_output.stdout)
                        .trim()
                        .to_string();
                    debug!("Found perf version from alternative path: {version}");
                    return Some(path.into());
                }
            }
        }
    }

    debug!("perf is installed but not functioning correctly");
    None
}

/// Detects if the required perf events are available on this system.
/// Returns the flags to pass to perf record command if they are available, otherwise returns None.
pub fn get_event_flags(perf_executable: &OsString) -> anyhow::Result<Option<String>> {
    let perf_events = PerfEvent::all_events();

    let output = Command::new(perf_executable)
        .arg("list")
        .output()
        .context("Failed to run perf list")?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Check if all required events are available
    // Expected format in `perf list` output:
    //
    // List of pre-defined events (to be used in -e or -M):
    //
    //  branch-instructions OR branches                    [Hardware event]
    //  branch-misses                                      [Hardware event]
    //  bus-cycles                                         [Hardware event]
    //  cache-misses                                       [Hardware event]
    //  cache-references                                   [Hardware event]
    //  cpu-cycles OR cycles                               [Hardware event]
    //  instructions                                       [Hardware event]
    //  ref-cycles                                         [Hardware event]
    let missing_events: Vec<PerfEvent> = perf_events
        .iter()
        .filter(|&&event| {
            !stdout.lines().any(|line| {
                line.split_whitespace()
                    .any(|word| word == event.to_perf_string())
            })
        })
        .copied()
        .collect();

    if !missing_events.is_empty() {
        // TODO(COD-2011) Move this back to a warning once we handle x86 events
        debug!(
            "Not all required perf events available. Missing: [{}], disabling detailed event sampling.",
            missing_events.into_iter().join(", ")
        );
        return Ok(None);
    }

    let events_string = perf_events.into_iter().join(",");
    debug!("All required perf events available: {events_string}",);
    Ok(Some(format!("-e {{{events_string}}}")))
}

pub fn get_compression_flags<S: AsRef<Path>>(perf_executable: S) -> Result<Option<String>> {
    let output = Command::new(perf_executable.as_ref())
        .arg("version")
        .arg("--build-options")
        .output()
        .context("Failed to run perf version --build-options")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    debug!("Perf version build options:\n{stdout}");

    // Look for zstd compression support in the build options
    // Expected format: "                  zstd: [ on  ]  # HAVE_ZSTD_SUPPORT"
    let has_zstd = stdout
        .lines()
        .any(|line| line.to_lowercase().contains("zstd: [ on"));

    if has_zstd {
        debug!("perf supports zstd compression");
        if std::env::var("CODSPEED_PERF_DISABLE_COMPRESSION").is_ok() {
            info!("CODSPEED_PERF_DISABLE_COMPRESSION is set, disabling perf compression");
            return Ok(None);
        }
        // 3 is a widely adopted default level (AWS Athena, Python, ...)
        Ok(Some("--compression-level=3".to_string()))
    } else {
        warn!("perf does not support zstd compression");
        Ok(None)
    }
}
