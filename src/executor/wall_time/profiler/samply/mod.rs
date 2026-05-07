//! Samply profiler integration.

use crate::executor::ExecutorConfig;
use crate::executor::helpers::command::CommandBuilder;
use crate::executor::shared::fifo::FifoBenchmarkData;
use crate::executor::wall_time::profiler::Profiler;
use crate::executor::wall_time::profiler::linux_sysctl::ensure_linux_profiling_sysctls;
use crate::prelude::*;
use crate::system::SystemInfo;
use async_trait::async_trait;
use runner_shared::artifacts::ArtifactExt;
use runner_shared::artifacts::ExecutionTimestamps;
use runner_shared::metadata::WalltimeMetadata;
use std::path::Path;
use std::path::PathBuf;

use super::WALLTIME_METADATA_CURRENT_VERSION;

const SAMPLY_OUTPUT_FILE_NAME: &str = "samply-profile.json.gz";
const SAMPLY_RATE_HZ: &str = "997";

pub struct SamplyProfiler {
    /// Set by [`Profiler::wrap`]. Currently unused after `wrap` returns —
    /// samply writes the file itself — but we hold onto it so future
    /// `finalize` work (e.g. validation, conversion) has the path on hand.
    output_path: Option<PathBuf>,
}

impl SamplyProfiler {
    pub fn new() -> Self {
        Self { output_path: None }
    }
}

#[async_trait(?Send)]
impl Profiler for SamplyProfiler {
    async fn setup(
        &self,
        _system_info: &SystemInfo,
        _setup_cache_dir: Option<&Path>,
    ) -> anyhow::Result<()> {
        ensure_linux_profiling_sysctls()
    }

    async fn wrap(
        &mut self,
        mut cmd_builder: CommandBuilder,
        _config: &ExecutorConfig,
        profile_folder: &Path,
    ) -> anyhow::Result<CommandBuilder> {
        let output_path = profile_folder.join(SAMPLY_OUTPUT_FILE_NAME);

        let mut samply_builder = CommandBuilder::new("samply");
        samply_builder.args([
            "record",
            "--presymbolicate",
            "--no-open",
            "--save-only",
            "--rate",
            SAMPLY_RATE_HZ,
        ]);
        samply_builder.arg("-o");
        samply_builder.arg(&output_path);
        samply_builder.arg("--");

        cmd_builder.wrap_with(samply_builder);
        self.output_path = Some(output_path);
        Ok(cmd_builder)
    }

    async fn finalize(
        &self,
        fifo_data: &FifoBenchmarkData,
        timestamps: &ExecutionTimestamps,
        profile_folder: &Path,
    ) -> anyhow::Result<()> {
        let Some(integration) = fifo_data.integration.clone() else {
            warn!(
                "Walltime profiling is enabled, but failed to detect benchmarks. If you wish to disable this warning, set CODSPEED_PROFILER_ENABLED=false"
            );
            return Ok(());
        };

        #[allow(deprecated)]
        let metadata = WalltimeMetadata {
            version: WALLTIME_METADATA_CURRENT_VERSION,
            integration,
            uri_by_ts: timestamps.uri_by_ts.clone(),
            markers: timestamps.markers.clone(),

            // These fields aren't required in samply, since we symbolicate client-side.
            ignored_modules_by_pid: Default::default(),
            debug_info: Default::default(),
            mapped_process_debug_info_by_pid: Default::default(),
            mapped_process_unwind_data_by_pid: Default::default(),
            mapped_process_module_symbols: Default::default(),
            path_key_to_path: Default::default(),

            // Deprecated fields below are no longer used
            debug_info_by_pid: Default::default(),
            ignored_modules: Default::default(),
        };
        metadata.save_to(profile_folder).unwrap();

        if let Err(e) = timestamps.save_to(profile_folder) {
            warn!("Failed to save execution timestamps: {e:?}");
        }

        Ok(())
    }
}
