use serde::{Deserialize, Serialize};

/// File suffix used when registering module symbols in a PID agnostic way.
pub const SYMBOLS_MAP_SUFFIX: &str = "symbols.map";

/// Per-pid mounting info referencing a deduplicated perf map entry.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MappedProcessModuleSymbols {
    pub perf_map_key: String,
    pub load_bias: u64,
}
