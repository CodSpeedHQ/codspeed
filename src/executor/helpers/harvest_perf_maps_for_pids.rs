use crate::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tokio::fs;

pub async fn harvest_perf_maps_for_pids(
    profile_folder: &Path,
    pids: &HashSet<libc::pid_t>,
) -> Result<()> {
    let perf_maps = pids
        .iter()
        .map(|pid| format!("perf-{pid}.map"))
        .map(|file_name| {
            (
                PathBuf::from("/tmp").join(&file_name),
                profile_folder.join(&file_name),
            )
        })
        .filter(|(src_path, _)| src_path.exists())
        .collect::<Vec<_>>();
    debug!("Found {} perf maps", perf_maps.len());

    for (src_path, dst_path) in perf_maps {
        fs::copy(&src_path, &dst_path).await.map_err(|e| {
            anyhow!(
                "Failed to copy perf map file: {:?} to {}: {}",
                src_path.file_name(),
                profile_folder.display(),
                e
            )
        })?;
    }

    Ok(())
}
