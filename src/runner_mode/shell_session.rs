use super::RunnerMode;
use crate::prelude::*;
use libc::pid_t;
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;
use sysinfo::Pid;
use sysinfo::ProcessRefreshKind;
use sysinfo::RefreshKind;
use sysinfo::System;

static SYSTEM: OnceLock<System> = OnceLock::new();

/// Get the root directory where the use mode is stored
/// If available, uses `$XDG_RUNTIME_DIR/codspeed_use_mode`
/// Otherwise, falls back to `std::env::temp_dir()/codspeed_use_mode`
fn get_use_mode_root_dir() -> PathBuf {
    let base_dir = if let Some(xdg_runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        PathBuf::from(xdg_runtime_dir)
    } else {
        std::env::temp_dir()
    };

    base_dir.join("codspeed_use_mode")
}

fn get_parent_pid(pid: pid_t) -> Option<pid_t> {
    let s = SYSTEM.get_or_init(|| {
        System::new_with_specifics(
            RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
        )
    });

    let current_pid = Pid::from_u32(pid as u32);

    s.process(current_pid)
        .and_then(|p| p.parent())
        .map(|pid| pid.as_u32() as pid_t)
}

fn get_mode_file_path(base_dir: &Path, pid: pid_t) -> PathBuf {
    base_dir.join(pid.to_string())
}

pub(crate) fn register_shell_session_mode(mode: &[RunnerMode]) -> Result<()> {
    let use_mode_dir = get_use_mode_root_dir();
    std::fs::create_dir_all(&use_mode_dir)?;

    let Some(parent_pid) = get_parent_pid(std::process::id() as pid_t) else {
        return Err(anyhow!("Could not determine parent PID"));
    };

    let mode_file_path = get_mode_file_path(&use_mode_dir, parent_pid);

    std::fs::write(mode_file_path, serde_json::to_string(mode)?)?;
    Ok(())
}

pub(crate) fn load_shell_session_mode() -> Result<Vec<RunnerMode>> {
    // Go up the process tree until we find a registered mode
    let mut current_pid = std::process::id() as pid_t;

    while let Some(parent_pid) = get_parent_pid(current_pid) {
        let use_mode_dir = get_use_mode_root_dir();
        let mode_file_path = get_mode_file_path(&use_mode_dir, parent_pid);

        if mode_file_path.exists() {
            let mode_str = std::fs::read_to_string(mode_file_path)?;
            let mode: Vec<RunnerMode> = serde_json::from_str(&mode_str)?;
            return Ok(mode);
        }

        current_pid = parent_pid;
    }

    Ok(Vec::new())
}
