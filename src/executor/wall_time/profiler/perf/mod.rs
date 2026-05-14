#![cfg_attr(not(unix), allow(dead_code, unused_mut))]

use crate::cli::UnwindingMode;
use crate::executor::ExecutorConfig;
use crate::executor::ToolStatus;
use crate::executor::helpers::command::CommandBuilder;
use crate::executor::helpers::detect_executable::command_has_executable;
use crate::executor::helpers::env::is_codspeed_debug_enabled;
use crate::executor::helpers::env::suppress_go_perf_unwinding_warning;
use crate::executor::helpers::harvest_perf_maps_for_pids::harvest_perf_maps_for_pids;
use crate::executor::helpers::run_with_sudo::wrap_with_sudo;
use crate::executor::shared::fifo::FifoBenchmarkData;
use crate::executor::wall_time::profiler::NO_BENCHMARKS_DETECTED_WARNING;
use crate::executor::wall_time::profiler::Profiler;
use crate::executor::wall_time::profiler::SAMPLING_RATE_HZ;
use crate::executor::wall_time::profiler::WALLTIME_METADATA_CURRENT_VERSION;
use crate::executor::wall_time::profiler::linux_sysctl::ensure_linux_profiling_sysctls;
use crate::executor::wall_time::profiler::perf::perf_executable::get_working_perf_executable;
use crate::prelude::*;
use crate::system::SystemInfo;
use anyhow::Context;
use async_trait::async_trait;
use fifo::PerfFifo;
use parse_perf_file::MemmapRecordsOutput;
use perf_executable::get_compression_flags;
use perf_executable::get_event_flags;
use runner_shared::artifacts::ArtifactExt;
use runner_shared::artifacts::ExecutionTimestamps;
use runner_shared::metadata::WalltimeMetadata;
use std::path::Path;
use std::path::PathBuf;

mod debug_info;
mod elf_helper;
mod jit_dump;
mod loaded_module;
mod module_symbols;
mod naming;
mod parse_perf_file;
mod save_artifacts;
pub(crate) mod setup;
mod unwind_data;

pub mod fifo;
pub mod perf_executable;

const PERF_PIPEDATA_FILE_NAME: &str = "perf.pipedata";

pub struct PerfProfiler {
    /// Set by [`Profiler::wrap_command`]; used by the FIFO hooks to control event
    /// recording on the live `perf record` process.
    perf_fifo: Option<PerfFifo>,

    /// Path to the file that the wrapped command pipes `perf record`'s
    /// stdout into. Set by [`Profiler::wrap_command`]; consumed by [`Profiler::finalize`].
    perf_file_path: Option<PathBuf>,
}

impl PerfProfiler {
    pub fn new() -> Self {
        Self {
            perf_fifo: None,
            perf_file_path: None,
        }
    }

    fn perf_fifo_mut(&mut self) -> anyhow::Result<&mut PerfFifo> {
        self.perf_fifo
            .as_mut()
            .context("PerfProfiler::wrap_command must be called before FIFO hooks")
    }
}

#[async_trait(?Send)]
impl Profiler for PerfProfiler {
    fn tool_status(&self) -> Option<ToolStatus> {
        Some(setup::get_perf_status())
    }

    async fn setup(
        &self,
        system_info: &SystemInfo,
        setup_cache_dir: Option<&Path>,
    ) -> anyhow::Result<()> {
        setup::install_perf(system_info, setup_cache_dir).await?;
        ensure_linux_profiling_sysctls()
    }

