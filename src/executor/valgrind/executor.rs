use async_trait::async_trait;
use std::path::Path;

use crate::executor::Executor;
use crate::executor::ToolStatus;
use crate::executor::{ExecutionContext, ExecutorName};
use crate::instruments::mongo_tracer::MongoTracer;
use crate::prelude::*;
use crate::system::SystemInfo;

use super::setup::{get_valgrind_status, install_valgrind};
use super::{helpers::perf_maps::harvest_perf_maps, helpers::venv_compat, measure};

pub struct ValgrindExecutor;

#[async_trait(?Send)]
impl Executor for ValgrindExecutor {
    fn name(&self) -> ExecutorName {
        ExecutorName::Valgrind
    }

    fn tool_status(&self) -> ToolStatus {
        get_valgrind_status()
    }

    async fn setup(&self, system_info: &SystemInfo, setup_cache_dir: Option<&Path>) -> Result<()> {
        install_valgrind(system_info, setup_cache_dir).await?;

        if let Err(error) = venv_compat::symlink_libpython(None) {
            warn!("Failed to symlink libpython");
            debug!("Script error: {error}");
        }

        Ok(())
    }

    async fn run(
        &self,
        execution_context: &ExecutionContext,
        mongo_tracer: &Option<MongoTracer>,
    ) -> Result<()> {
        //TODO: add valgrind version check
        measure::measure(
            &execution_context.config,
            &execution_context.profile_folder,
            mongo_tracer,
        )
        .await?;

        Ok(())
    }

    async fn teardown(&self, execution_context: &ExecutionContext) -> Result<()> {
        harvest_perf_maps(&execution_context.profile_folder).await?;

        // No matter the command in input, at this point valgrind will have been run and have produced output files.
        //
        // Contrary to walltime, checking that benchmarks have been detected here would require
        // parsing the valgrind output files, which is not ideal at this stage.
        // A comprehensive message will be sent to the user if no benchmarks are detected,
        // even if it's later in the process than technically possible.

        Ok(())
    }
}
