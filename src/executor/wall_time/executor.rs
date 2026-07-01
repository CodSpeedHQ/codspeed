use super::helpers::validate_walltime_results;
use super::isolation::Isolation;
use super::profiler::Profiler;
use super::profiler::perf::PerfProfiler;
use super::profiler::samply::SamplyProfiler;
use crate::executor::Executor;
use crate::executor::ExecutorConfig;
use crate::executor::ToolStatus;
use crate::executor::config::WalltimeProfiler;
use crate::executor::helpers::command::CommandBuilder;
use crate::executor::helpers::env::{build_path_env, get_base_injected_env};
use crate::executor::helpers::get_bench_command::get_bench_command;
use crate::executor::helpers::run_command_with_log_pipe::run_command_with_log_pipe;
use crate::executor::helpers::run_command_with_log_pipe::run_command_with_log_pipe_and_callback;
use crate::executor::helpers::run_with_env::wrap_with_env;
use crate::executor::helpers::run_with_sudo::wrap_with_sudo;
use crate::executor::shared::fifo::FifoBenchmarkData;
use crate::executor::shared::fifo::RunnerFifo;
use crate::executor::{ExecutionContext, ExecutorName, ExecutorSupport};
use crate::instruments::mongo_tracer::MongoTracer;
use crate::prelude::*;
use crate::runner_mode::RunnerMode;
use crate::system::{SupportedOs, SystemInfo};
use async_trait::async_trait;
use runner_shared::artifacts::ExecutionTimestamps;
use runner_shared::fifo::Command as FifoCommand;
use runner_shared::fifo::IntegrationMode;
use std::cell::OnceCell;
use std::fs::canonicalize;
use std::io::Write;
use std::path::Path;
use tempfile::NamedTempFile;

pub struct WallTimeExecutor {
    profiler: Option<Box<dyn Profiler>>,

    /// Stashed by [`Executor::run`] and consumed by [`Executor::teardown`] to
    /// hand the run's outputs to [`Profiler::finalize`].
    benchmark_state: OnceCell<(FifoBenchmarkData, ExecutionTimestamps)>,
}

fn select_profiler(profiler_override: Option<WalltimeProfiler>) -> Option<Box<dyn Profiler>> {
    match profiler_override {
        Some(WalltimeProfiler::Perf) => Some(Box::new(PerfProfiler::new())),
        Some(WalltimeProfiler::Samply) => Some(Box::new(SamplyProfiler::new())),
        None => {
            if cfg!(target_os = "linux") {
                Some(Box::new(PerfProfiler::new()))
            } else if cfg!(target_os = "macos") {
                Some(Box::new(SamplyProfiler::new()))
            } else {
                None
            }
        }
    }
}

impl WallTimeExecutor {
    pub fn new(profiler_override: Option<WalltimeProfiler>) -> Self {
        Self {
            profiler: select_profiler(profiler_override),
            benchmark_state: OnceCell::new(),
        }
    }

    /// Prepare the benchmark command wrapped with the necessary environment variables for
    /// introspection and environment forwarding ahead of privilege escalation and isolation.
    fn walltime_bench_cmd(
        config: &ExecutorConfig,
        execution_context: &ExecutionContext,
    ) -> Result<(NamedTempFile, NamedTempFile, CommandBuilder)> {
        let path_value = build_path_env(config.enable_introspection)?;

        let mut extra_env = get_base_injected_env(
            RunnerMode::Walltime,
            &execution_context.profile_folder,
            &execution_context.config,
        );
        extra_env.insert("PATH".into(), path_value);

        // We have to write the benchmark command to a script, to ensure proper formatting
        // and to not have to manually escape everything.
        let mut script_file = NamedTempFile::new()?;
        script_file.write_all(get_bench_command(config)?.as_bytes())?;

        let mut bench_cmd = CommandBuilder::new("bash");
        bench_cmd.arg(script_file.path());
        let (mut bench_cmd, env_file) = wrap_with_env(bench_cmd, &extra_env)?;

        if let Some(cwd) = &config.working_directory {
            let abs_cwd = canonicalize(cwd)?;
            bench_cmd.current_dir(abs_cwd);
        }

        Ok((env_file, script_file, bench_cmd))
    }
}

#[async_trait(?Send)]
impl Executor for WallTimeExecutor {
    fn name(&self) -> ExecutorName {
        ExecutorName::WallTime
    }

    fn tool_status(&self) -> Option<ToolStatus> {
        self.profiler.as_ref().and_then(|p| p.tool_status())
    }

