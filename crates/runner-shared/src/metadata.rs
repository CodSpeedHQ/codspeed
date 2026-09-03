use anyhow::Context;
use libc::pid_t;
use serde::de::{Deserializer, Error};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::BufWriter;
use std::path::Path;
use std::path::PathBuf;

use crate::debug_info::{MappedProcessDebugInfo, ModuleDebugInfo};
use crate::fifo::MarkerType;
use crate::module_symbols::MappedProcessModuleSymbols;
use crate::unwind_data::MappedProcessUnwindData;

/// Reads a pid-keyed map whose keys arrive as strings.
///
/// JSON object keys are always strings. serde_json's direct deserializer
/// special-cases that and parses integer map keys, but a `#[serde(flatten)]`
/// field is buffered into serde's internal `Content` first, and that path has
/// no such special case — a `pid_t` key then fails with `invalid type: string`.
/// See <https://github.com/serde-rs/serde/issues/1183>.
///
/// Only the read side needs this: serializing writes the same bytes either way.
fn pid_keys_from_strings<'de, V, D>(deserializer: D) -> Result<HashMap<pid_t, V>, D::Error>
where
    V: Deserialize<'de>,
    D: Deserializer<'de>,
{
    HashMap::<String, V>::deserialize(deserializer)?
        .into_iter()
        .map(|(key, value)| {
            let pid = key
                .parse::<pid_t>()
                .map_err(|_| D::Error::custom(format!("invalid pid key: {key}")))?;
            Ok((pid, value))
        })
        .collect()
}

/// The per-profile module artifacts: the deduplicated debug info, unwind data
/// and symbol tables extracted from the ELF modules the profiled processes
/// mapped, plus the per-pid references into them.
///
/// Flattened into every metadata format, so all profiling modes describe their
/// modules identically.
#[derive(Serialize, Deserialize, Default)]
pub struct ModuleArtifacts {
    /// Deduplicated debug info entries, keyed by semantic key
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub debug_info: HashMap<String, ModuleDebugInfo>,

