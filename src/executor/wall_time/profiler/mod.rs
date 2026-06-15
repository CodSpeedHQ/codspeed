//! Abstraction for profiling the wall time of a command execution.
//!
//! A [`Profiler`] wraps the user's benchmark command with a sampling tool
//! (perf, samply, instruments, ...) and produces a unified set of artifacts
//! in the profile folder.

mod linux_sysctl;
pub mod perf;
pub mod samply;

const WALLTIME_METADATA_CURRENT_VERSION: u64 = 1;

/// Sampling rate (Hz) shared by all walltime profilers. A prime number to
/// avoid synchronization with periodic tasks.
pub(super) const SAMPLING_RATE_HZ: u32 = 997;

/// Warning shown when walltime profiling ran but no benchmarks were detected.
pub(super) const NO_BENCHMARKS_DETECTED_WARNING: &str = "Walltime profiling is enabled, but failed to detect benchmarks. \
     If you wish to disable this warning, set CODSPEED_PROFILER_ENABLED=false";

use crate::executor::ExecutorConfig;
use crate::executor::ToolStatus;
use crate::executor::helpers::command::CommandBuilder;
use crate::executor::shared::fifo::FifoBenchmarkData;
use crate::system::SystemInfo;
use async_trait::async_trait;
use runner_shared::artifacts::ExecutionTimestamps;
use std::path::Path;

#[async_trait(?Send)]
pub trait Profiler {
    fn tool_status(&self) -> Option<ToolStatus> {
        None
    }

    /// One-time system setup (install tool, tweak sysctls, ...).
    async fn setup(
        &self,
        _system_info: &SystemInfo,
        _setup_cache_dir: Option<&Path>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Wrap the user command with the profiler invocation. The returned
    /// `CommandBuilder` is what gets spawned.
    ///
    /// Profilers stash any live state they need for the duration of the run
    /// (control fifos, output paths) on `self`.
    async fn wrap_command(
        &mut self,
        cmd: CommandBuilder,
        config: &ExecutorConfig,
        profile_folder: &Path,
    ) -> anyhow::Result<CommandBuilder>;

    /// The benchmarked process signaled the start of a measured region.
    async fn on_start_profiler(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    /// The benchmarked process signaled the end of a measured region.
    async fn on_stop_profiler(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Health-check ping from the benchmarked process. Returning `false`
    /// indicates the profiler is unhealthy and the harness should report an
    /// error to the integration.
    async fn on_ping(&mut self) -> anyhow::Result<bool> {
        Ok(true)
    }

    /// Post-run: harvest any side artifacts (perf maps, jit dumps, module
    /// info) and write the unified profile metadata into `profile_folder`.
    async fn finalize(
        &self,
        fifo_data: &FifoBenchmarkData,
        timestamps: &ExecutionTimestamps,
        profile_folder: &Path,
    ) -> anyhow::Result<()>;
}
