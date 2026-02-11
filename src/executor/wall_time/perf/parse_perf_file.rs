use super::module_symbols::ModuleSymbols;
use super::unwind_data::unwind_data_from_elf;
use crate::prelude::*;
use libc::pid_t;
use linux_perf_data::PerfFileReader;
use linux_perf_data::PerfFileRecord;
use linux_perf_data::linux_perf_event_reader::EventRecord;
use linux_perf_data::linux_perf_event_reader::RecordType;
use runner_shared::unwind_data::ProcessUnwindData;
use runner_shared::unwind_data::UnwindData;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

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

pub struct MemmapRecordsOutput {
    /// Module symbols and the computed load bias for each pid that maps the ELF path.
    pub loaded_modules_by_path: HashMap<PathBuf, LoadedModule>,
    pub tracked_pids: HashSet<pid_t>,
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

    // 1MiB buffer
    let reader = std::io::BufReader::with_capacity(
        1024 * 1024,
        std::fs::File::open(perf_file_path.as_ref())?,
    );

    let PerfFileReader {
        mut perf_file,
        mut record_iter,
    } = PerfFileReader::parse_pipe(reader)?;

    while let Some(record) = record_iter.next_record(&mut perf_file).unwrap() {
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

                if pid_filter.add_child_if_parent_tracked(fork_record.ppid, fork_record.pid) {
                    trace!(
                        "Fork: Tracking child PID {} from parent PID {}",
                        fork_record.pid, fork_record.ppid
                    );
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
    })
}

/// PID filter for parsing perf records
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
