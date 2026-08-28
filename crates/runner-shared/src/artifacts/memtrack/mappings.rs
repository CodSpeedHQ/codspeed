use libc::pid_t;
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// The file-backed mappings the tracked process tree loaded, recorded as they
/// happened. Companion to the event stream: allocation stacks are raw
/// addresses, and these are what turns them back into modules.
///
/// Kept out of the event stream so a consumer that only needs the module set
/// does not have to decode millions of allocation events.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemtrackMappings {
    pub mappings: Vec<ProcessMapping>,
}

impl super::super::ArtifactExt for MemtrackMappings {}

/// One executable mapping of one file into one process, as `PERF_RECORD_MMAP2`
/// would describe it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessMapping {
    pub pid: pid_t,
    /// Resolved in-kernel at mmap time, so it is correct for the mapping
    /// process's mount namespace even if the process is already gone.
    pub path: String,
    /// Kernel `s_dev` encoding: `(major << 20) | minor`. With `ino`, proves at
    /// analysis time that the path still names the file that was mapped.
    pub dev: u64,
    pub ino: u64,
    /// Offset of the mapping's first byte in the file. In bytes, matching
    /// `PERF_RECORD_MMAP2`'s `pgoff` and the load-bias computation.
    pub file_offset: u64,
    pub avma_range: Range<u64>,
    /// CLOCK_MONOTONIC nanoseconds, the same clock the events carry. The
    /// mapping is valid from here until a later mapping covers the range.
    pub timestamp: u64,
}
