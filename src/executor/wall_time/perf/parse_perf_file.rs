use super::perf_map::ProcessSymbols;
use super::unwind_data::UnwindDataExt;
use crate::prelude::*;
use libc::pid_t;
use linux_perf_data::PerfFileReader;
use linux_perf_data::PerfFileRecord;
use linux_perf_data::linux_perf_event_reader::EventRecord;
use linux_perf_data::linux_perf_event_reader::RecordType;
use runner_shared::unwind_data::UnwindData;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

pub struct MemmapRecordsOutput {
    pub symbols_by_pid: HashMap<pid_t, ProcessSymbols>,
    pub unwind_data_by_pid: HashMap<pid_t, Vec<UnwindData>>,
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
    let mut symbols_by_pid = HashMap::<pid_t, ProcessSymbols>::new();
    let mut unwind_data_by_pid = HashMap::<pid_t, Vec<UnwindData>>::new();

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

                process_mmap2_record(mmap2_record, &mut symbols_by_pid, &mut unwind_data_by_pid);
            }
            _ => continue,
        }
    }

    // Retrieve the set of PIDs we ended up tracking after processing all records
    let tracked_pids: HashSet<pid_t> = match pid_filter {
        PidFilter::All => symbols_by_pid.keys().copied().collect(),
        PidFilter::TrackedPids(tracked) => tracked,
    };

    Ok(MemmapRecordsOutput {
        symbols_by_pid,
        unwind_data_by_pid,
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
    symbols_by_pid: &mut HashMap<pid_t, ProcessSymbols>,
    unwind_data_by_pid: &mut HashMap<pid_t, Vec<UnwindData>>,
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
    symbols_by_pid
        .entry(record.pid)
        .or_insert(ProcessSymbols::new(record.pid))
        .add_mapping(
            record.pid,
            &record_path_string,
            record.address,
            end_addr,
            record.page_offset,
        );

    match UnwindData::new(
        record_path_string.as_bytes(),
        record.page_offset,
        record.address,
        end_addr,
        None,
    ) {
        Ok(unwind_data) => {
            unwind_data_by_pid
                .entry(record.pid)
                .or_default()
                .push(unwind_data);
            trace!(
                "Added unwind data for {record_path_string} ({:x} - {:x})",
                record.address, end_addr
            );
        }
        Err(error) => {
            debug!("Failed to create unwind data for module {record_path_string}: {error}");
        }
    }
}
