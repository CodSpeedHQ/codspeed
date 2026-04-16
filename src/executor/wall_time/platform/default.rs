use crate::executor::ExecutionContext;
use crate::executor::ToolStatus;
use crate::executor::helpers::command::CommandBuilder;
use crate::executor::helpers::run_command_with_log_pipe::run_command_with_log_pipe;
use crate::prelude::*;
use crate::system::SystemInfo;
use std::path::Path;
use std::process::ExitStatus;

pub struct WalltimePlatform;

impl WalltimePlatform {
    pub fn new() -> Self {
        Self
    }

    pub fn tool_status(&self) -> Option<ToolStatus> {
        None
    }

    pub async fn setup(
        &self,
        _system_info: &SystemInfo,
        _setup_cache_dir: Option<&Path>,
    ) -> Result<()> {
        Ok(())
    }

    pub async fn run_bench_cmd(
        &self,
        cmd_builder: CommandBuilder,
        _execution_context: &ExecutionContext,
    ) -> Result<ExitStatus> {
        let cmd = cmd_builder.build();
        debug!("cmd: {cmd:?}");
        run_command_with_log_pipe(cmd).await
    }

    pub async fn teardown(&self, _execution_context: &ExecutionContext) -> Result<()> {
        Ok(())
    }
}
