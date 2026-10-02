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

/// Every placement of a module in one process. A process can map the same file
/// more than once at different addresses, and each placement has its own load
/// bias.
#[derive(Default, Clone)]
pub struct ProcessLoadedModule {
    /// Load biases used to adjust declared elf addresses to their actual runtime addresses, one
    /// per distinct placement. A bias is the difference between where the segment *actually* is in
    /// memory versus where the ELF file *preferred* it to be
    pub symbols_load_biases: Vec<u64>,
    /// Unwind data of each executable mapping, derived from both load bias and the actual unwind data
    pub process_unwind_data: Vec<ProcessUnwindData>,
}

impl ProcessLoadedModule {
    pub fn add_load_bias(&mut self, load_bias: u64) {
        if !self.symbols_load_biases.contains(&load_bias) {
            self.symbols_load_biases.push(load_bias);
        }
    }
}

impl LoadedModule {
    pub fn pids(&self) -> impl Iterator<Item = pid_t> {
        self.process_loaded_modules.keys().copied()
    }
}
