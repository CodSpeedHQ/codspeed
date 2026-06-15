use std::fmt::Display;

pub mod config;
mod execution_context;
pub(crate) mod helpers;
mod interfaces;
mod memory;
pub mod orchestrator;
mod shared;
#[cfg(test)]
mod tests;
mod valgrind;
mod wall_time;

use crate::instruments::mongo_tracer::{MongoTracer, install_mongodb_tracer};
use crate::prelude::*;
use crate::runner_mode::RunnerMode;
use crate::system::SystemInfo;
use async_trait::async_trait;
pub use config::{BenchmarkTarget, ExecutorConfig, WalltimeProfiler};
pub use execution_context::ExecutionContext;
pub use interfaces::ExecutorName;
pub use orchestrator::Orchestrator;

use memory::executor::MemoryExecutor;
use std::path::Path;
use valgrind::executor::ValgrindExecutor;
use wall_time::executor::WallTimeExecutor;

impl Display for RunnerMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[allow(deprecated)]
            RunnerMode::Instrumentation => write!(f, "instrumentation"),
            RunnerMode::Simulation => write!(f, "simulation"),
            RunnerMode::Walltime => write!(f, "walltime"),
            RunnerMode::Memory => write!(f, "memory"),
        }
    }
}

pub const EXECUTOR_TARGET: &str = "executor";

pub fn get_executor_from_mode(
    mode: &RunnerMode,
    walltime_profiler: Option<WalltimeProfiler>,
) -> Box<dyn Executor> {
    match mode {
        #[allow(deprecated)]
        RunnerMode::Instrumentation | RunnerMode::Simulation => Box::new(ValgrindExecutor),
        RunnerMode::Walltime => Box::new(WallTimeExecutor::new(walltime_profiler)),
        RunnerMode::Memory => Box::new(MemoryExecutor),
    }
}

pub fn get_all_executors() -> Vec<Box<dyn Executor>> {
    vec![
        Box::new(ValgrindExecutor),
        Box::new(WallTimeExecutor::new(None)),
        Box::new(MemoryExecutor),
    ]
}

/// Installation status of a tool required by an executor.
pub struct ToolStatus {
    pub tool_name: String,
    pub status: ToolInstallStatus,
}

pub enum ToolInstallStatus {
    /// Tool is installed with a correct, compatible version.
    Installed { version: String },
    /// Tool is installed but has a version issue (wrong version, not a CodSpeed build, etc.).
    IncorrectVersion { version: String, message: String },
    /// Tool is not installed at all.
    NotInstalled,
}

/// Readiness of any elevated privileges an executor needs beyond a plain install
/// (e.g. file capabilities). Reported alongside [`ToolStatus`] in `setup status`.
pub enum PrivilegeStatus {
    /// Privileges are in place; `detail` explains how (root, capabilities granted, …).
    Satisfied { detail: String },
    /// Privileges are missing; `message` tells the user how to obtain them.
    Missing { message: String },
}

/// How well a given executor runs on a given [`SupportedOs`].
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum ExecutorSupport {
    /// The executor cannot run on this OS at all — `run_executor` hard-bails.
    Unsupported,
    /// The executor runs on this OS, but the user is responsible for installing the required tooling themselves.
    RequiresManualInstallation,
    /// The executor runs on this OS and `setup()` knows how to auto-install tooling.
    FullySupported,
}

#[async_trait(?Send)]
pub trait Executor {
    fn name(&self) -> ExecutorName;

    /// Report the installation status of the tool(s) this executor depends on.
    fn tool_status(&self) -> Option<ToolStatus>;

    /// Report whether the elevated privileges this executor needs are in place.
    /// Defaults to `None` for executors that need none. Only consulted once the
    /// tool itself is installed.
    fn privilege_status(&self) -> Option<PrivilegeStatus> {
        None
    }

    /// Declare how well this executor runs on the host system. Drives whether `setup()` is invoked
    /// (only when [`ExecutorSupport::FullySupported`]) and whether we bail out of running the
    /// executor at all (on [`ExecutorSupport::Unsupported`]).
    fn support_level(&self, system_info: &SystemInfo) -> ExecutorSupport;

    async fn setup(
        &self,
        _system_info: &SystemInfo,
        _setup_cache_dir: Option<&Path>,
    ) -> Result<()> {
        Ok(())
    }

    /// Grant any elevated privileges this executor needs (e.g. file capabilities).
    /// Runs after [`setup`](Self::setup) and may prompt for sudo. Defaults to a
    /// no-op for executors that need none.
    fn grant_privileges(&self) -> Result<()> {
        Ok(())
    }

    /// Runs the executor
    async fn run(
        &mut self,
        execution_context: &ExecutionContext,
        // TODO: use Instruments instead of directly passing the mongodb tracer
        mongo_tracer: &Option<MongoTracer>,
    ) -> Result<()>;

    async fn teardown(&self, execution_context: &ExecutionContext) -> Result<()>;
}

/// Run a single executor: setup → run → teardown → persist logs.
/// Does NOT upload.
pub async fn run_executor(
    executor: &mut dyn Executor,
    orchestrator: &Orchestrator,
    execution_context: &ExecutionContext,
    setup_cache_dir: Option<&Path>,
) -> Result<()> {
    match executor.support_level(&orchestrator.system_info) {
        ExecutorSupport::Unsupported => {
            bail!(
                "The {} executor is not supported on {}",
                executor.name(),
                orchestrator.system_info.os
            );
        }
        ExecutorSupport::RequiresManualInstallation | ExecutorSupport::FullySupported => {
            if !execution_context.config.skip_setup {
                executor
                    .setup(&orchestrator.system_info, setup_cache_dir)
                    .await?;
                executor.grant_privileges()?;
            }
        }
    }

    if !execution_context.config.skip_setup {
        // TODO: refactor and move directly in the Instruments struct as a `setup` method
        if execution_context.config.instruments.is_mongodb_enabled() {
            install_mongodb_tracer().await?;
        }

        debug!("Environment ready");
    }

    if !execution_context.config.skip_run {
        // TODO: refactor and move directly in the Instruments struct as a `start` method
        let mongo_tracer =
            if let Some(mongodb_config) = &execution_context.config.instruments.mongodb {
                let mut mongo_tracer =
                    MongoTracer::try_from(&execution_context.profile_folder, mongodb_config)?;
                mongo_tracer.start().await?;
                Some(mongo_tracer)
            } else {
                None
            };

        executor.run(execution_context, &mongo_tracer).await?;

        // TODO: refactor and move directly in the Instruments struct as a `stop` method
        if let Some(mut mongo_tracer) = mongo_tracer {
            mongo_tracer.stop().await?;
        }
        debug!("Tearing down the executor");
        executor.teardown(execution_context).await?;

        orchestrator
            .logger
            .persist_log_to_profile_folder(&execution_context.profile_folder)?;
    } else {
        debug!("Skipping the run of the benchmarks");
    };

    Ok(())
}
