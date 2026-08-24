//! Kernel controls that stabilise memory measurements:
//!
//! - [transparent huge pages](https://docs.kernel.org/admin-guide/mm/transhuge.html)
//!   can allocate a 2 MiB page when a benchmark touches a small part of a
//!   mapping, making its RSS depend on page-promotion timing.
//! - swap lets the kernel evict anonymous pages, so a benchmark's resident set
//!   reflects reclaim decisions rather than what it allocated.
//! - [`vm.drop_caches`](https://docs.kernel.org/admin-guide/sysctl/vm.html)
//!   clears clean page cache and reclaimable slab objects, giving each run the
//!   same cache state.
//!
//! [`MemoryTunables`] captures the previous settings and restores them on
//! drop, so a host that only looks like CI — `CI=true` inside a container
//! sharing the host's non-namespaced knobs, say — is left as it was.

use crate::executor::helpers::run_with_sudo::{can_elevate_without_prompt, run_with_sudo};
use crate::prelude::*;
use std::fs::read_to_string;

/// Guard holding the settings that were changed.
/// Empty when nothing was changed, making [`Drop`] a no-op.
#[derive(Debug)]
#[must_use = "the knobs are restored as soon as the guard is dropped"]
pub struct MemoryTunables {
    /// THP knob path -> the mode it held before.
    thp: Vec<(String, String)>,
    /// Swap areas that were turned off, re-enabled one by one on drop.
    swap: Vec<SwapArea>,
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
            swap: Self::disable_swap(),
        };
        // After swapoff: faulting the swapped-out pages back in dirties the cache.
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

    /// Turns off the swap areas that are safe to turn off, returning the ones
    /// that have to be turned back on.
    fn disable_swap() -> Vec<SwapArea> {
        let Ok(swaps) = read_to_string(PROC_SWAPS) else {
            debug!("{PROC_SWAPS} is missing, skipping swap");
            return Vec::new();
        };
        let areas = SwapArea::parse_all(&swaps);
        if areas.is_empty() {
            debug!("No swap area is active, skipping swap");
            return Vec::new();
        }

        let Some(available_kib) = read_to_string("/proc/meminfo")
            .ok()
            .as_deref()
            .and_then(mem_available_kib)
        else {
            warn!("Cannot read MemAvailable, leaving swap enabled");
            return Vec::new();
        };

        let mut disabled = Vec::new();
        for area in SwapArea::to_disable(areas, available_kib) {
            match run_with_sudo("swapoff", [&area.path]) {
                Ok(()) => disabled.push(area),
                Err(error) => warn!("Failed to disable swap on {}: {error}", area.path),
            }
        }

        disabled
    }
}

impl Drop for MemoryTunables {
    fn drop(&mut self) {
        if self.thp.is_empty() && self.swap.is_empty() {
            return;
        }

        start_group!("Restoring kernel memory tunables");
        // `swapon -a` would only cover the areas listed in /etc/fstab, so a swap
        // file activated by hand would never come back. Restoring in the order
        // /proc/swaps listed them also gives the areas whose priority the kernel
        // picks back their original relative order.
        for area in &self.swap {
            let argv = area.swapon_argv();
            if let Err(error) = run_with_sudo("swapon", &argv) {
                warn!(
                    "Failed to re-enable swap on {}, re-run manually with `sudo swapon {}`: {error}",
                    area.path,
                    argv.join(" ")
                );
            }
        }
        for (path, value) in &self.thp {
            if let Err(error) = write_root_file(path, value) {
                warn!("Failed to restore transparent huge pages ({path}) to {value}: {error}");
            }
        }
        end_group!();
    }
}

const PROC_SWAPS: &str = "/proc/swaps";

/// An active swap area, as listed by `/proc/swaps`.
#[derive(Debug, PartialEq, Eq)]
struct SwapArea {
    path: String,
    /// How much of the area currently holds evicted pages.
    used_kib: u64,
    /// Reclaim order: higher areas are used first. Negative when the kernel
    /// assigned it rather than the caller.
    priority: i32,
}

impl SwapArea {
    /// Parses `/proc/swaps`, whose rows read as `Filename Type Size Used Priority`.
    fn parse_all(content: &str) -> Vec<Self> {
        content
            .lines()
            .skip(1)
            .filter_map(|line| {
                let mut columns = line.split_whitespace();
                let path = columns.next()?.to_string();
                // Type and Size sit between Filename and Used.
                let used_kib = columns.nth(2)?.parse().ok()?;
                let priority = columns.next()?.parse().ok()?;

                Some(Self {
                    path,
                    used_kib,
                    priority,
                })
            })
            .collect()
    }

