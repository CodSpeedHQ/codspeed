/// Subset of perf events that CodSpeed supports.
///
/// Each variant is a semantic slot of the cache/execution model; the concrete
/// perf event chosen for it depends on the architecture (see
/// [`Self::to_perf_string`]).
#[derive(Debug, Clone, Copy)]
pub enum PerfEvent {
    CpuCycles,
    /// L1 data cache accesses.
    L1DCache,
    /// Accesses one level below L1: what L1 misses spill into. Hits in L1 are
    /// derived as `L1DCache - L2DCache`.
    L2DCache,
    /// Misses out of the last profiled cache level (i.e. trips to memory).
    /// Hits below L1 are derived as `L2DCache - CacheMisses`.
    CacheMisses,
    Instructions,
}

impl PerfEvent {
    /// Every perf event name that can back this slot, across all supported
    /// architectures. For parsers, which must handle profiles recorded on any
    /// architecture regardless of where they run.
    pub fn perf_strings(&self) -> &'static [&'static str] {
        match self {
            PerfEvent::CpuCycles => &["cpu-cycles"],
            PerfEvent::L1DCache => &["l1d_cache", "L1-dcache-loads"],
            PerfEvent::L2DCache => &["l2d_cache", "L1-dcache-load-misses"],
            PerfEvent::CacheMisses => &["l2d_cache_refill", "cache-misses"],
            PerfEvent::Instructions => &["instructions"],
        }
    }

    /// The perf event name backing this slot on the current architecture.
    ///
    /// On arm64 these are the architected PMU events (resolved through sysfs):
    /// `l2d_cache` counts all L2 accesses and `l2d_cache_refill` its misses.
    /// On x86_64 there is no generalized combined L2 event, so the slots are
    /// backed by the generalized cache events: L1 read misses stand in for
    /// "accesses below L1", and `cache-misses` (last-level misses) for trips
    /// to memory — lumping L2 and L3 hits together in the derived
    /// `L2DCache - CacheMisses`.
    pub fn to_perf_string(&self) -> &'static str {
        #[cfg(target_arch = "x86_64")]
        match self {
            PerfEvent::CpuCycles => "cpu-cycles",
            PerfEvent::L1DCache => "L1-dcache-loads",
            PerfEvent::L2DCache => "L1-dcache-load-misses",
            PerfEvent::CacheMisses => "cache-misses",
            PerfEvent::Instructions => "instructions",
        }
        #[cfg(not(target_arch = "x86_64"))]
        match self {
            PerfEvent::CpuCycles => "cpu-cycles",
            PerfEvent::L1DCache => "l1d_cache",
            PerfEvent::L2DCache => "l2d_cache",
            PerfEvent::CacheMisses => "l2d_cache_refill",
            PerfEvent::Instructions => "instructions",
        }
    }

    pub fn all_events() -> Vec<PerfEvent> {
        vec![
            PerfEvent::CpuCycles,
            PerfEvent::L1DCache,
            PerfEvent::L2DCache,
            PerfEvent::CacheMisses,
            PerfEvent::Instructions,
        ]
    }

    /// Architecture-independent name for this slot in samply profiles.
    ///
    /// samply labels each extra-event column with the name we pass it, so
    /// every architecture shares one name per slot and parsers match on it
    /// directly — unlike the perf integration, where columns carry the
    /// arch-specific event names of [`Self::perf_strings`].
    pub fn samply_name(&self) -> &'static str {
        match self {
            PerfEvent::CpuCycles => "cpu-cycles",
            PerfEvent::L1DCache => "l1d-cache",
            PerfEvent::L2DCache => "l2d-cache",
            PerfEvent::CacheMisses => "cache-misses",
            PerfEvent::Instructions => "instructions",
        }
    }

    /// The `<name>:<type>:<config>` spec for samply's `--perf-events`,
    /// resolving this slot to a concrete PMU event of the CPU we are running
    /// on. `None` when the slot has no suitable backing event on this CPU.
    pub fn to_samply_spec(&self) -> Option<String> {
        let (event_type, config) = self.perf_event_attr()?;
        Some(format!(
            "{}:{}:{:#x}",
            self.samply_name(),
            event_type,
            config
        ))
    }

    /// The `perf_event_attr` `(type, config)` encoding backing this slot on
    /// the current CPU.
    fn perf_event_attr(&self) -> Option<(u32, u64)> {
        // perf_event_attr type values from <linux/perf_event.h>.
        const PERF_TYPE_HARDWARE: u32 = 0;
        const PERF_TYPE_RAW: u32 = 4;
        match self {
            // Generalized hardware events, portable across architectures.
            PerfEvent::CpuCycles => Some((PERF_TYPE_HARDWARE, 0)),
            PerfEvent::Instructions => Some((PERF_TYPE_HARDWARE, 1)),
            _ => Some((PERF_TYPE_RAW, self.raw_cache_config()?)),
        }
    }

    /// Raw PMU encoding of this cache slot on x86_64: `umask << 8 | event`.
    ///
    /// Only Intel has a vetted selection; other vendors get no cache events.
    /// The events are picked so that each slot counts demand traffic of one
    /// consistent population, keeping the derived hit counts
    /// (`L1DCache - L2DCache`, `L2DCache - CacheMisses`) from underflowing
    /// the way mixed-population events (e.g. loads vs. all-cause line fills)
    /// can in store- or prefetch-heavy code.
    #[cfg(target_arch = "x86_64")]
    fn raw_cache_config(&self) -> Option<u64> {
        if !is_genuine_intel() {
            return None;
        }
        // Retired load instructions, by the cache level that served them:
        // MEM_INST_RETIRED.ALL_LOADS, MEM_LOAD_RETIRED.L1_MISS and
        // MEM_LOAD_RETIRED.L3_MISS. Demand loads only (stores and prefetches
        // don't count), encodings stable since Skylake.
        match self {
            PerfEvent::L1DCache => Some(0x81d0),
            PerfEvent::L2DCache => Some(0x08d1),
            PerfEvent::CacheMisses => Some(0x20d1),
            _ => None,
        }
    }

    /// Raw PMU encoding of this cache slot on arm64: the architected PMU
    /// event number (Arm ARM D8.11).
    #[cfg(target_arch = "aarch64")]
    fn raw_cache_config(&self) -> Option<u64> {
        match self {
            // L1D_CACHE: L1 data cache accesses, loads and stores.
            PerfEvent::L1DCache => Some(0x04),
            // L1D_CACHE_REFILL: L1D line fills. Defined against the same
            // access population as L1D_CACHE — unlike L2D_CACHE, which also
            // counts L1 write-backs, instruction-side refills and table
            // walks, and counts lines where L1D_CACHE counts operations —
            // so the `L1DCache - L2DCache` hit derivation stays sound.
            PerfEvent::L2DCache => Some(0x03),
            // L2D_CACHE_REFILL: refills of L2 or L1 from outside those
            // caches. On the Cortex-A72 macro-runner fleet (a1.metal) there
            // is no L3, so these are trips to DRAM. Includes instruction-side
            // refills, so it can exceed L1D_CACHE_REFILL in icache-missing
            // code; the derived hit counts saturate against that.
            PerfEvent::CacheMisses => Some(0x17),
            _ => None,
        }
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    fn raw_cache_config(&self) -> Option<u64> {
        None
    }
}

