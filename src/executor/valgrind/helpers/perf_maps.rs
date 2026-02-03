use crate::executor::helpers::harvest_perf_maps_for_pids::harvest_perf_maps_for_pids;
use crate::prelude::*;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

pub async fn harvest_perf_maps(profile_folder: &Path) -> Result<()> {
    // Get profile files (files with .out extension)
    let profile_files = fs::read_dir(profile_folder)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().unwrap_or_default() == "out")
        .collect_vec();

    let pids = profile_files
        .iter()
        .filter_map(|path| path.file_stem())
        .map(|pid| pid.to_str().unwrap())
        .filter_map(|pid| pid.parse().ok())
        .collect::<HashSet<_>>();

    harvest_perf_maps_for_pids(profile_folder, &pids).await
}