    /// `swapon` arguments re-enabling the area with the priority it had.
    /// `SWAP_FLAG_PREFER` only encodes `0..=32767`, so a kernel-assigned
    /// negative priority cannot be asked for and is left to be assigned again.
    fn swapon_argv(&self) -> Vec<String> {
        let mut argv = Vec::new();
        if self.priority >= 0 {
            argv.push("-p".to_string());
            argv.push(self.priority.to_string());
        }
        argv.push(self.path.clone());

        argv
    }

    /// Selects the areas that can be turned off without risking the host:
    /// `swapoff` faults every evicted page back into RAM, so it needs the
    /// resident set to fit.
    fn to_disable(areas: Vec<Self>, mem_available_kib: u64) -> Vec<Self> {
        let (candidates, zram): (Vec<_>, Vec<_>) =
            areas.into_iter().partition(|area| !area.is_zram());
        for area in &zram {
            warn!(
                "Leaving zram swap {} enabled: disabling it would reset its size",
                area.path
            );
        }

        let used_kib: u64 = candidates.iter().map(|area| area.used_kib).sum();
        if used_kib >= mem_available_kib {
            warn!(
                "Leaving swap enabled: faulting {used_kib} kB of evicted pages back in does not fit in {mem_available_kib} kB of available memory"
            );
            return Vec::new();
        }

        candidates
    }

    /// zram areas are compressed swap backed by RAM. `swapoff` resets their
    /// `disksize` to 0, after which a plain `swapon` fails with `EINVAL` and
    /// only restarting the zram unit brings them back.
    fn is_zram(&self) -> bool {
        self.path.starts_with("/dev/zram")
    }
}

/// The `MemAvailable` line of `/proc/meminfo`, which reads as `MemAvailable: 123 kB`.
fn mem_available_kib(meminfo: &str) -> Option<u64> {
    meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemAvailable:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
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

    fn area(path: &str, used_kib: u64, priority: i32) -> SwapArea {
        SwapArea {
            path: path.to_string(),
            used_kib,
            priority,
        }
    }

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

    #[test]
    fn parses_the_active_swap_areas() {
        let content = "Filename\t\t\t\tType\t\tSize\t\tUsed\t\tPriority\n\
                       /swapfile                               file\t\t8388604\t\t131072\t\t-2\n\
                       /dev/zram0                              partition\t4194300\t\t0\t\t100\n";

        assert_eq!(
            SwapArea::parse_all(content),
            vec![area("/swapfile", 131072, -2), area("/dev/zram0", 0, 100)]
        );
    }

    #[test]
    fn reports_no_swap_area_when_swap_is_off() {
        let content = "Filename\t\t\t\tType\t\tSize\t\tUsed\t\tPriority\n";

        assert_eq!(SwapArea::parse_all(content), vec![]);
    }

    #[test]
    fn reads_the_available_memory() {
        let meminfo = "MemTotal:       16316360 kB\nMemAvailable:    9583756 kB\n";

        assert_eq!(mem_available_kib(meminfo), Some(9583756));
    }

    #[test]
    fn disables_a_swap_file_whose_pages_fit_in_memory() {
        let areas = vec![area("/swapfile", 131072, -2)];

        assert_eq!(
            SwapArea::to_disable(areas, 9583756),
            vec![area("/swapfile", 131072, -2)]
        );
    }

    #[test]
    fn never_disables_zram_swap() {
        let areas = vec![area("/dev/zram0", 0, 100)];

        assert_eq!(SwapArea::to_disable(areas, 9583756), vec![]);
    }

    #[test]
    fn keeps_swap_enabled_when_its_pages_do_not_fit_in_memory() {
        let areas = vec![
            area("/swapfile", 4194304, -2),
            area("/swapfile2", 4194304, -3),
        ];

        assert_eq!(SwapArea::to_disable(areas, 1048576), vec![]);
    }

    #[test]
    fn asks_for_the_priority_the_kernel_did_not_pick() {
        assert_eq!(
            area("/swapfile", 0, 10).swapon_argv(),
            vec!["-p", "10", "/swapfile"]
        );
    }

    #[test]
    fn leaves_a_kernel_assigned_priority_to_be_reassigned() {
        assert_eq!(area("/swapfile", 0, -2).swapon_argv(), vec!["/swapfile"]);
    }
}
