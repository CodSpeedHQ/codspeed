use crate::executor::helpers::harvest_perf_maps_for_pids::harvest_perf_maps_for_pids;
use crate::prelude::*;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Extracts a PID from a profile output file path.
///
/// Supports both Callgrind (`<pid>.out`) and Tracegrind (`<pid>.tgtrace`) file formats.
fn extract_pid_from_profile_file(path: &Path) -> Option<libc::pid_t> {
    let ext = path.extension()?.to_str()?;
    match ext {
        "out" | "tgtrace" => path.file_stem()?.to_str()?.parse().ok(),
        _ => None,
    }
}

pub async fn harvest_perf_maps(profile_folder: &Path) -> Result<()> {
    let pids = fs::read_dir(profile_folder)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter_map(|path| extract_pid_from_profile_file(&path))
        .collect::<HashSet<_>>();

    harvest_perf_maps_for_pids(profile_folder, &pids).await
}
