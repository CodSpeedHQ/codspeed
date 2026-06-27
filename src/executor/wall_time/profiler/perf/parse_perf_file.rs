use super::loaded_module::{LoadedModule, ProcessLoadedModule};
use super::module_symbols::ModuleSymbols;
use super::unwind_data::unwind_data_from_elf;
use crate::prelude::*;
use libc::pid_t;
use linux_perf_data::PerfFileReader;
use linux_perf_data::PerfFileRecord;
use linux_perf_data::linux_perf_event_reader::EventRecord;
use linux_perf_data::linux_perf_event_reader::RecordType;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

pub struct MemmapRecordsOutput {
    /// Module symbols and the computed load bias for each pid that maps the ELF path.
    pub loaded_modules_by_path: HashMap<PathBuf, LoadedModule>,
    pub tracked_pids: HashSet<pid_t>,
    /// Jitdump file paths discovered from MMAP2 records, keyed by PID.
    pub jit_dump_paths_by_pid: HashMap<pid_t, Vec<PathBuf>>,
}

/// Parse the perf file at `perf_file_path` and look for MMAP2 records for the given `pids`.
/// If the pids filter is empty, all MMAP2 records will be parsed.
///
/// Returns process symbols and unwind data for the executable mappings found in the perf file.
pub fn parse_for_memmap2<P: AsRef<Path>>(
    perf_file_path: P,
    mut pid_filter: PidFilter,
) -> Result<MemmapRecordsOutput> {
    let mut loaded_modules_by_path = HashMap::<PathBuf, LoadedModule>::new();
    let mut jit_dump_paths_by_pid = HashMap::<pid_t, Vec<PathBuf>>::new();

    // 1MiB buffer
    let reader = std::io::BufReader::with_capacity(
        1024 * 1024,
        std::fs::File::open(perf_file_path.as_ref())?,
    );

    let PerfFileReader {
        mut perf_file,
        mut record_iter,
    } = PerfFileReader::parse_pipe(reader)?;

    // This loop relies on the events being in chronological order, which is guaranteed by the perf file format.
    while let Some(record) = record_iter.next_record(&mut perf_file)? {
        let PerfFileRecord::EventRecord { record, .. } = record else {
            continue;
        };

        // Check the type from the raw record to avoid parsing overhead since we do not care about
        // most records.
        match record.record_type {
            RecordType::FORK => {
                // Process fork events to track children (and children of children) of filtered PIDs
                let Ok(parsed_record) = record.parse() else {
                    continue;
                };

                let EventRecord::Fork(fork_record) = parsed_record else {
                    continue;
                };

                // Thread creation also emits FORK, which share parent's address space, so nothing to do
                if fork_record.ppid == fork_record.pid {
                    continue;
                }

                if pid_filter.add_child_if_parent_tracked(fork_record.ppid, fork_record.pid) {
                    trace!(
                        "Fork: Tracking child PID {} from parent PID {}",
                        fork_record.pid, fork_record.ppid
                    );
                }

                inherit_parent_mappings(
                    &mut loaded_modules_by_path,
                    fork_record.ppid,
                    fork_record.pid,
                );
            }
            RecordType::COMM => {
                // An execve() replaces the entire address space: the mappings inherited from the
                // parent on fork (and any pre-exec mappings of this pid) are no longer valid.
                // TODO(COD-1377):  Manage timing in module mapping to improve symbolication.
                let Ok(parsed_record) = record.parse() else {
                    continue;
                };

                let EventRecord::Comm(comm_record) = parsed_record else {
                    continue;
                };

                if !comm_record.is_execve {
                    continue;
                }

                if pid_filter.should_include(comm_record.pid) {
                    trace!(
                        "Exec: Purging inherited mappings for PID {}",
                        comm_record.pid
                    );
                    purge_process_mappings(&mut loaded_modules_by_path, comm_record.pid);
                }
            }
            RecordType::MMAP2 => {
                let Ok(parsed_record) = record.parse() else {
                    continue;
                };

                // Should never fail since we already checked the type in the raw record
                let EventRecord::Mmap2(mmap2_record) = parsed_record else {
                    continue;
                };

                // Filter on pid early to avoid string allocation for unwanted records
                if !pid_filter.should_include(mmap2_record.pid) {
                    continue;
                }

                // Collect jitdump file paths before the PROT_EXEC filter in process_mmap2_record
                // skips them. JIT runtimes mmap the jitdump file so perf records it.
                // Match perf's jit_detect(): basename must be `jit-<pid>.dump`.
                if is_jit_dump_path(&mmap2_record.path.as_slice(), mmap2_record.pid) {
                    let path = PathBuf::from(
                        String::from_utf8_lossy(&mmap2_record.path.as_slice()).into_owned(),
                    );
                    if path.exists() {
                        debug!("Found jitdump path from MMAP2 record: {path:?}");
                        jit_dump_paths_by_pid
                            .entry(mmap2_record.pid)
                            .or_default()
                            .push(path);
                    }
                }

                process_mmap2_record(mmap2_record, &mut loaded_modules_by_path);
            }
            _ => continue,
        }
    }

    // Retrieve the set of PIDs we ended up tracking after processing all records
    let tracked_pids: HashSet<pid_t> = match pid_filter {
        PidFilter::All => loaded_modules_by_path
            .iter()
            .flat_map(|(_, loaded)| loaded.pids())
            .collect(),
        PidFilter::TrackedPids(tracked) => tracked,
    };

    Ok(MemmapRecordsOutput {
        loaded_modules_by_path,
        tracked_pids,
        jit_dump_paths_by_pid,
    })
}

