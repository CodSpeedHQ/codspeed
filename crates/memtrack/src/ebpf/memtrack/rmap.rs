use crate::kernel::KernelVersion;
use crate::prelude::*;

/// How much of the folio rmap hook set the running kernel can attach. The hooks
/// are paired add/remove accounting, so a level is all-or-nothing: attaching
/// adds without removes makes reconstructed RSS grow monotonically.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RmapSupport {
    /// No rmap hooks attach; RSS comes from the rss_stat tracepoint alone.
    Unsupported,
    /// PTE and PMD pairs (kernel >= 6.8)
    Core,
    /// adds the PUD pair (kernel >= 6.15)
    CoreAndPud,
}

impl RmapSupport {
    /// What the running kernel provides. Never fails: a kernel that predates the
    /// folio rmap API, or whose version cannot be read, reports `Unsupported`
    /// and the hooks are left unattached.
    ///
    /// The release is a precise gate here because every target is an
    /// unconditional definition in `mm/rmap.c` — only the bodies are `#ifdef`-ed
    /// on `CONFIG_TRANSPARENT_HUGEPAGE` — so the symbols exist on any kernel new
    /// enough to declare them, whatever the arch or config. On an arch without
    /// PUD THP the PUD pair attaches and simply never fires.
    pub fn detect() -> Self {
        let version = match KernelVersion::current() {
            Ok(version) => version,
            Err(e) => {
                warn!("Failed to read the kernel version, no rmap hooks: {e:#}");
                return Self::Unsupported;
            }
        };

        let support = Self::for_version(version);
        match support {
            Self::Unsupported => info!("Kernel {version} has no folio rmap hooks (needs >= 6.8)"),
            Self::Core => debug!(
                "Kernel {version} has no PUD rmap pair (needs >= 6.15); PUD-mapped THP unaccounted"
            ),
            Self::CoreAndPud => {}
        }
        support
    }

    fn for_version(version: KernelVersion) -> Self {
        if version < KernelVersion::new(6, 8) {
            return Self::Unsupported;
        }
        if version < KernelVersion::new(6, 15) {
            return Self::Core;
        }
        Self::CoreAndPud
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The predecessors of the folio hooks were removed in the same release that
    /// introduced them, so 6.8 is a hard edge with no mixed window; 6.15 adds the
    /// PUD pair to an otherwise complete set.
    #[test]
    fn maps_releases_to_support_levels() {
        for (major, minor, expected) in [
            (5, 4, RmapSupport::Unsupported),
            (5, 15, RmapSupport::Unsupported),
            (6, 7, RmapSupport::Unsupported),
            (6, 8, RmapSupport::Core),
            (6, 11, RmapSupport::Core),
            (6, 14, RmapSupport::Core),
            (6, 15, RmapSupport::CoreAndPud),
            (7, 1, RmapSupport::CoreAndPud),
        ] {
            let version = KernelVersion::new(major, minor);
            assert_eq!(
                RmapSupport::for_version(version),
                expected,
                "kernel {version}"
            );
        }
    }
}
