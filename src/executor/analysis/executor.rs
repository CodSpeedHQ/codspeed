use super::instruments::enabled_instruments;
use crate::executor::helpers::command::CommandBuilder;
use crate::executor::helpers::env::{build_path_env, get_base_injected_env};
use crate::executor::helpers::get_bench_command::get_bench_command;
use crate::executor::helpers::run_command_with_log_pipe::run_command_with_log_pipe_and_callback;
use crate::executor::helpers::run_with_env::wrap_with_env;
use crate::executor::shared::fifo::RunnerFifo;
use crate::executor::{ExecutionContext, Executor, ExecutorName, ExecutorSupport, ToolStatus};
use crate::instruments::mongo_tracer::MongoTracer;
use crate::prelude::*;
use crate::runner_mode::RunnerMode;
use crate::system::SystemInfo;
use async_trait::async_trait;
use runner_shared::artifacts::{ArtifactExt, ExecutionTimestamps};
use runner_shared::fifo::Command as FifoCommand;
use runner_shared::fifo::IntegrationMode;
use std::cell::OnceCell;
use std::collections::HashMap;
use std::fs::canonicalize;
use std::io::Write;
use tempfile::NamedTempFile;

/// Runs the benchmarks once each in analysis mode, without a profiler, with the
/// [analysis instruments](super::instruments) enabled in the configuration hooked on.
///
/// The benchmark markers are saved in the profile folder, so that what the instruments collect
/// can be placed on the clock of each benchmark.
pub struct AnalysisExecutor {
    /// The runner mode the benchmarks are told they run in.
    mode: RunnerMode,
    /// Stashed by [`Executor::run`] for [`Executor::teardown`].
    timestamps: OnceCell<ExecutionTimestamps>,
}

impl AnalysisExecutor {
    pub fn new(mode: RunnerMode) -> Self {
        Self {
            mode,
            timestamps: OnceCell::new(),
        }
    }

    fn bench_cmd(
        &self,
        execution_context: &ExecutionContext,
        instruments_env: HashMap<String, String>,
    ) -> Result<(NamedTempFile, NamedTempFile, CommandBuilder)> {
        let config = &execution_context.config;
        let mut extra_env =
            get_base_injected_env(self.mode.clone(), &execution_context.profile_folder, config);
        extra_env.insert("PATH".into(), build_path_env(config.enable_introspection)?);
        extra_env.extend(instruments_env);

        // Written to a script to keep the command's formatting without escaping it.
        let mut script_file = NamedTempFile::new()?;
        script_file.write_all(get_bench_command(config)?.as_bytes())?;

        let mut bench_cmd = CommandBuilder::new("bash");
        bench_cmd.arg(script_file.path());
        let (mut bench_cmd, env_file) = wrap_with_env(bench_cmd, &extra_env)?;

        if let Some(cwd) = &config.working_directory {
            bench_cmd.current_dir(canonicalize(cwd)?);
        }

        Ok((env_file, script_file, bench_cmd))
    }
}

#[async_trait(?Send)]
impl Executor for AnalysisExecutor {
    fn name(&self) -> ExecutorName {
        ExecutorName::Analysis
    }

    fn tool_status(&self) -> Option<ToolStatus> {
        None
    }

    fn support_level(&self, _system_info: &SystemInfo) -> ExecutorSupport {
        ExecutorSupport::FullySupported
    }

    async fn run(
        &mut self,
        execution_context: &ExecutionContext,
        _mongo_tracer: &Option<MongoTracer>,
    ) -> Result<()> {
        let mut instruments = enabled_instruments(&execution_context.config.instruments);
        let mut instruments_env = HashMap::new();
        for instrument in instruments.iter_mut() {
            instrument
                .start(&execution_context.profile_folder, &mut instruments_env)
                .await?;
        }

        let (_env_file, _script_file, cmd_builder) =
            self.bench_cmd(execution_context, instruments_env)?;
        let cmd = cmd_builder.build().into();
        debug!("cmd: {cmd:?}");

        let mut runner_fifo = RunnerFifo::new()?;
        let timestamps = &self.timestamps;
        let fifo_instruments = &mut instruments;
        let result = run_command_with_log_pipe_and_callback(cmd, async move |mut child| {
            let on_cmd = async |cmd: &FifoCommand| {
                match cmd {
                    // Run each benchmark once.
                    FifoCommand::GetIntegrationMode => {
                        return Ok(Some(FifoCommand::IntegrationModeResponse(
                            IntegrationMode::Analysis,
                        )));
                    }
                    FifoCommand::StartProfiler => {
                        for instrument in fifo_instruments.iter_mut() {
                            instrument.on_start_profiler().await?;
                        }
                    }
                    FifoCommand::StopProfiler => {
                        for instrument in fifo_instruments.iter_mut() {
                            instrument.on_stop_profiler().await?;
                        }
                    }
                    _ => {}
                }
                Ok(None)
            };
            let (marker_result, _fifo_data, exit_status) =
                runner_fifo.handle_fifo_messages(&mut child, on_cmd).await?;
            let _ = timestamps.set(marker_result);
            Ok(exit_status)
        })
        .await;

        // Stopped even when the process failed, to release what they hold.
        for instrument in instruments.iter_mut() {
            instrument.stop().await?;
        }

        let status = result.map_err(|e| anyhow!("failed to execute the benchmark process. {e}"))?;
        debug!("cmd exit status: {status:?}");

        if !status.success() {
            bail!("failed to execute the benchmark process: {status}");
        }

        Ok(())
    }

    async fn teardown(&self, execution_context: &ExecutionContext) -> Result<()> {
        let Some(timestamps) = self.timestamps.get() else {
            bail!("The benchmark process did not report its benchmarks");
        };
        if timestamps.uri_by_ts.is_empty() {
            let message = format!("No benchmarks were run in {} mode", self.mode);
            if !execution_context.config.allow_empty {
                bail!(
                    "{message}. Were they built for it, e.g. with `cargo codspeed build -m {}`?",
                    self.mode
                );
            }
            info!("{message}.");
        }
        timestamps.save_to(&execution_context.profile_folder)?;
        Ok(())
    }
}
