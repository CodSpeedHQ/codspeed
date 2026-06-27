use linux_perf_event_reader::constants::{
    PERF_COUNT_HW_CPU_CYCLES, PERF_COUNT_HW_INSTRUCTIONS, PERF_TYPE_HARDWARE, PERF_TYPE_RAW,
};

/// Subset of perf events that CodSpeed supports.
///
/// Each variant is a semantic slot of the cache/execution model, named by
/// [`Self::to_perf_string`] and backed by a concrete PMU event resolved for
/// the current CPU (see [`Self::to_samply_spec`]).
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
    /// The event name backing this slot.
    pub fn to_perf_string(&self) -> &'static str {
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

    /// The `<name>:<type>:<config>` spec for samply's `--perf-events`,
    /// resolving this slot to a concrete PMU event of the CPU we are running
    /// on.
    ///
    /// `None` when the slot has no suitable backing event on this CPU.
    /// The column is labelled with [`Self::to_perf_string`] so samply profiles
    /// carry the same event names as perf ones and parse through one path.
    pub fn to_samply_spec(&self) -> Option<String> {
        let (event_type, config) = self.perf_event_attr()?;
        Some(format!(
            "{}:{}:{:#x}",
            self.to_perf_string(),
            event_type,
            config
        ))
    }

    /// The `perf_event_attr` `(type, config)` encoding backing this slot on
    /// the current CPU.
    fn perf_event_attr(&self) -> Option<(u32, u64)> {
        match self {
            // Generalized hardware events, portable across architectures.
            PerfEvent::CpuCycles => Some((PERF_TYPE_HARDWARE, PERF_COUNT_HW_CPU_CYCLES.into())),
            PerfEvent::Instructions => {
                Some((PERF_TYPE_HARDWARE, PERF_COUNT_HW_INSTRUCTIONS.into()))
            }
            _ => Some((PERF_TYPE_RAW, self.raw_cache_config()?)),
        }
    }

    /// Raw PMU encoding of this cache slot on x86_64: `umask << 8 | event`,
    /// the layout the kernel expects in `perf_event_attr.config` for
    /// `PERF_TYPE_RAW`.
    ///
    /// Only Intel has a vetted selection; other vendors get no cache events.
    /// EventCode/UMask come from Intel's perfmon tables, listed per mnemonic in
    /// the Skylake-X core event file
    /// (<https://github.com/intel/perfmon/blob/main/SKX/events/skylakex_core.json>),
    /// stable since Skylake.
    #[cfg(target_arch = "x86_64")]
    fn raw_cache_config(&self) -> Option<u64> {
        if !is_genuine_intel() {
            // Not tested on AMD or other x86_64 vendors yet
            return None;
        }
        // Retired load instructions, by the cache level that served them
        // (demand loads only; stores and prefetches don't count).
        match self {
            // MEM_INST_RETIRED.ALL_LOADS: 0xD0 | 0x81 << 8
            PerfEvent::L1DCache => Some(0x81d0),
            // MEM_LOAD_RETIRED.L1_MISS: 0xD1 | 0x08 << 8
            PerfEvent::L2DCache => Some(0x08d1),
            // MEM_LOAD_RETIRED.L3_MISS: 0xD1 | 0x20 << 8
            PerfEvent::CacheMisses => Some(0x20d1),
            _ => None,
        }
    }

    /// Raw PMU encoding of this cache slot on arm64: the architected PMU event
    /// number, used directly as `perf_event_attr.config` for `PERF_TYPE_RAW`.
    ///
    /// These are common (architected) event numbers, listed per mnemonic in
    /// Arm's PMU event table for the Cortex-A72 fleet
    /// (<https://github.com/ARM-software/data/blob/master/pmu/cortex-a72.json>).
    #[cfg(target_arch = "aarch64")]
    fn raw_cache_config(&self) -> Option<u64> {
        match self {
            // L1D_CACHE (0x04): L1 data cache accesses, loads and stores.
            PerfEvent::L1DCache => Some(0x04),
            // L1D_CACHE_REFILL (0x03): L1D line fills. Defined against the same
            // access population as L1D_CACHE — unlike L2D_CACHE, which also
            // counts L1 write-backs, instruction-side refills and table
            // walks, and counts lines where L1D_CACHE counts operations —
            // so the `L1DCache - L2DCache` hit derivation stays sound.
            PerfEvent::L2DCache => Some(0x03),
            // L2D_CACHE_REFILL (0x17): refills of L2 or L1 from outside those
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
    fn event_names_are_unique() {
        let mut names: Vec<_> = PerfEvent::all_events()
            .iter()
            .map(|event| event.to_perf_string())
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
