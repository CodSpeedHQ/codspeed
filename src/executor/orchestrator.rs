use super::{ExecutionContext, ExecutorName, get_executor_from_mode, run_executor};
use crate::api_client::CodSpeedAPIClient;
use crate::binary_installer::ensure_binary_installed;
use crate::cli::exec::{EXEC_HARNESS_COMMAND, EXEC_HARNESS_VERSION, multi_targets};
use crate::cli::run::logger::Logger;
use crate::config::CodSpeedConfig;
use crate::executor::config::BenchmarkTarget;
use crate::executor::config::OrchestratorConfig;
use crate::prelude::*;
use crate::run_environment::{self, RunEnvironment, RunEnvironmentProvider};
use crate::runner_mode::RunnerMode;
use crate::system::{self, SystemInfo};
use crate::upload::{UploadResult, upload};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// Shared orchestration state created once per CLI invocation.
///
/// Holds the run-level configuration, environment provider, system info, and logger.
pub struct Orchestrator {
    pub config: OrchestratorConfig,
    pub system_info: SystemInfo,
    pub provider: Box<dyn RunEnvironmentProvider>,
    pub logger: Logger,
}

impl Orchestrator {
    pub fn is_local(&self) -> bool {
        self.provider.get_run_environment() == RunEnvironment::Local
    }

    pub async fn new(
        mut config: OrchestratorConfig,
        codspeed_config: &CodSpeedConfig,
        api_client: &CodSpeedAPIClient,
    ) -> Result<Self> {
        let provider = run_environment::get_provider(&config, api_client).await?;
        let system_info = SystemInfo::new()?;
        system::check_system(&system_info)?;
        let logger = Logger::new(provider.as_ref())?;

        if provider.get_run_environment() == RunEnvironment::Local {
            if codspeed_config.auth.token.is_none() {
                bail!("You have to authenticate the CLI first. Run `codspeed auth login`.");
            }
            debug!("Using the token from the CodSpeed configuration file");
            config.set_token(codspeed_config.auth.token.clone());
        }

        #[allow(deprecated)]
        if config.modes.contains(&RunnerMode::Instrumentation) {
            warn!(
                "The 'instrumentation' runner mode is deprecated and will be removed in a future version. \
                Please use 'simulation' instead."
            );
        }

        Ok(Orchestrator {
            config,
            system_info,
            provider,
            logger,
        })
    }

    /// Execute the benchmark target for all configured modes, then upload results.
    ///
    /// Reads the target from `self.config.target`:
    /// - `Exec`: installs exec-harness, wraps the command, then runs all modes
    /// - `Entrypoint`: runs the command directly across all modes
    pub async fn execute<F>(&self, setup_cache_dir: Option<&Path>, poll_results: F) -> Result<()>
    where
        F: AsyncFn(&UploadResult) -> Result<()>,
    {
        match &self.config.target {
            BenchmarkTarget::Exec {
                command,
                name,
                walltime_args,
            } => {
                ensure_binary_installed(EXEC_HARNESS_COMMAND, EXEC_HARNESS_VERSION, || {
                    format!(
                        "https://github.com/CodSpeedHQ/codspeed/releases/download/exec-harness-v{EXEC_HARNESS_VERSION}/exec-harness-installer.sh"
                    )
                })
                .await?;

                let single_target = BenchmarkTarget::Exec {
                    command: command.clone(),
                    name: name.clone(),
                    walltime_args: walltime_args.clone(),
                };
                let pipe_cmd = multi_targets::build_exec_targets_pipe_command(&[&single_target])?;
                let completed_runs = self.run_all_modes(pipe_cmd, setup_cache_dir).await?;
                self.upload_and_poll(completed_runs, &poll_results).await?;
            }
            BenchmarkTarget::Entrypoint { command, .. } => {
                let completed_runs = self.run_all_modes(command.clone(), setup_cache_dir).await?;
                self.upload_and_poll(completed_runs, &poll_results).await?;
            }
        }

        Ok(())
    }

    /// Run the given command across all configured modes, returning completed run contexts.
    async fn run_all_modes(
        &self,
        command: String,
        setup_cache_dir: Option<&Path>,
    ) -> Result<Vec<(ExecutionContext, ExecutorName)>> {
        let modes = &self.config.modes;
        let is_multi_mode = modes.len() > 1;
        let mut completed_runs: Vec<(ExecutionContext, ExecutorName)> = vec![];
        if !self.config.skip_run {
            start_opened_group!("Running the benchmarks");
        }
        for mode in modes {
            if is_multi_mode {
                info!("Running benchmarks for {mode} mode");
            }
            let mut per_mode_config = self.config.executor_config_for_command(command.clone());
            // For multi-mode runs, always create a fresh profile folder per mode
            // even if the user specified one (to avoid modes overwriting each other).
            if is_multi_mode {
                per_mode_config.profile_folder = None;
            }
            let ctx = ExecutionContext::new(per_mode_config)?;
            let executor = get_executor_from_mode(mode);

            run_executor(executor.as_ref(), self, &ctx, setup_cache_dir).await?;
            completed_runs.push((ctx, executor.name()));
        }
        if !self.config.skip_run {
            end_group!();
        }
        Ok(completed_runs)
    }

    /// Upload completed runs and poll results.
    async fn upload_and_poll<F>(
        &self,
        mut completed_runs: Vec<(ExecutionContext, ExecutorName)>,
        poll_results: F,
    ) -> Result<()>
    where
        F: AsyncFn(&UploadResult) -> Result<()>,
    {
        let skip_upload = self.config.skip_upload;

        if !skip_upload {
            start_group!("Uploading results");
            let last_upload_result = self.upload_all(&mut completed_runs).await?;
            end_group!();

            if self.is_local() {
                poll_results(&last_upload_result).await?;
            }
        } else {
            debug!("Skipping upload of performance data");
        }

        Ok(())
    }

    /// Build the structured suffix that differentiates this upload within the run.
    /// This is a temporary implementation
    fn build_run_part_suffix(executor_name: &ExecutorName) -> BTreeMap<String, Value> {
        BTreeMap::from([(
            "executor".to_string(),
            Value::from(executor_name.to_string()),
        )])
    }

    pub async fn upload_all(
        &self,
        completed_runs: &mut [(ExecutionContext, ExecutorName)],
    ) -> Result<UploadResult> {
        let mut last_upload_result: Option<UploadResult> = None;

        let total_runs = completed_runs.len();
        for (ctx, executor_name) in completed_runs.iter_mut() {
            if !self.is_local() {
                // OIDC tokens can expire quickly, so refresh just before each upload
                self.provider.set_oidc_token(&mut ctx.config).await?;
            }

            if total_runs > 1 {
                info!("Uploading results for {executor_name:?} executor");
            }
            let run_part_suffix = Self::build_run_part_suffix(executor_name);
            let upload_result = upload(self, ctx, executor_name.clone(), run_part_suffix).await?;
            last_upload_result = Some(upload_result);
        }
        info!("Performance data uploaded");
        if let Some(upload_result) = &last_upload_result {
            info!(
                "Linked repository: {}",
                console::style(format!(
                    "{}/{}",
                    upload_result.owner, upload_result.repository
                ))
                .bold()
            );
        }

        last_upload_result.ok_or_else(|| anyhow::anyhow!("No completed runs to upload"))
    }
}