#[cfg(target_arch = "x86_64")]
fn is_genuine_intel() -> bool {
    use std::arch::x86_64::__cpuid;
    // CPUID leaf 0: vendor string in EBX,EDX,ECX.
    let leaf0 = unsafe { __cpuid(0) };
    let mut vendor = [0u8; 12];
    vendor[0..4].copy_from_slice(&leaf0.ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&leaf0.edx.to_le_bytes());
    vendor[8..12].copy_from_slice(&leaf0.ecx.to_le_bytes());
    &vendor == b"GenuineIntel"
}

impl std::fmt::Display for PerfEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_perf_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_slots_have_samply_specs() {
        assert_eq!(
            PerfEvent::CpuCycles.to_samply_spec().unwrap(),
            "cpu-cycles:0:0x0"
        );
        assert_eq!(
            PerfEvent::Instructions.to_samply_spec().unwrap(),
            "instructions:0:0x1"
        );
    }

    #[test]
    fn samply_names_are_unique() {
        let mut names: Vec<_> = PerfEvent::all_events()
            .iter()
            .map(|event| event.samply_name())
            .collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), PerfEvent::all_events().len());
    }

    #[test]
    fn print_specs_for_this_host() {
        for event in PerfEvent::all_events() {
            println!("{event:?} -> {:?}", event.to_samply_spec());
        }
    }
}
