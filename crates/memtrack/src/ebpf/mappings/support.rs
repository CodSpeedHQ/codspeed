use crate::kernel::KernelVersion;
use crate::prelude::*;

/// How the running kernel can resolve a mapped file's path inside BPF.
///
/// Only a BPF LSM program can do it at all: `bpf_d_path()` is restricted to
/// `BPF_TRACE_ITER` programs, sleepable LSM hooks and a fixed fentry allowlist
/// that contains no mmap path (`bpf_d_path_allowed()` in
/// `kernel/trace/bpf_trace.c`), and the `bpf_path_d_path()` kfunc that replaces
/// it rejects every program type but LSM (`bpf_fs_kfuncs_filter()` in
/// `fs/bpf_fs_kfuncs.c`).
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum MappingSupport {
    /// Paths cannot be resolved, so allocation stacks could not be attributed to
    /// modules and are not worth capturing.
    Unsupported,
    /// Sleepable LSM hook calling `bpf_d_path()` (kernel >= 5.11).
    Legacy,
    /// LSM hook calling the `bpf_path_d_path()` kfunc (kernel >= 6.12).
    Kfunc,
}

impl MappingSupport {
    /// What the running kernel and its boot configuration provide.
    ///
    /// The kernel release is only half the gate: `bpf` must also be in the
    /// active LSM list, which is fixed at boot by `CONFIG_LSM`/`lsm=` and cannot
    /// be inferred from the version.
    pub fn detect() -> Self {
        if !bpf_lsm_active() {
            info!(
                "The bpf LSM is not active (see /sys/kernel/security/lsm), so mapped module paths \
                 cannot be resolved"
            );
            return Self::Unsupported;
        }

        let version = match KernelVersion::current() {
            Ok(version) => version,
            Err(e) => {
                warn!("Failed to read the kernel version, no mapping records: {e:#}");
                return Self::Unsupported;
            }
        };

        let support = Self::for_version(version);
        match support {
            Self::Unsupported => {
                info!("Kernel {version} cannot resolve paths from an LSM program (needs >= 5.11)")
            }
            Self::Legacy => {
                debug!("Kernel {version} predates the bpf_path_d_path kfunc, using bpf_d_path")
            }
            Self::Kfunc => {}
        }
        support
    }

    fn for_version(version: KernelVersion) -> Self {
        if version < KernelVersion::new(5, 11) {
            return Self::Unsupported;
        }
        if version < KernelVersion::new(6, 12) {
            return Self::Legacy;
        }
        Self::Kfunc
    }
}

/// Whether `bpf` is one of the LSMs the running kernel initialized. An
/// unreadable file means securityfs is not mounted, in which case no LSM program
/// will attach either.
fn bpf_lsm_active() -> bool {
    const PATH: &str = "/sys/kernel/security/lsm";

    let Ok(active) = std::fs::read_to_string(PATH) else {
        debug!("Could not read {PATH} to check whether the bpf LSM is active");
        return false;
    };
    active.trim().split(',').any(|lsm| lsm == "bpf")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `bpf_lsm_mmap_file` has been in the sleepable hook set since 5.11, and
    /// 6.12 is the first release carrying `bpf_path_d_path`.
    #[test]
    fn maps_releases_to_support_levels() {
        for (major, minor, expected) in [
            (5, 4, MappingSupport::Unsupported),
            (5, 10, MappingSupport::Unsupported),
            (5, 11, MappingSupport::Legacy),
            (6, 11, MappingSupport::Legacy),
            (6, 12, MappingSupport::Kfunc),
            (7, 1, MappingSupport::Kfunc),
        ] {
            let version = KernelVersion::new(major, minor);
            assert_eq!(MappingSupport::for_version(version), expected, "{version}");
        }
    }
}