    async fn wrap_command(
        &mut self,
        mut cmd_builder: CommandBuilder,
        config: &ExecutorConfig,
        profile_folder: &Path,
    ) -> anyhow::Result<CommandBuilder> {
        let perf_fifo = PerfFifo::new()?;
        let perf_file_path = profile_folder.join(PERF_PIPEDATA_FILE_NAME);

        // Infer the unwinding mode from the benchmark cmd
        let (cg_mode, stack_size) = if let Some(mode) = config.perf_unwinding_mode {
            (mode, None)
        } else if command_has_executable(
            &config.command,
            &["gradle", "gradlew", "java", "maven", "mvn", "mvnw"],
        ) {
            // In Java, we must use FP unwinding otherwise we'll have broken call stacks.
            (UnwindingMode::FramePointer, None)
        } else if command_has_executable(&config.command, &["cargo"]) {
            (UnwindingMode::Dwarf, None)
        } else if command_has_executable(&config.command, &["go"]) {
            (UnwindingMode::FramePointer, None)
        } else if command_has_executable(&config.command, &["pytest", "uv", "python", "python3"]) {
            // Note that the higher the stack size, the larger the file, although it is mitigated
            // by zstd compression
            (UnwindingMode::Dwarf, Some(32 * 1024))
        } else {
            // Default to dwarf unwinding since it works well with most binaries.
            debug!("No call graph mode detected, defaulting to dwarf");
            (UnwindingMode::Dwarf, None)
        };

        let cg_mode = match cg_mode {
            UnwindingMode::FramePointer => {
                suppress_go_perf_unwinding_warning();
                "fp"
            }
            UnwindingMode::Dwarf => &format!("dwarf,{}", stack_size.unwrap_or(8192)),
        };
        debug!("Using call graph mode: {cg_mode:?}");

        let working_perf_executable =
            get_working_perf_executable().context("Failed to find a working perf executable")?;
        let mut perf_wrapper_builder = CommandBuilder::new(&working_perf_executable);
        perf_wrapper_builder.arg("record");
        if !is_codspeed_debug_enabled() {
            perf_wrapper_builder.arg("--quiet");
        }
        // Add compression if available
        if let Some(compression_flags) = get_compression_flags(&working_perf_executable)? {
            perf_wrapper_builder.arg(compression_flags);
            // Add events flag if all required events are available
            if let Some(events_flag) = get_event_flags(&working_perf_executable)? {
                perf_wrapper_builder.arg(events_flag);
            }
        }

        perf_wrapper_builder.args([
            "--timestamp",
            // Required for matching the markers and URIs to the samples.
            "-k",
            "CLOCK_MONOTONIC",
            &format!("--freq={SAMPLING_RATE_HZ}"),
            "--delay=-1",
            "-g",
            "--user-callchains",
            &format!("--call-graph={cg_mode}"),
            &format!(
                "--control=fifo:{},{}",
                perf_fifo.ctl_path().to_string_lossy(),
                perf_fifo.ack_path().to_string_lossy()
            ),
            "-o",
            "-", // Output to stdout for piping
            "--",
        ]);

        cmd_builder.wrap_with(perf_wrapper_builder);

        let raw_command = format!(
            "set -o pipefail && {} | cat > {}",
            &cmd_builder.as_command_line(),
            perf_file_path.to_string_lossy()
        );

        let mut wrapped_builder = CommandBuilder::new("bash");
        wrapped_builder.args(["-c", &raw_command]);

        // IMPORTANT: Preserve the working directory from the original command
        if let Some(cwd) = cmd_builder.get_current_dir() {
            wrapped_builder.current_dir(cwd);
        }

        let wrapped_builder = wrap_with_sudo(wrapped_builder)?;

        self.perf_fifo = Some(perf_fifo);
        self.perf_file_path = Some(perf_file_path);

        Ok(wrapped_builder)
    }

    async fn on_start_profiler(&mut self) -> anyhow::Result<()> {
        self.perf_fifo_mut()?.start_events().await
    }

    async fn on_stop_profiler(&mut self) -> anyhow::Result<()> {
        self.perf_fifo_mut()?.stop_events().await
    }

    async fn on_ping(&mut self) -> anyhow::Result<bool> {
        Ok(self.perf_fifo_mut()?.ping().await.is_ok())
    }

