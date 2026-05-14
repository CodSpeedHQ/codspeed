use super::module_symbols::{ModuleSymbols, Symbol};
use crate::prelude::*;
use linux_perf_data::jitdump::{JitDumpReader, JitDumpRecord};
use runner_shared::unwind_data::{ProcessUnwindData, UnwindData};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

struct JitDump {
    path: PathBuf,
}

impl JitDump {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn into_perf_map(self) -> Result<ModuleSymbols> {
        let mut symbols = Vec::new();

        let file = std::fs::File::open(self.path)?;
        let mut reader = JitDumpReader::new(file)?;
        while let Some(raw_record) = reader.next_record()? {
            let JitDumpRecord::CodeLoad(record) = raw_record.parse()? else {
                continue;
            };

            let name = record.function_name.as_slice();
            let name = String::from_utf8_lossy(&name);

            symbols.push(Symbol {
                addr: record.vma,
                size: record.code_bytes.len() as u64,
                name: name.to_string(),
            });
        }
        debug!("Extracted {} JIT symbols", symbols.len());

        Ok(ModuleSymbols::new(symbols))
    }

    /// Parses the JIT dump file and converts it into deduplicated unwind data + pid mappings.
    ///
    /// The JIT dump file contains synthetic `eh_frame` data for jitted functions. This can be parsed and
    /// then converted to `UnwindData` + `UnwindDataPidMappingWithFullPath` which is used for stack unwinding.
    ///
    /// See: https://github.com/python/cpython/blob/main/Python/perf_jit_trampoline.c
    pub fn into_unwind_data(self) -> Result<Vec<(UnwindData, ProcessUnwindData)>> {
        let file = std::fs::File::open(self.path)?;

        let mut harvested_unwind_data = Vec::new();
        let mut current_unwind_info: Option<(Vec<u8>, Vec<u8>)> = None;

        let mut reader = JitDumpReader::new(file)?;
        while let Some(raw_record) = reader.next_record()? {
            // The first recording is always the unwind info, followed by the code load event
            // (see `perf_map_jit_write_entry` in https://github.com/python/cpython/blob/9743d069bd53e9d3a8f09df899ec1c906a79da24/Python/perf_jit_trampoline.c#L1163C13-L1163C37)
            match raw_record.parse()? {
                JitDumpRecord::CodeUnwindingInfo(record) => {
                    // Store unwind info for the next code loads
                    current_unwind_info = Some((
                        record.eh_frame.as_slice().to_vec(),
                        record.eh_frame_hdr.as_slice().to_vec(),
                    ));
                }
                JitDumpRecord::CodeLoad(record) => {
                    let name = record.function_name.as_slice();
                    let name = String::from_utf8_lossy(&name);

                    let avma_start = record.vma;
                    let code_size = record.code_bytes.len() as u64;
                    let avma_end = avma_start + code_size;

                    let Some((eh_frame, eh_frame_hdr)) = current_unwind_info.take() else {
                        warn!("No unwind info available for JIT code load: {name}");
                        continue;
                    };

                    let path = format!("jit_{name}");

                    let unwind_data = UnwindData {
                        path,
                        base_svma: 0,
                        eh_frame_hdr,
                        eh_frame_hdr_svma: 0..0,
                        eh_frame,
                        eh_frame_svma: 0..0,
                    };

                    let process_unwind_data = ProcessUnwindData {
                        timestamp: Some(raw_record.timestamp),
                        avma_range: avma_start..avma_end,
                        base_avma: 0,
                    };

                    harvested_unwind_data.push((unwind_data, process_unwind_data));
                }
                _ => {
                    warn!("Unhandled JIT dump record: {raw_record:?}");
                }
            }
        }

        Ok(harvested_unwind_data)
    }
}

/// Walk the fork chain rooted at `pid` and return ancestor pids from oldest
/// to youngest. Stops on self-loops or cycles.
fn ancestor_chain(
    pid: libc::pid_t,
    parent_by_pid: &HashMap<libc::pid_t, libc::pid_t>,
) -> Vec<libc::pid_t> {
    let mut ancestors = Vec::new();
    let mut cursor = pid;
    while let Some(&ppid) = parent_by_pid.get(&cursor) {
        if ppid == cursor || ppid == pid || ancestors.contains(&ppid) {
            break;
        }
        ancestors.push(ppid);
        cursor = ppid;
    }
    ancestors.reverse();
    ancestors
}