    /// Per-pid debug info references, mapping PID to mounted modules' debug info
    /// Referenced by `path_keys` that point to the deduplicated `debug_info` entries.
    #[serde(
        default,
        skip_serializing_if = "HashMap::is_empty",
        deserialize_with = "pid_keys_from_strings"
    )]
    pub mapped_process_debug_info_by_pid: HashMap<pid_t, Vec<MappedProcessDebugInfo>>,

    /// Per-pid unwind data references, mapping PID to mounted modules' unwind data
    /// Referenced by `path_keys` that point to the deduplicated `unwind_data` files on disk.
    #[serde(
        default,
        skip_serializing_if = "HashMap::is_empty",
        deserialize_with = "pid_keys_from_strings"
    )]
    pub mapped_process_unwind_data_by_pid: HashMap<pid_t, Vec<MappedProcessUnwindData>>,

    /// Per-pid symbol references, mapping PID to its mounted modules' symbols
    /// Referenced by `path_keys` that point to the deduplicated `symbols.map` files on disk.
    #[serde(
        default,
        skip_serializing_if = "HashMap::is_empty",
        deserialize_with = "pid_keys_from_strings"
    )]
    pub mapped_process_module_symbols: HashMap<pid_t, Vec<MappedProcessModuleSymbols>>,

    /// Mapping from semantic `path_key` to original binary path on host disk
    /// Used by `mapped_process_debug_info_by_pid`, `mapped_process_unwind_data_by_pid` and
    /// `mapped_process_module_symbols` the deduplicated entries
    ///
    /// Until now, only kept for traceability, if we ever need to reconstruct the original paths from the keys
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub path_key_to_path: HashMap<String, PathBuf>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct WalltimeMetadata {
    /// The version of this metadata format.
    pub version: u64,

    /// Name and version of the integration
    pub integration: (String, String),

    /// Per-pid modules that should be ignored, with runtime address ranges derived from symbol bounds + load bias
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub ignored_modules_by_pid: HashMap<pid_t, Vec<(String, u64, u64)>>,

    #[serde(flatten)]
    pub artifacts: ModuleArtifacts,

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

impl WalltimeMetadata {
    pub fn from_reader<R: std::io::Read>(reader: R) -> anyhow::Result<Self> {
        serde_json::from_reader(reader).context("Could not parse walltime metadata from JSON")
    }

    pub fn save_to<P: AsRef<Path>>(&self, path: P) -> anyhow::Result<()> {
        let file = std::fs::File::create(path.as_ref().join("walltime.metadata"))?;
        const BUFFER_SIZE: usize = 256 * 1024 /* 256 KB */;

        let writer = BufWriter::with_capacity(BUFFER_SIZE, file);
        serde_json::to_writer(writer, self)?;
        Ok(())
    }
}

/// Companion to the memtrack event stream: the modules its allocation stacks
/// resolve against. Memory mode records benchmark boundaries in
/// `ExecutionTimestamps`, so unlike [`WalltimeMetadata`] it carries no markers.
#[derive(Serialize, Deserialize, Default)]
pub struct MemtrackMetadata {
    /// The version of this metadata format.
    pub version: u64,

    /// Name and version of the integration
    pub integration: (String, String),

    #[serde(flatten)]
    pub artifacts: ModuleArtifacts,
}

impl MemtrackMetadata {
    pub const CURRENT_VERSION: u64 = 1;

    pub fn new(integration: (String, String), artifacts: ModuleArtifacts) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            integration,
            artifacts,
        }
    }

    pub fn from_reader<R: std::io::Read>(reader: R) -> anyhow::Result<Self> {
        serde_json::from_reader(reader).context("Could not parse memtrack metadata from JSON")
    }

    pub fn save_to<P: AsRef<Path>>(&self, path: P) -> anyhow::Result<()> {
        let file = std::fs::File::create(path.as_ref().join("memtrack.metadata"))?;
        const BUFFER_SIZE: usize = 256 * 1024 /* 256 KB */;

        let writer = BufWriter::with_capacity(BUFFER_SIZE, file);
        serde_json::to_writer(writer, self)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from the flat `WalltimeMetadata` that predates
    /// [`ModuleArtifacts`]: flattening must not move a single byte, since the
    /// parser reads this format from runners of every version.
    const WALLTIME_JSON: &str = r#"{"version":7,"integration":["codspeed-rust","4.2.0"],"ignored_modules_by_pid":{"42":[["/lib/libpython.so",4096,8192]]},"debug_info":{"0__libc.so.6":{"object_path":"/lib/libc.so.6","addr_bounds":[4096,36864],"load_bias":4096,"debug_infos":[{"addr":4352,"size":32,"name":"malloc","file":"malloc.c","line":11}]}},"mapped_process_debug_info_by_pid":{"42":[{"debug_info_key":"0__libc.so.6","load_bias":4096}]},"mapped_process_unwind_data_by_pid":{"42":[{"unwind_data_key":"0__libc.so.6","timestamp":1234,"avma_range":{"start":4096,"end":36864},"base_avma":4096}]},"mapped_process_module_symbols":{"42":[{"perf_map_key":"0__libc.so.6","load_bias":4096}]},"path_key_to_path":{"0__libc.so.6":"/lib/libc.so.6"},"uri_by_ts":[[1,"bench::a"]],"ignored_modules":[],"markers":[]}"#;

    fn populated_artifacts() -> ModuleArtifacts {
        ModuleArtifacts {
            debug_info: HashMap::from([(
                "0__libc.so.6".to_string(),
                ModuleDebugInfo {
                    object_path: "/lib/libc.so.6".to_string(),
                    addr_bounds: (0x1000, 0x9000),
                    load_bias: 0x1000,
                    debug_infos: vec![crate::debug_info::DebugInfo {
                        addr: 0x1100,
                        size: 0x20,
                        name: "malloc".to_string(),
                        file: "malloc.c".to_string(),
                        line: Some(11),
                    }],
                },
            )]),
            mapped_process_debug_info_by_pid: HashMap::from([(
                42,
                vec![MappedProcessDebugInfo {
                    debug_info_key: "0__libc.so.6".to_string(),
                    load_bias: 0x1000,
                }],
            )]),
            mapped_process_unwind_data_by_pid: HashMap::from([(
                42,
                vec![MappedProcessUnwindData {
                    unwind_data_key: "0__libc.so.6".to_string(),
                    inner: crate::unwind_data::ProcessUnwindData {
                        timestamp: Some(1234),
                        avma_range: 0x1000..0x9000,
                        base_avma: 0x1000,
                    },
                }],
            )]),
            mapped_process_module_symbols: HashMap::from([(
                42,
                vec![crate::module_symbols::MappedProcessModuleSymbols {
                    perf_map_key: "0__libc.so.6".to_string(),
                    load_bias: 0x1000,
                }],
            )]),
            path_key_to_path: HashMap::from([(
                "0__libc.so.6".to_string(),
                PathBuf::from("/lib/libc.so.6"),
            )]),
        }
    }

    #[test]
    fn walltime_metadata_serialization_is_unchanged_by_flattening() {
        #[allow(deprecated)]
        let metadata = WalltimeMetadata {
            version: 7,
            integration: ("codspeed-rust".to_string(), "4.2.0".to_string()),
            ignored_modules_by_pid: HashMap::from([(
                42,
                vec![("/lib/libpython.so".to_string(), 0x1000, 0x2000)],
            )]),
            artifacts: populated_artifacts(),
            uri_by_ts: vec![(1, "bench::a".to_string())],
            ignored_modules: vec![],
            markers: vec![],
            debug_info_by_pid: HashMap::new(),
        };

        assert_eq!(serde_json::to_string(&metadata).unwrap(), WALLTIME_JSON);
    }

    #[test]
    fn walltime_metadata_round_trips_through_the_flattened_fields() {
        let parsed = WalltimeMetadata::from_reader(WALLTIME_JSON.as_bytes()).unwrap();

        assert_eq!(parsed.artifacts.path_key_to_path.len(), 1);
        assert_eq!(
            parsed.artifacts.mapped_process_unwind_data_by_pid[&42].len(),
            1
        );
        assert_eq!(serde_json::to_string(&parsed).unwrap(), WALLTIME_JSON);
    }

    #[test]
    fn memtrack_metadata_round_trips() {
        let metadata = MemtrackMetadata {
            version: MemtrackMetadata::CURRENT_VERSION,
            integration: ("codspeed-rust".to_string(), "4.2.0".to_string()),
            artifacts: populated_artifacts(),
        };

        let json = serde_json::to_string(&metadata).unwrap();
        let parsed = MemtrackMetadata::from_reader(json.as_bytes()).unwrap();

        assert_eq!(serde_json::to_string(&parsed).unwrap(), json);
        assert_eq!(
            parsed.artifacts.mapped_process_module_symbols[&42][0].perf_map_key,
            "0__libc.so.6"
        );
    }
}