    async fn finalize(
        &self,
        fifo_data: &FifoBenchmarkData,
        timestamps: &ExecutionTimestamps,
        profile_folder: &Path,
    ) -> anyhow::Result<()> {
        let start = std::time::Instant::now();

        let perf_file_path = self
            .perf_file_path
            .as_ref()
            .context("PerfProfiler::wrap_command must be called before finalize")?;

        let bench_data = BenchmarkData {
            fifo_data,
            marker_result: timestamps,
        };

        // Append perf maps, unwind info and other metadata
        if let Err(BenchmarkDataSaveError::MissingIntegration) =
            bench_data.save_to(profile_folder, perf_file_path).await
        {
            warn!("{NO_BENCHMARKS_DETECTED_WARNING}");
            return Ok(());
        }

        debug!("Perf teardown took: {:?}", start.elapsed());
        Ok(())
    }
}

struct BenchmarkData<'a> {
    fifo_data: &'a FifoBenchmarkData,
    marker_result: &'a ExecutionTimestamps,
}

#[derive(Debug)]
enum BenchmarkDataSaveError {
    MissingIntegration,
    FailedToParsePerfFile,
    FailedToHarvestPerfMaps,
    FailedToHarvestJitDumps,
}

impl BenchmarkData<'_> {
    async fn save_to(
        &self,
        path: &Path,
        perf_file_path: &Path,
    ) -> Result<(), BenchmarkDataSaveError> {
        self.marker_result.save_to(path).unwrap();

        let pid_filter = if self.fifo_data.is_exec_harness() {
            parse_perf_file::PidFilter::All
        } else {
            parse_perf_file::PidFilter::TrackedPids(self.fifo_data.bench_pids.clone())
        };

        debug!("Pid filter for perf file parsing: {pid_filter:?}");
        debug!("Reading perf data from file for mmap extraction");
        let MemmapRecordsOutput {
            loaded_modules_by_path,
            tracked_pids,
            parent_by_pid,
        } = parse_perf_file::parse_for_memmap2(perf_file_path, pid_filter).map_err(|e| {
            error!("Failed to parse perf file: {e}");
            BenchmarkDataSaveError::FailedToParsePerfFile
        })?;

        // Harvest the perf maps generated by python. This will copy the perf
        // maps from /tmp to the profile folder. We have to write our own perf
        // maps to these files AFTERWARDS, otherwise it'll be overwritten!
        debug!("Harvesting perf maps and jit dumps for pids: {tracked_pids:?}");
        harvest_perf_maps_for_pids(path, &tracked_pids)
            .await
            .map_err(|e| {
                error!("Failed to harvest perf maps: {e}");
                BenchmarkDataSaveError::FailedToHarvestPerfMaps
            })?;
        let jit_unwind_data_by_pid = jit_dump::save_symbols_and_harvest_unwind_data_for_pids(
            path,
            &tracked_pids,
            &parent_by_pid,
        )
        .await
        .map_err(|e| {
            error!("Failed to harvest jit dumps: {e}");
            BenchmarkDataSaveError::FailedToHarvestJitDumps
        })?;

        let artifacts =
            save_artifacts::save_artifacts(path, &loaded_modules_by_path, &jit_unwind_data_by_pid);

        debug!("Saving metadata");
        #[allow(deprecated)]
        let metadata = WalltimeMetadata {
            version: WALLTIME_METADATA_CURRENT_VERSION,
            integration: self
                .fifo_data
                .integration
                .clone()
                .ok_or(BenchmarkDataSaveError::MissingIntegration)?,
            uri_by_ts: self.marker_result.uri_by_ts.clone(),
            ignored_modules_by_pid: artifacts.ignored_modules_by_pid,
            markers: self.marker_result.markers.clone(),
            debug_info: artifacts.debug_info,
            mapped_process_debug_info_by_pid: artifacts.mapped_process_debug_info_by_pid,
            mapped_process_unwind_data_by_pid: artifacts.mapped_process_unwind_data_by_pid,
            mapped_process_module_symbols: artifacts.symbol_pid_mappings_by_pid,
            path_key_to_path: artifacts.key_to_path,
            // Deprecated fields below are no longer used
            debug_info_by_pid: Default::default(),
            ignored_modules: Default::default(),
        };
        metadata.save_to(path).unwrap();

        Ok(())
    }
}
