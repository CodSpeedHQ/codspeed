use super::helpers::validate_walltime_results;
use super::isolation::wrap_with_isolation;
use super::profiler::Profiler;
use super::profiler::perf::PerfProfiler;
use super::profiler::samply::SamplyProfiler;
use crate::executor::Executor;
use crate::executor::ExecutorConfig;
use crate::executor::ToolStatus;
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
use std::path::PathBuf;
use std::process::Command;
use tempfile::NamedTempFile;

struct HookScriptsGuard {
    post_bench_script: PathBuf,
}

impl HookScriptsGuard {
    fn execute_script_from_path<P: AsRef<Path>>(path: P) -> anyhow::Result<()> {
        let path = path.as_ref();
        if !path.exists() || !path.is_file() {
            debug!("Script not found or not a file: {}", path.display());
            return Ok(());
        }

        let output = Command::new("bash").args([&path]).output()?;
        if !output.status.success() {
            debug!("stdout: {}", String::from_utf8_lossy(&output.stdout));
            debug!("stderr: {}", String::from_utf8_lossy(&output.stderr));
            bail!("Failed to execute script: {}", path.display());
        }

        Ok(())
    }

    pub fn setup_with_scripts<P: AsRef<Path>>(pre_bench_script: P, post_bench_script: P) -> Self {
        if let Err(e) = Self::execute_script_from_path(pre_bench_script.as_ref()) {
            debug!("Failed to execute pre-bench script: {e}");
        }

        Self {
            post_bench_script: post_bench_script.as_ref().to_path_buf(),
        }
    }

    pub fn setup() -> Self {
        Self::setup_with_scripts(
            "/usr/local/bin/codspeed-pre-bench",
            "/usr/local/bin/codspeed-post-bench",
        )
    }
}

impl Drop for HookScriptsGuard {
    fn drop(&mut self) {
        if let Err(e) = Self::execute_script_from_path(&self.post_bench_script) {
            debug!("Failed to execute post-bench script: {e}");
        }
    }
}

pub struct WallTimeExecutor {
    profiler: Option<Box<dyn Profiler>>,

    /// Stashed by [`Executor::run`] and consumed by [`Executor::teardown`] to
    /// hand the run's outputs to [`Profiler::finalize`].
    benchmark_state: OnceCell<(FifoBenchmarkData, ExecutionTimestamps)>,
}

fn select_profiler() -> Option<Box<dyn Profiler>> {
    const PROFILER_OVERRIDE_ENV: &str = "CODSPEED_WALLTIME_PROFILER";

    match std::env::var(PROFILER_OVERRIDE_ENV).ok().as_deref() {
        Some("perf") => return Some(Box::new(PerfProfiler::new())),
        Some("samply") => return Some(Box::new(SamplyProfiler::new())),
        Some(other) => {
            warn!(
                "Ignoring unknown {PROFILER_OVERRIDE_ENV}={other:?}; expected `perf` or `samply`."
            );
        }
        None => {}
    }

    if cfg!(target_os = "linux") {
        Some(Box::new(PerfProfiler::new()))
    } else if cfg!(target_os = "macos") {
        Some(Box::new(SamplyProfiler::new()))
    } else {
        None
    }
}

impl WallTimeExecutor {
    pub fn new() -> Self {
        Self {
            profiler: select_profiler(),
            benchmark_state: OnceCell::new(),
        }
    }

    fn walltime_bench_cmd(
        config: &ExecutorConfig,
        execution_context: &ExecutionContext,
        isolate: bool,
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

        let bench_cmd = if isolate {
            wrap_with_isolation(bench_cmd)?
        } else {
            bench_cmd
        };

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
        let _guard = HookScriptsGuard::setup();

        let isolate = self
            .profiler
            .as_ref()
            .is_none_or(|p| p.requires_isolation());
        let (_env_file, _script_file, cmd_builder) = WallTimeExecutor::walltime_bench_cmd(
            &execution_context.config,
            execution_context,
            isolate,
        )?;

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
                    benchmark_state,
                )
                .await
            }
            _ => {
                let cmd_builder = if cfg!(target_os = "linux") {
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
    benchmark_state: &OnceCell<(FifoBenchmarkData, ExecutionTimestamps)>,
) -> Result<std::process::ExitStatus> {
    let wrapped = profiler
        .wrap_command(cmd_builder, config, profile_folder)
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

#[cfg(test)]
mod tests {
    use tempfile::NamedTempFile;

    use super::*;
    use std::{
        io::{Read, Write},
        os::unix::fs::PermissionsExt,
    };

    #[test]
    fn test_env_guard_no_crash() {
        fn create_run_script(content: &str) -> anyhow::Result<NamedTempFile> {
            let rwx = std::fs::Permissions::from_mode(0o777);
            let mut script_file = tempfile::Builder::new()
                .suffix(".sh")
                .permissions(rwx)
                .disable_cleanup(true)
                .tempfile()?;
            script_file.write_all(content.as_bytes())?;

            Ok(script_file)
        }

        let mut tmp_dst = tempfile::NamedTempFile::new().unwrap();

        let pre_script = create_run_script(&format!(
            "#!/usr/bin/env bash\necho \"pre\" >> {}",
            tmp_dst.path().display()
        ))
        .unwrap();
        let post_script = create_run_script(&format!(
            "#!/usr/bin/env bash\necho \"post\" >> {}",
            tmp_dst.path().display()
        ))
        .unwrap();

        {
            let _guard =
                HookScriptsGuard::setup_with_scripts(pre_script.path(), post_script.path());
        }

        let mut result = String::new();
        tmp_dst.read_to_string(&mut result).unwrap();
        assert_eq!(result, "pre\npost\n");
    }
}
