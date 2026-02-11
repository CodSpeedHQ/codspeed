#![cfg_attr(not(unix), allow(dead_code, unused_mut))]

use crate::cli::UnwindingMode;
use crate::executor::Config;
use crate::executor::helpers::command::CommandBuilder;
use crate::executor::helpers::env::is_codspeed_debug_enabled;
use crate::executor::helpers::run_command_with_log_pipe::run_command_with_log_pipe_and_callback;
use crate::executor::helpers::run_with_sudo::run_with_sudo;
use crate::executor::helpers::run_with_sudo::wrap_with_sudo;
use crate::executor::shared::fifo::FifoBenchmarkData;
use crate::executor::shared::fifo::RunnerFifo;
use crate::executor::wall_time::perf::perf_executable::get_working_perf_executable;
use crate::prelude::*;
use anyhow::Context;
use fifo::PerfFifo;
use parse_perf_file::MemmapRecordsOutput;
use perf_executable::get_compression_flags;
use perf_executable::get_event_flags;
use runner_shared::artifacts::ArtifactExt;
use runner_shared::artifacts::ExecutionTimestamps;
use runner_shared::fifo::Command as FifoCommand;
use runner_shared::fifo::IntegrationMode;
use runner_shared::metadata::PerfMetadata;
use std::path::Path;
use std::path::PathBuf;
use std::{cell::OnceCell, process::ExitStatus};

mod jit_dump;
mod naming;
mod parse_perf_file;
mod save_artifacts;
mod setup;

pub mod debug_info;
pub mod elf_helper;
pub mod fifo;
pub mod module_symbols;
pub mod perf_executable;
pub mod unwind_data;

const PERF_METADATA_CURRENT_VERSION: u64 = 1;
const PERF_PIPEDATA_FILE_NAME: &str = "perf.pipedata";

pub struct PerfRunner {
    benchmark_data: OnceCell<BenchmarkData>,
}

impl PerfRunner {
    pub async fn setup_environment(
        system_info: &crate::system::SystemInfo,
        setup_cache_dir: Option<&Path>,
    ) -> anyhow::Result<()> {
        setup::install_perf(system_info, setup_cache_dir).await?;

        let sysctl_read = |name: &str| -> anyhow::Result<i64> {
            let output = std::process::Command::new("sysctl").arg(name).output()?;
            let output = String::from_utf8(output.stdout)?;

            Ok(output
                .split(" = ")
                .last()
                .context("Couldn't find the value in sysctl output")?
                .trim()
                .parse::<i64>()?)
        };

        // Allow access to kernel symbols
        if sysctl_read("kernel.kptr_restrict")? != 0 {
            run_with_sudo("sysctl", ["-w", "kernel.kptr_restrict=0"])?;
        }

        // Allow non-root profiling
        if sysctl_read("kernel.perf_event_paranoid")? != -1 {
            run_with_sudo("sysctl", ["-w", "kernel.perf_event_paranoid=-1"])?;
        }

        Ok(())
    }

    pub fn new() -> Self {
        Self {
            benchmark_data: OnceCell::new(),
        }
    }

    pub async fn run(
        &self,
        mut cmd_builder: CommandBuilder,
        config: &Config,
        profile_folder: &Path,
    ) -> anyhow::Result<ExitStatus> {
        let perf_fifo = PerfFifo::new()?;
        let runner_fifo = RunnerFifo::new()?;

        // Infer the unwinding mode from the benchmark cmd
        let (cg_mode, stack_size) = if let Some(mode) = config.perf_unwinding_mode {
            (mode, None)
        } else if config.command.contains("cargo") {
            (UnwindingMode::Dwarf, None)
        } else if config.command.contains("pytest")
            || config.command.contains("uv")
            || config.command.contains("python")
        {
            // Note that the higher the stack size, the larger the file, although it is mitigated
            // by zstd compression
            (UnwindingMode::Dwarf, Some(16 * 1024))
        } else {
            // Default to dwarf unwinding since it works well with most binaries.
            debug!("No call graph mode detected, defaulting to dwarf");
            (UnwindingMode::Dwarf, None)
        };

        let cg_mode = match cg_mode {
            UnwindingMode::FramePointer => "fp",
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
            "--freq=997", // Use a prime number to avoid synchronization with periodic tasks
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
            self.get_perf_file_path(profile_folder).to_string_lossy()
        );

        let mut wrapped_builder = CommandBuilder::new("bash");
        wrapped_builder.args(["-c", &raw_command]);

        // IMPORTANT: Preserve the working directory from the original command
        if let Some(cwd) = cmd_builder.get_current_dir() {
            wrapped_builder.current_dir(cwd);
        }

        let cmd = wrap_with_sudo(wrapped_builder)?.build();
        debug!("cmd: {cmd:?}");

        let on_process_started = |mut child: std::process::Child| async move {
            // If we output pipedata, we do not parse the perf map during teardown yet, so we need to parse memory
            // maps as we receive the `CurrentBenchmark` fifo commands.
            let (data, exit_status) = Self::handle_fifo(runner_fifo, perf_fifo, &mut child).await?;
            self.benchmark_data.set(data).unwrap_or_else(|_| {
                error!("Failed to set benchmark data in PerfRunner");
            });
            Ok(exit_status)
        };
        run_command_with_log_pipe_and_callback(cmd, on_process_started).await
    }

