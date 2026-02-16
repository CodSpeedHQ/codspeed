use anyhow::Context;
use libc::pid_t;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::BufWriter;
use std::path::Path;
use std::path::PathBuf;

use crate::debug_info::{MappedProcessDebugInfo, ModuleDebugInfo};
use crate::fifo::MarkerType;
use crate::module_symbols::MappedProcessModuleSymbols;
use crate::unwind_data::MappedProcessUnwindData;

#[derive(Serialize, Deserialize, Default)]
pub struct PerfMetadata {
    /// The version of this metadata format.
    pub version: u64,

    /// Name and version of the integration
    pub integration: (String, String),

    /// Per-pid modules that should be ignored, with runtime address ranges derived from symbol bounds + load bias
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub ignored_modules_by_pid: HashMap<pid_t, Vec<(String, u64, u64)>>,

    /// Deduplicated debug info entries, keyed by semantic key
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub debug_info: HashMap<String, ModuleDebugInfo>,

    /// Per-pid debug info references, mapping PID to mounted modules' debug info
    /// Referenced by `path_keys` that point to the deduplicated `debug_info` entries.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub mapped_process_debug_info_by_pid: HashMap<pid_t, Vec<MappedProcessDebugInfo>>,

    /// Per-pid unwind data references, mapping PID to mounted modules' unwind data
    /// Referenced by `path_keys` that point to the deduplicated `unwind_data` files on disk.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub mapped_process_unwind_data_by_pid: HashMap<pid_t, Vec<MappedProcessUnwindData>>,

    /// Per-pid symbol references, mapping PID to its mounted modules' symbols
    /// Referenced by `path_keys` that point to the deduplicated `symbols.map` files on disk.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub mapped_process_module_symbols: HashMap<pid_t, Vec<MappedProcessModuleSymbols>>,

    /// Mapping from semantic `path_key` to original binary path on host disk
    /// Used by `mapped_process_debug_info_by_pid`, `mapped_process_unwind_data_by_pid` and
    /// `mapped_process_module_symbols` the deduplicated entries
    ///
    /// Until now, only kept for traceability, if we ever need to reconstruct the original paths from the keys
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub path_key_to_path: HashMap<String, PathBuf>,

    // Deprecated fields below are kept for backward compatibility, since this struct is used in
    // the parser and older versions of the runner still generate them
    //
    /// The URIs of the benchmarks with the timestamps they were executed at.
    #[deprecated(note = "Use ExecutionTimestamps in the 'artifacts' module instead")]
    pub uri_by_ts: Vec<(u64, String)>,

    /// Modules that should be ignored and removed from the folded trace and callgraph (e.g. python interpreter)
    #[deprecated(note = "Use 'ignored_modules_by_pid' instead")]
    pub ignored_modules: Vec<(String, u64, u64)>,

    /// Marker for certain regions in the profiling data
    #[deprecated(note = "Use ExecutionTimestamps in the 'artifacts' module instead")]
    pub markers: Vec<MarkerType>,

    /// Kept for backward compatibility, was used before deduplication of debug info entries.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    #[deprecated(note = "Use 'debug_info' + 'mapped_process_debug_info_by_pid' instead")]
    pub debug_info_by_pid: HashMap<pid_t, Vec<ModuleDebugInfo>>,
}

impl PerfMetadata {
    pub fn from_reader<R: std::io::Read>(reader: R) -> anyhow::Result<Self> {
        serde_json::from_reader(reader).context("Could not parse perf metadata from JSON")
    }

    pub fn save_to<P: AsRef<Path>>(&self, path: P) -> anyhow::Result<()> {
        let file = std::fs::File::create(path.as_ref().join("perf.metadata"))?;
        const BUFFER_SIZE: usize = 256 * 1024 /* 256 KB */;

        let writer = BufWriter::with_capacity(BUFFER_SIZE, file);
        serde_json::to_writer(writer, self)?;
        Ok(())
    }
}