    fn support_level(&self, system_info: &SystemInfo) -> ExecutorSupport {
        match &system_info.os {
            SupportedOs::Linux(distro) if distro.is_supported() => ExecutorSupport::FullySupported,
            SupportedOs::Macos { .. } => ExecutorSupport::FullySupported,
            SupportedOs::Linux(_) => ExecutorSupport::RequiresManualInstallation,
        }
    }

    async fn setup(&self, system_info: &SystemInfo, setup_cache_dir: Option<&Path>) -> Result<()> {
        if let Some(profiler) = &self.profiler {
            profiler.setup(system_info, setup_cache_dir).await?;
        }
        Ok(())
    }

    async fn run(
        &mut self,
        execution_context: &ExecutionContext,
        _mongo_tracer: &Option<MongoTracer>,
    ) -> Result<()> {
        // Resolve isolation once: this runs the pre-bench setup, wraps the bench
        // leaf, and (for hook mode) runs post-bench on drop. Held for the whole run
        // so its teardown fires after the benchmark completes. `requires_sudo` is
        // read off it for the privilege wrapping below (or in the profiler).
        let isolation = Isolation::resolve();
        let requires_sudo = isolation.requires_sudo();

        let (_env_file, _script_file, cmd_builder) =
            WallTimeExecutor::walltime_bench_cmd(&execution_context.config, execution_context)?;
        let cmd_builder = isolation.wrap_bench(cmd_builder)?;

        // Split-borrow `self` so the closure inside `run_with_profiler` can
        // capture `benchmark_state` while we hold `&mut profiler`.
        let Self {
            profiler,
            benchmark_state,
        } = self;

        let status = match profiler.as_mut() {
            Some(profiler) if execution_context.config.enable_profiler => {
                run_with_profiler(
                    profiler.as_mut(),
                    cmd_builder,
                    &execution_context.config,
                    &execution_context.profile_folder,
                    requires_sudo,
                    benchmark_state,
                )
                .await
            }
            _ => {
                let cmd_builder = if requires_sudo {
                    wrap_with_sudo(cmd_builder)?
                } else {
                    cmd_builder
                };
                let cmd = cmd_builder.build();
                debug!("cmd: {cmd:?}");
                run_command_with_log_pipe(cmd).await
            }
        };

        let status = status.map_err(|e| anyhow!("failed to execute the benchmark process. {e}"))?;
        debug!("cmd exit status: {status:?}");

        if !status.success() {
            bail!("failed to execute the benchmark process: {status}");
        }

        Ok(())
    }

    async fn teardown(&self, execution_context: &ExecutionContext) -> Result<()> {
        debug!("Copying files to the profile folder");

        if let (Some(profiler), Some((fifo_data, timestamps))) =
            (&self.profiler, self.benchmark_state.get())
        {
            profiler
                .finalize(fifo_data, timestamps, &execution_context.profile_folder)
                .await?;
        }

        validate_walltime_results(
            &execution_context.profile_folder,
            execution_context.config.allow_empty,
        )?;

        Ok(())
    }
}

/// Drive a single benchmark run through a [`Profiler`]: wrap the command,
/// spawn it, dispatch FIFO commands from the integration into the profiler's
/// hooks, and stash the run's outputs for [`Profiler::finalize`] in teardown.
async fn run_with_profiler(
    profiler: &mut dyn Profiler,
    cmd_builder: CommandBuilder,
    config: &ExecutorConfig,
    profile_folder: &Path,
    requires_sudo: bool,
    benchmark_state: &OnceCell<(FifoBenchmarkData, ExecutionTimestamps)>,
) -> Result<std::process::ExitStatus> {
    let wrapped = profiler
        .wrap_command(cmd_builder, config, profile_folder, requires_sudo)
        .await?;
    let cmd = wrapped.build();
    debug!("cmd: {cmd:?}");

    let mut runner_fifo = RunnerFifo::new()?;

    run_command_with_log_pipe_and_callback(cmd, async move |mut child| {
        let on_cmd = async |c: &FifoCommand| match c {
            FifoCommand::StartProfiler => {
                profiler.on_start_profiler().await?;
                Ok(None)
            }
            FifoCommand::StopProfiler => {
                profiler.on_stop_profiler().await?;
                Ok(None)
            }
            #[allow(deprecated)]
            FifoCommand::PingProfiler => Ok(Some(if profiler.on_ping().await? {
                FifoCommand::Ack
            } else {
                FifoCommand::Err
            })),
            FifoCommand::GetIntegrationMode => Ok(Some(FifoCommand::IntegrationModeResponse(
                IntegrationMode::Walltime,
            ))),
            _ => Ok(None),
        };

        let (timestamps, fifo_data, exit_status) =
            runner_fifo.handle_fifo_messages(&mut child, on_cmd).await?;

        let _ = benchmark_state.set((fifo_data, timestamps));

        Ok(exit_status)
    })
    .await
}