    pub async fn save_files_to(&self, profile_folder: &Path) -> anyhow::Result<()> {
        let start = std::time::Instant::now();

        let bench_data = self
            .benchmark_data
            .get()
            .expect("Benchmark order is not available");

        // Append perf maps, unwind info and other metadata
        if let Err(BenchmarkDataSaveError::MissingIntegration) = bench_data
            .save_to(profile_folder, &self.get_perf_file_path(profile_folder))
            .await
        {
            warn!(
                "Perf is enabled, but failed to detect benchmarks. If you wish to disable this warning, set CODSPEED_PERF_ENABLED=false"
            );
            return Ok(());
        }

        let elapsed = start.elapsed();
        debug!("Perf teardown took: {elapsed:?}");
        Ok(())
    }

    async fn handle_fifo(
        mut runner_fifo: RunnerFifo,
        mut perf_fifo: PerfFifo,
        child: &mut std::process::Child,
    ) -> anyhow::Result<(BenchmarkData, std::process::ExitStatus)> {
        let on_cmd = async |cmd: &FifoCommand| {
            #[allow(deprecated)]
            match cmd {
                FifoCommand::StartBenchmark => {
                    perf_fifo.start_events().await?;
                }
                FifoCommand::StopBenchmark => {
                    perf_fifo.stop_events().await?;
                }
                FifoCommand::PingPerf => {
                    if perf_fifo.ping().await.is_err() {
                        return Ok(Some(FifoCommand::Err));
                    }
                    return Ok(Some(FifoCommand::Ack));
                }
                FifoCommand::GetIntegrationMode => {
                    return Ok(Some(FifoCommand::IntegrationModeResponse(
                        IntegrationMode::Perf,
                    )));
                }
                _ => {}
            }

            Ok(None)
        };

        let (marker_result, fifo_data, exit_status) =
            runner_fifo.handle_fifo_messages(child, on_cmd).await?;

        Ok((
            BenchmarkData {
                fifo_data,
                marker_result,
            },
            exit_status,
        ))
    }

    fn get_perf_file_path<P: AsRef<Path>>(&self, profile_folder: P) -> PathBuf {
        profile_folder.as_ref().join(PERF_PIPEDATA_FILE_NAME)
    }
}

pub struct BenchmarkData {
    fifo_data: FifoBenchmarkData,
    marker_result: ExecutionTimestamps,
}

#[derive(Debug)]
pub enum BenchmarkDataSaveError {
    MissingIntegration,
    FailedToParsePerfFile,
    FailedToHarvestJitDumps,
}

impl BenchmarkData {
    pub async fn save_to<P: AsRef<std::path::Path>>(
        &self,
        path: P,
        perf_file_path: P,
    ) -> Result<(), BenchmarkDataSaveError> {
        self.marker_result.save_to(&path).unwrap();

        let path_ref = path.as_ref();

        debug!("Reading perf data from file for mmap extraction");
        let pid_filter = if self.fifo_data.is_exec_harness() {
            parse_perf_file::PidFilter::All
        } else {
            parse_perf_file::PidFilter::TrackedPids(self.fifo_data.bench_pids.clone())
        };
        let MemmapRecordsOutput {
            loaded_modules_by_path,
            tracked_pids,
        } = {
            parse_perf_file::parse_for_memmap2(perf_file_path, pid_filter).map_err(|e| {
                error!("Failed to parse perf file: {e}");
                BenchmarkDataSaveError::FailedToParsePerfFile
            })?
        };

        // Harvest the perf maps generated by python. This will copy the perf
        // maps from /tmp to the profile folder. We have to write our own perf
        // maps to these files AFTERWARDS, otherwise it'll be overwritten!
        debug!("Harvesting perf maps and jit dumps for pids: {tracked_pids:?}");
        let jit_unwind_data_by_pid =
            jit_dump::save_symbols_and_harvest_unwind_data_for_pids(path_ref, &tracked_pids)
                .await
                .map_err(|e| {
                    error!("Failed to harvest jit dumps: {e}");
                    BenchmarkDataSaveError::FailedToHarvestJitDumps
                })?;

        let artifacts = save_artifacts::save_artifacts(
            path_ref,
            &loaded_modules_by_path,
            &jit_unwind_data_by_pid,
        );

        debug!("Saving metadata");
        #[allow(deprecated)]
        let metadata = PerfMetadata {
            version: PERF_METADATA_CURRENT_VERSION,
            integration: self
                .fifo_data
                .integration
                .clone()
                .ok_or(BenchmarkDataSaveError::MissingIntegration)?,
            uri_by_ts: self.marker_result.uri_by_ts.clone(),
            ignored_modules: artifacts.ignored_modules,
            markers: self.marker_result.markers.clone(),
            debug_info: artifacts.debug_info,
            mapped_process_debug_info_by_pid: artifacts.mapped_process_debug_info_by_pid,
            mapped_process_unwind_data_by_pid: artifacts.mapped_process_unwind_data_by_pid,
            mapped_process_module_symbols: artifacts.symbol_pid_mappings_by_pid,
            path_key_to_path: artifacts.key_to_path,
            // Not yet implemented
            ignored_modules_by_pid: Default::default(),
            // Deprecated fields below are no longer used
            debug_info_by_pid: Default::default(),
        };
        metadata.save_to(&path).unwrap();

        Ok(())
    }
}
