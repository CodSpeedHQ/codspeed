use super::{ExecutionContext, ExecutorName, get_executor_from_mode, run_executor};
use crate::api_client::CodSpeedAPIClient;
use crate::binary_installer::ensure_binary_installed;
use crate::cli::exec::multi_targets;
use crate::cli::run::logger::Logger;
use crate::config::CodSpeedConfig;
use crate::executor::config::BenchmarkTarget;
use crate::executor::config::OrchestratorConfig;
use crate::executor::helpers::profile_folder::create_profile_folder;
use crate::local_logger::rolling_buffer::{activate_rolling_buffer, deactivate_rolling_buffer};
use crate::prelude::*;
use crate::run_environment::{self, RunEnvironment, RunEnvironmentProvider};
use crate::runner_mode::RunnerMode;
use crate::system::{self, SystemInfo};
use crate::upload::{UploadResult, upload};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const EXEC_HARNESS_COMMAND: &str = "exec-harness";
pub const EXEC_HARNESS_VERSION: &str = "1.2.0";

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

    /// Execute all benchmark targets for all configured modes, then upload results.
    ///
    /// Flattens all `(command, mode)` pairs into a single iteration:
    /// - All `Exec` targets are combined into a single exec-harness command
    /// - Each `Entrypoint` target produces its own command
    /// - Each command is crossed with every configured mode
    ///
    /// Each `(command, mode)` pair gets its own profile folder. When the user
    /// specifies `--profile-folder` and there are multiple pairs, deterministic
    /// subdirectories (`<mode>-<index>`) are created under that folder.
    pub async fn execute<F>(&self, setup_cache_dir: Option<&Path>, poll_results: F) -> Result<()>
    where
        F: AsyncFn(&UploadResult) -> Result<()>,
    {
        // Build (command, label) pairs while we still know the target type
        let mut command_labels: Vec<(String, String)> = vec![];

        let exec_targets: Vec<&BenchmarkTarget> = self
            .config
            .targets
            .iter()
            .filter(|t| matches!(t, BenchmarkTarget::Exec { .. }))
            .collect();

        if !exec_targets.is_empty() {
            ensure_binary_installed(EXEC_HARNESS_COMMAND, EXEC_HARNESS_VERSION, || {
                format!(
                    "https://github.com/CodSpeedHQ/codspeed/releases/download/exec-harness-v{EXEC_HARNESS_VERSION}/exec-harness-installer.sh"
                )
            })
            .await?;

            let pipe_cmd = multi_targets::build_exec_targets_pipe_command(&exec_targets)?;
            let label = match exec_targets.as_slice() {
                [BenchmarkTarget::Exec { command, .. }] => {
                    format!("Running `{}` with exec-harness", command.join(" "))
                }
                targets => format!("Running {} commands with exec-harness", targets.len()),
            };
            command_labels.push((pipe_cmd, label));
        }

        for target in &self.config.targets {
            if let BenchmarkTarget::Entrypoint { command, .. } = target {
                command_labels.push((command.clone(), command.clone()));
            }
        }

        struct ExecutorTarget<'a> {
            command: String,
            mode: &'a RunnerMode,
            label: String,
        }

        // Flatten into (command, mode) run parts
        let modes = &self.config.modes;
        let run_parts: Vec<ExecutorTarget> = command_labels
            .iter()
            .flat_map(|(cmd, label)| {
                modes.iter().map(move |mode| ExecutorTarget {
                    command: cmd.clone(),
                    mode,
                    label: format!("[{mode}] {label}"),
                })
            })
            .collect();

        let total_parts = run_parts.len();
        let mut all_completed_runs = vec![];

        if !self.config.skip_run {
            start_opened_group!("Running the benchmarks");
        }

        for (run_part_index, part) in run_parts.into_iter().enumerate() {
            let config = self.config.executor_config_for_command(part.command);
            let profile_folder =
                self.resolve_profile_folder(part.mode, run_part_index, total_parts)?;

            let ctx = ExecutionContext::new(config, profile_folder);
            let executor = get_executor_from_mode(part.mode);

            activate_rolling_buffer(&part.label);

            run_executor(executor.as_ref(), self, &ctx, setup_cache_dir).await?;

            deactivate_rolling_buffer();
            all_completed_runs.push((ctx, executor.name()));
        }

        if !self.config.skip_run {
            end_group!();
        }

        self.upload_and_poll(all_completed_runs, &poll_results)
            .await?;

        Ok(())
    }

    /// Resolve the profile folder for a given run part.
    ///
    /// - Single run part + user-specified folder: use as-is
    /// - Multiple run parts + user-specified folder: `<folder>/<mode>-<index>`
    /// - No user-specified folder: create a random temp folder
    fn resolve_profile_folder(
        &self,
        mode: &RunnerMode,
        run_part_index: usize,
        total_parts: usize,
    ) -> Result<PathBuf> {
        match (&self.config.profile_folder, total_parts) {
            (Some(folder), 1) => Ok(folder.clone()),
            (Some(folder), _) => {
                let subfolder = folder.join(format!("{mode}-{run_part_index}"));
                std::fs::create_dir_all(&subfolder).with_context(|| {
                    format!(
                        "Failed to create profile subfolder: {}",
                        subfolder.display()
                    )
                })?;
                Ok(subfolder)
            }
            (None, _) => create_profile_folder(),
        }
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
    fn build_run_part_suffix(
        executor_name: &ExecutorName,
        run_part_index: usize,
        total_runs: usize,
    ) -> BTreeMap<String, Value> {
        let mut suffix = BTreeMap::from([(
            "executor".to_string(),
            Value::from(executor_name.to_string()),
        )]);
        if total_runs > 1 {
            suffix.insert("run-part-index".to_string(), Value::from(run_part_index));
        }
        suffix
    }

    pub async fn upload_all(
        &self,
        completed_runs: &mut [(ExecutionContext, ExecutorName)],
    ) -> Result<UploadResult> {
        let mut last_upload_result: Option<UploadResult> = None;

        let total_runs = completed_runs.len();
        for (run_part_index, (ctx, executor_name)) in completed_runs.iter_mut().enumerate() {
            if !self.is_local() {
                // OIDC tokens can expire quickly, so refresh just before each upload
                self.provider.set_oidc_token(&mut ctx.config).await?;
            }

            if total_runs > 1 {
                info!("Uploading results {}/{total_runs}", run_part_index + 1);
            }
            let run_part_suffix =
                Self::build_run_part_suffix(executor_name, run_part_index, total_runs);
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
