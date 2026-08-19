//! Kernel controls that stabilise memory measurements:
//!
//! - [transparent huge pages](https://docs.kernel.org/admin-guide/mm/transhuge.html)
//!   can allocate a 2 MiB page when a benchmark touches a small part of a
//!   mapping, making its RSS depend on page-promotion timing.
//! - [`vm.drop_caches`](https://docs.kernel.org/admin-guide/sysctl/vm.html)
//!   clears clean page cache and reclaimable slab objects, giving each run the
//!   same cache state.
//!
//! [`MemoryTunables`] captures the previous THP setting and restores it on
//! drop, so a host that only looks like CI — `CI=true` inside a container
//! sharing the host's non-namespaced knobs, say — is left as it was.

use crate::executor::helpers::run_with_sudo::{can_elevate_without_prompt, run_with_sudo};
use crate::prelude::*;
use std::fs::read_to_string;

/// Guard holding the previous THP setting when it was changed.
/// Empty when THP was not changed, making [`Drop`] a no-op.
#[derive(Debug)]
#[must_use = "the knobs are restored as soon as the guard is dropped"]
pub struct MemoryTunables {
    /// THP knob path -> the mode it held before.
    thp: Vec<(String, String)>,
}

impl MemoryTunables {
    /// Applies the controls on a best-effort basis: a control that cannot be
    /// set is warned about, never fatal.
    pub fn apply() -> Option<Self> {
        // Blocking the run on an interactive password prompt would be worse than
        // measuring without the knobs.
        if !can_elevate_without_prompt() {
            warn!(
                "Cannot elevate privileges without a password prompt, skipping kernel memory tunables"
            );
            return None;
        }

        start_group!("Applying kernel memory tunables");
        let tunables = Self {
            thp: Self::set_thp_enabled("never"),
        };
        Self::drop_page_cache();
        end_group!();

        Some(tunables)
    }

    /// Drops the page cache. Nothing to restore: the node is a write-only
    /// trigger and the kernel refills the cache on demand.
    fn drop_page_cache() {
        // drop_caches only reclaims clean objects; flush dirty buffers first.
        nix::unistd::sync();
        if let Err(error) = write_root_file("/proc/sys/vm/drop_caches", "3") {
            warn!("Failed to drop the page cache: {error}");
        }
    }

    /// Sets the THP default mode, returning the prior mode when it changed.
    fn set_thp_enabled(value: &str) -> Vec<(String, String)> {
        let mut previous = Vec::new();

        let path = "/sys/kernel/mm/transparent_hugepage/enabled";
        let Some(active) = read_thp_mode(path) else {
            debug!("{path} is missing or has no active mode, skipping");
            return previous;
        };
        if active == value {
            return previous;
        }

        match write_root_file(path, value) {
            Ok(()) => previous.push((path.to_string(), active)),
            Err(error) => warn!("Failed to set transparent huge pages ({path}): {error}"),
        }

        previous
    }
}

impl Drop for MemoryTunables {
    fn drop(&mut self) {
        if self.thp.is_empty() {
            return;
        }

        start_group!("Restoring kernel memory tunables");
        for (path, value) in &self.thp {
            if let Err(error) = write_root_file(path, value) {
                warn!("Failed to restore transparent huge pages ({path}) to {value}: {error}");
            }
        }
        end_group!();
    }
}

/// The active mode of a THP knob, whose value reads as `always [madvise] never`.
fn read_thp_mode(path: &str) -> Option<String> {
    let content = read_to_string(path).ok()?;
    let mode = content
        .split_whitespace()
        .find_map(|token| token.strip_prefix('[')?.strip_suffix(']'))?;

    Some(mode.to_string())
}

/// Write to a root-owned /proc or /sys node. `run_with_sudo` cannot pipe stdin,
/// so the redirect happens inside a shell instead of `sudo tee`.
fn write_root_file(path: &str, value: &str) -> Result<()> {
    run_with_sudo("sh", ["-c", &format!("printf '%s' {value} > {path}")])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_active_thp_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enabled");
        std::fs::write(&path, "always [madvise] never\n").unwrap();

        assert_eq!(
            read_thp_mode(path.to_str().unwrap()),
            Some("madvise".to_string())
        );
    }

    #[test]
    fn reports_no_thp_mode_when_none_is_active() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enabled");
        std::fs::write(&path, "always madvise never\n").unwrap();

        assert_eq!(read_thp_mode(path.to_str().unwrap()), None);
    }
}
