use serde::{Deserialize, Serialize};

#[derive(Hash, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct DebugInfo {
    pub addr: u64,
    pub size: u64,
    pub name: String,
    pub file: String,
    pub line: Option<u32>,
}

impl std::fmt::Debug for DebugInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let location = format!("{}:{}", self.file, self.line.unwrap_or(0));
        let name = &self.name;
        write!(
            f,
            "DebugInfo {{ addr: {:x}, size: {:x}, name: {}, location: {} }}",
            self.addr, self.size, name, location
        )
    }
}

/// Per-pid mounting info referencing a deduplicated debug info entry.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MappedProcessDebugInfo {
    pub debug_info_key: String,
    pub load_bias: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleDebugInfo {
    /// The path to the object file on disk (e.g. `/usr/lib/libc.so.6`)
    pub object_path: String,

    /// The minimum and maximum address covered by the debug infos. This is useful for
    /// quickly checking if an address might be covered by this module.
    pub addr_bounds: (u64, u64),

    /// The load bias of the module. This is the difference between the address in the
    /// symbol table and the actual address in memory.
    pub load_bias: u64,

    /// The debug info for this module, sorted by address.
    pub debug_infos: Vec<DebugInfo>,
}
