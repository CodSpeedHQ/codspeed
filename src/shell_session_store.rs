//! Shell-session-scoped key/value state, keyed by the parent shell's PID.
//!
//! State is written to `$XDG_RUNTIME_DIR/<kind>/<parent_pid>` (or the system
//! temp dir if `XDG_RUNTIME_DIR` is unset). Loading walks up the process tree
//! until a registered file is found, so the value is shared across subshells
//! of the shell that registered it.

use crate::prelude::*;
use libc::pid_t;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;
use sysinfo::Pid;
use sysinfo::ProcessRefreshKind;
use sysinfo::RefreshKind;
use sysinfo::System;

/// Registry of the shell-session state kinds the CLI uses. Each variant
/// maps to its own subdirectory under the runtime root, so different
/// kinds never collide.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SessionKind {
    /// Active runner mode(s) set by `codspeed use <mode>`.
    Mode,
    /// Active profile set by `codspeed profile use <name>`.
    Profile,
}

impl SessionKind {
    fn as_dir_name(self) -> &'static str {
        match self {
            SessionKind::Mode => "codspeed_use_mode",
            SessionKind::Profile => "codspeed_profile",
        }
    }
}

static SYSTEM: OnceLock<System> = OnceLock::new();

fn get_root_dir(kind: SessionKind) -> PathBuf {
    let base_dir = if let Some(xdg_runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        PathBuf::from(xdg_runtime_dir)
    } else {
        std::env::temp_dir()
    };

    base_dir.join(kind.as_dir_name())
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

fn get_state_file_path(base_dir: &Path, pid: pid_t) -> PathBuf {
    base_dir.join(pid.to_string())
}

/// Persist `value` for the current shell session (keyed by the parent PID of
/// this process).
pub(crate) fn register<T: Serialize>(kind: SessionKind, value: &T) -> Result<()> {
    let dir = get_root_dir(kind);
    std::fs::create_dir_all(&dir)?;

    let parent_pid =
        get_parent_pid(std::process::id() as pid_t).context("Could not determine parent PID")?;

    let path = get_state_file_path(&dir, parent_pid);
    std::fs::write(path, serde_json::to_string(value)?)?;
    Ok(())
}

/// Look up a previously-registered value by walking up the process tree from
/// this process. Returns `None` if no ancestor has registered a value.
pub(crate) fn load<T: DeserializeOwned>(kind: SessionKind) -> Result<Option<T>> {
    let dir = get_root_dir(kind);
    let mut current_pid = std::process::id() as pid_t;

    while let Some(parent_pid) = get_parent_pid(current_pid) {
        let path = get_state_file_path(&dir, parent_pid);
        if path.exists() {
            let raw = std::fs::read_to_string(path)?;
            let value: T = serde_json::from_str(&raw)?;
            return Ok(Some(value));
        }
        current_pid = parent_pid;
    }

    Ok(None)
}