/// PID filter for parsing perf records
#[derive(Debug)]
pub enum PidFilter {
    /// Parse records for all PIDs
    All,
    /// Parse records only for specific PIDs and their children
    TrackedPids(HashSet<pid_t>),
}

impl PidFilter {
    /// Check if a PID should be included in parsing
    fn should_include(&self, pid: pid_t) -> bool {
        match self {
            PidFilter::All => true,
            PidFilter::TrackedPids(tracked_pids) => tracked_pids.contains(&pid),
        }
    }

    /// Add a child PID to the filter if we're tracking its parent
    /// Returns true if the child was added
    fn add_child_if_parent_tracked(&mut self, parent_pid: pid_t, child_pid: pid_t) -> bool {
        match self {
            PidFilter::All => false, // Already tracking all PIDs
            PidFilter::TrackedPids(tracked_pids) => {
                if tracked_pids.contains(&parent_pid) {
                    tracked_pids.insert(child_pid)
                } else {
                    false
                }
            }
        }
    }
}

/// Copy every module the parent pid has mounted onto the child pid.
///
/// Forked processes inherit their parent's memory mappings, but there will not be any MMAP2 record
/// in the perf data since the mapping has already happened.
fn inherit_parent_mappings(
    loaded_modules_by_path: &mut HashMap<PathBuf, LoadedModule>,
    ppid: pid_t,
    pid: pid_t,
) {
    use std::collections::hash_map::Entry;

    for loaded_module in loaded_modules_by_path.values_mut() {
        let inherited =
            loaded_module
                .process_loaded_modules
                .get(&ppid)
                .map(|p| ProcessLoadedModule {
                    symbols_load_bias: p.symbols_load_bias,
                    process_unwind_data: p.process_unwind_data.clone(),
                });
        let Some(inherited) = inherited else {
            continue;
        };
        // Only insert if the child has no entry yet; an existing entry came from
        // the child's own MMAP2 and is authoritative.
        if let Entry::Vacant(slot) = loaded_module.process_loaded_modules.entry(pid) {
            slot.insert(inherited);
        }
    }
}

/// Drop every mapping recorded for `pid` across all modules.
fn purge_process_mappings(loaded_modules_by_path: &mut HashMap<PathBuf, LoadedModule>, pid: pid_t) {
    for loaded_module in loaded_modules_by_path.values_mut() {
        loaded_module.process_loaded_modules.remove(&pid);
    }
}

/// Returns true if the path basename matches perf's `jit_detect()` pattern: `jit-<pid>.dump`,
/// where `<pid>` must match the MMAP2 record's PID.
fn is_jit_dump_path(path: &[u8], pid: pid_t) -> bool {
    let Some(pos) = path.iter().rposition(|&b| b == b'/') else {
        return false;
    };
    let basename = &path[pos + 1..];
    let expected = format!("jit-{pid}.dump");
    basename == expected.as_bytes()
}

