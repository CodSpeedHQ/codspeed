use super::super::perf::PerfRunner;
use crate::executor::ExecutionContext;
use crate::executor::ToolStatus;
use crate::executor::helpers::command::CommandBuilder;
use crate::executor::helpers::run_command_with_log_pipe::run_command_with_log_pipe;
use crate::executor::helpers::run_with_sudo::wrap_with_sudo;
use crate::prelude::*;
use crate::system::SystemInfo;
use std::path::Path;
use std::process::ExitStatus;

pub struct WalltimePlatform {
    perf: Option<PerfRunner>,
}

impl WalltimePlatform {
    pub fn new() -> Self {
        Self {
            perf: cfg!(target_os = "linux").then(PerfRunner::new),
        }
    }

    pub fn tool_status(&self) -> Option<ToolStatus> {
        self.perf
            .as_ref()
            .map(|_| super::super::perf::setup::get_perf_status())
    }

    pub async fn setup(
        &self,
        system_info: &SystemInfo,
        setup_cache_dir: Option<&Path>,
    ) -> Result<()> {
        if self.perf.is_some() {
            return PerfRunner::setup_environment(system_info, setup_cache_dir).await;
        }
        Ok(())
    }

    pub async fn run_bench_cmd(
        &self,
        cmd_builder: CommandBuilder,
        execution_context: &ExecutionContext,
    ) -> Result<ExitStatus> {
        if let Some(perf) = &self.perf
            && execution_context.config.enable_perf
        {
            perf.run(
                cmd_builder,
                &execution_context.config,
                &execution_context.profile_folder,
            )
            .await
        } else {
            let cmd = wrap_with_sudo(cmd_builder)?.build();
            debug!("cmd: {cmd:?}");
            run_command_with_log_pipe(cmd).await
        }
    }

    pub async fn teardown(&self, execution_context: &ExecutionContext) -> Result<()> {
        if let Some(perf) = &self.perf
            && execution_context.config.enable_perf
        {
            perf.save_files_to(&execution_context.profile_folder)
                .await?;
        }
        Ok(())
    }
}