/// Converts all the `jit-<pid>.dump` into a perf-<pid>.map with symbols, and collects the unwind data.
///
/// # Symbols
/// Since a jit dump is by definition specific to a single pid, we append the harvested symbols
/// into a perf-<pid>.map instead of writing a specific jit.symbols.map.
///
/// # Unwind data
/// Unwind data is generated as a list.
///
/// # Fork inheritance
/// CPython's perf-trampoline writes a fresh jitdump per process. After
/// `fork()`, the child only registers code objects it newly enters, so
/// pre-fork trampolines — whose memory the child still holds (COW) and
/// returns through — are absent from the child's jitdump. To close that gap,
/// each child's perf-map and JIT unwind data are augmented with every
/// ancestor's jitdump entries (oldest first; child entries last so they
/// win on any address collision).
///
/// Known limitation: we currently inherit each ancestor's *entire final*
/// jitdump, not just records emitted before the descendant forked off.
/// Records the ancestor emitted after the fork describe trampolines in
/// pages that diverged (CoW-separated) from the descendant's address
/// space, so attributing the descendant's samples to those symbols can be
/// wrong. We have the timestamps in hand (jitdump CodeLoad records and the
/// FORK event both carry them) and could filter, but this hasn't been
/// observed to cause issues on real workloads yet. Revisit if we start
/// seeing implausible attributions in pool-worker callgraphs.
pub async fn save_symbols_and_harvest_unwind_data_for_pids(
    profile_folder: &Path,
    pids: &HashSet<libc::pid_t>,
    parent_by_pid: &HashMap<libc::pid_t, libc::pid_t>,
) -> Result<HashMap<i32, Vec<(UnwindData, ProcessUnwindData)>>> {
    // Convert every jitdump once and stash the results so children can pull
    // from their ancestors without re-parsing the dump files.
    let mut symbols_by_pid: HashMap<libc::pid_t, ModuleSymbols> = HashMap::new();
    let mut unwind_data_by_pid: HashMap<libc::pid_t, Vec<(UnwindData, ProcessUnwindData)>> =
        HashMap::new();

    for pid in pids {
        let path = PathBuf::from("/tmp").join(format!("jit-{pid}.dump"));
        if !path.exists() {
            continue;
        }
        debug!("Found JIT dump file: {path:?}");

        match JitDump::new(path.clone()).into_perf_map() {
            Ok(symbols) => {
                symbols_by_pid.insert(*pid, symbols);
            }
            Err(error) => {
                warn!("Failed to convert jit dump into perf map: {error:?}");
                continue;
            }
        }

        match JitDump::new(path).into_unwind_data() {
            Ok(data) => {
                unwind_data_by_pid.insert(*pid, data);
            }
            Err(error) => {
                warn!("Failed to convert jit dump into unwind data: {error:?}");
            }
        }
    }

    // Write perf-<pid>.map for each pid, prepending ancestor entries so the
    // child can resolve frames in pre-fork trampolines it still executes.
    let mut jit_unwind_data_by_path: HashMap<libc::pid_t, Vec<(UnwindData, ProcessUnwindData)>> =
        HashMap::new();
    for pid in pids {
        let Some(own_symbols) = symbols_by_pid.get(pid) else {
            continue;
        };

        let ancestors = ancestor_chain(*pid, parent_by_pid);
        let inheriting_ancestors: Vec<_> = ancestors
            .iter()
            .filter(|a| symbols_by_pid.contains_key(a))
            .collect();
        if !inheriting_ancestors.is_empty() {
            debug!(
                "perf-{pid}.map: inheriting JIT entries from {} ancestor pid(s): {:?}",
                inheriting_ancestors.len(),
                inheriting_ancestors,
            );
        }

        let map_path = profile_folder.join(format!("perf-{pid}.map"));
        for ancestor_pid in &ancestors {
            if let Some(ancestor_symbols) = symbols_by_pid.get(ancestor_pid) {
                ancestor_symbols.append_to_file(&map_path)?;
            }
        }
        own_symbols.append_to_file(&map_path)?;

        let mut merged_unwind = Vec::new();
        for ancestor_pid in &ancestors {
            if let Some(ancestor_unwind) = unwind_data_by_pid.get(ancestor_pid) {
                merged_unwind.extend(ancestor_unwind.iter().cloned());
            }
        }
        if let Some(own_unwind) = unwind_data_by_pid.get(pid) {
            merged_unwind.extend(own_unwind.iter().cloned());
        }
        if !merged_unwind.is_empty() {
            jit_unwind_data_by_path.insert(*pid, merged_unwind);
        }
    }

    Ok(jit_unwind_data_by_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancestor_chain_single_parent() {
        let parent_by_pid = HashMap::from([(200, 100)]);
        assert_eq!(ancestor_chain(200, &parent_by_pid), vec![100]);
    }

    #[test]
    fn ancestor_chain_multi_level_returns_oldest_first() {
        let parent_by_pid = HashMap::from([(300, 200), (200, 100)]);
        assert_eq!(ancestor_chain(300, &parent_by_pid), vec![100, 200]);
    }

    #[test]
    fn ancestor_chain_no_parent() {
        assert!(ancestor_chain(100, &HashMap::new()).is_empty());
    }

    #[test]
    fn ancestor_chain_breaks_self_referential_cycle() {
        let parent_by_pid = HashMap::from([(200, 100), (100, 200)]);
        let chain = ancestor_chain(200, &parent_by_pid);
        assert_eq!(chain, vec![100]);
    }
}