/// Process a single MMAP2 record and add it to the symbols and unwind data maps
fn process_mmap2_record(
    record: linux_perf_data::linux_perf_event_reader::Mmap2Record,
    loaded_modules_by_path: &mut HashMap<PathBuf, LoadedModule>,
) {
    // Check PROT_EXEC early to avoid string allocation for non-executable mappings
    if record.protection as i32 & libc::PROT_EXEC == 0 {
        return;
    }

    // Filter on raw bytes before allocating a String
    let path_slice: &[u8] = &record.path.as_slice();

    // Skip anonymous mappings
    if path_slice == b"//anon" {
        return;
    }

    // Skip special mappings like [vdso], [heap], etc.
    if path_slice.first() == Some(&b'[') && path_slice.last() == Some(&b']') {
        return;
    }

    let record_path_string = String::from_utf8_lossy(path_slice).into_owned();
    let record_path = PathBuf::from(&record_path_string);
    let end_addr = record.address + record.length;

    trace!(
        "Mapping: Pid {}: {:016x}-{:016x} {:08x} {:?} (Prot {:?})",
        record.pid,
        record.address,
        end_addr,
        record.page_offset,
        record_path_string,
        record.protection,
    );

    let load_bias = match ModuleSymbols::compute_load_bias(
        &record_path,
        record.address,
        end_addr,
        record.page_offset,
    ) {
        Ok(load_bias) => load_bias,
        Err(e) => {
            debug!("Failed to compute load bias for {record_path_string}: {e}");
            return;
        }
    };

    let loaded_module = loaded_modules_by_path
        .entry(record_path.clone())
        .or_default();

    let process_loaded_module = loaded_module
        .process_loaded_modules
        .entry(record.pid)
        .or_default();

    // Extract module symbols if it's no module symbol from path
    if loaded_module.module_symbols.is_none() {
        match ModuleSymbols::from_elf(&record_path) {
            Ok(symbols) => loaded_module.module_symbols = Some(symbols),
            Err(error) => {
                debug!("Failed to load symbols for module {record_path_string}: {error}");
            }
        }
    }

    // Store load bias for this process mounting
    process_loaded_module.symbols_load_bias = Some(load_bias);

    // Extract unwind_data
    match unwind_data_from_elf(
        record_path_string.as_bytes(),
        record.address,
        end_addr,
        None,
        load_bias,
    ) {
        Ok((unwind_data, process_unwind_data)) => {
            loaded_module.unwind_data = Some(unwind_data);
            process_loaded_module.process_unwind_data = Some(process_unwind_data);
        }
        Err(error) => {
            debug!("Failed to load unwind data for module {record_path_string}: {error}");
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_module_with_parent(ppid: pid_t, load_bias: u64) -> LoadedModule {
        let mut m = LoadedModule::default();
        m.process_loaded_modules.insert(
            ppid,
            ProcessLoadedModule {
                symbols_load_bias: Some(load_bias),
                process_unwind_data: None,
            },
        );
        m
    }

    #[test]
    fn inherit_parent_mappings_copies_parent_entry_to_child() {
        let mut modules: HashMap<PathBuf, LoadedModule> = HashMap::new();
        modules.insert(
            PathBuf::from("/lib/libpython.so"),
            make_module_with_parent(100, 0xdead),
        );

        inherit_parent_mappings(&mut modules, 100, 200);

        let m = &modules[&PathBuf::from("/lib/libpython.so")];
        let child = m.process_loaded_modules.get(&200).unwrap();
        assert_eq!(child.symbols_load_bias, Some(0xdead));
    }

    #[test]
    fn inherit_parent_mappings_does_not_overwrite_existing_child_entry() {
        let mut modules: HashMap<PathBuf, LoadedModule> = HashMap::new();
        let mut m = make_module_with_parent(100, 0xdead);
        // Child already has its own (post-exec) mapping at a different bias.
        m.process_loaded_modules.insert(
            200,
            ProcessLoadedModule {
                symbols_load_bias: Some(0xcafe),
                process_unwind_data: None,
            },
        );
        modules.insert(PathBuf::from("/lib/libpython.so"), m);

        inherit_parent_mappings(&mut modules, 100, 200);

        let child = modules[&PathBuf::from("/lib/libpython.so")]
            .process_loaded_modules
            .get(&200)
            .unwrap();
        assert_eq!(child.symbols_load_bias, Some(0xcafe));
    }

    #[test]
    fn purge_process_mappings_removes_only_the_target_pid() {
        // Reproduces the fork+exec collision: child 200 forked from bash (100) and inherited
        // bash's PIE mapping at 0xaaaaaaaa0000, then exec'd its own binary at the same base.
        let mut modules: HashMap<PathBuf, LoadedModule> = HashMap::new();

        // Inherited-from-bash module that must be dropped on exec.
        let mut bash = make_module_with_parent(100, 0xaaaaaaaa0000);
        bash.process_loaded_modules.insert(
            200,
            ProcessLoadedModule {
                symbols_load_bias: Some(0xaaaaaaaa0000),
                process_unwind_data: None,
            },
        );
        modules.insert(PathBuf::from("/usr/bin/bash"), bash);

        // The real post-exec binary, also based at 0xaaaaaaaa0000 for pid 200.
        modules.insert(
            PathBuf::from("/cpp/build/fractal_main"),
            make_module_with_parent(200, 0xaaaaaaaa0000),
        );

        purge_process_mappings(&mut modules, 200);

        // Pid 200 is gone from every module...
        assert!(
            !modules[&PathBuf::from("/usr/bin/bash")]
                .process_loaded_modules
                .contains_key(&200)
        );
        assert!(
            !modules[&PathBuf::from("/cpp/build/fractal_main")]
                .process_loaded_modules
                .contains_key(&200)
        );
        // ...but bash's own pid 100 mapping is untouched.
        assert_eq!(
            modules[&PathBuf::from("/usr/bin/bash")]
                .process_loaded_modules
                .get(&100)
                .unwrap()
                .symbols_load_bias,
            Some(0xaaaaaaaa0000)
        );
    }
}
