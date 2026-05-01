use super::module_symbols::ModuleSymbols;
use libc::pid_t;
use runner_shared::unwind_data::{ProcessUnwindData, UnwindData};
use std::collections::HashMap;

/// A loaded ELF module discovered while parsing a profiler's sample stream.
///
/// Holds the symbol/unwind data extracted from the file plus the per-process
/// mounting metadata (load bias and rebased unwind data) for every pid that
/// mapped this module.
#[derive(Default)]
pub struct LoadedModule {
    /// Symbols extracted from the mapped ELF file
    pub module_symbols: Option<ModuleSymbols>,
    /// Unwind data extracted from the mapped ELF file
    pub unwind_data: Option<UnwindData>,
    /// Per-process mounting information
    pub process_loaded_modules: HashMap<pid_t, ProcessLoadedModule>,
}

#[derive(Default)]
pub struct ProcessLoadedModule {
    /// Load bias used to adjust declared elf addresses to their actual runtime addresses
    /// The bias is the difference between where the segment *actually* is in memory versus where the ELF file *preferred* it to be
    pub symbols_load_bias: Option<u64>,
    /// Unwind data specific to the process mounting, derived from both load bias and the actual unwind data
    pub process_unwind_data: Option<ProcessUnwindData>,
}

impl LoadedModule {
    pub fn pids(&self) -> impl Iterator<Item = pid_t> {
        self.process_loaded_modules.keys().copied()
    }
}
