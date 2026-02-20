use super::{Config, ExecutionContext, ExecutorName, get_executor_from_mode, run_executor};
use crate::api_client::CodSpeedAPIClient;
use crate::cli::run::logger::Logger;
use crate::config::CodSpeedConfig;
use crate::prelude::*;
use crate::run_environment::{self, RunEnvironment, RunEnvironmentProvider};
use crate::runner_mode::RunnerMode;
use crate::system::{self, SystemInfo};
use crate::upload::UploadResult;
use std::path::Path;

/// Shared orchestration state created once per CLI invocation.
///
/// Contains the run environment provider, system info, and logger — things
/// that are the same regardless of which executor mode is running.
pub struct Orchestrator {
    pub system_info: SystemInfo,
    pub provider: Box<dyn RunEnvironmentProvider>,
    pub logger: Logger,
}

impl Orchestrator {
    pub fn is_local(&self) -> bool {
        self.provider.get_run_environment() == RunEnvironment::Local
    }

    pub async fn new(
        config: &mut Config,
        codspeed_config: &CodSpeedConfig,
        api_client: &CodSpeedAPIClient,
    ) -> Result<Self> {
        let provider = run_environment::get_provider(config, api_client).await?;
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
            system_info,
            provider,
            logger,
        })
    }

    /// Execute benchmarks for all configured modes, then upload results.
    pub async fn execute<F>(
        &self,
        config: &mut Config,
        setup_cache_dir: Option<&Path>,
        poll_results: F,
    ) -> Result<()>
    where
        F: AsyncFn(&UploadResult) -> Result<()>,
    {
        // Phase 1: Run all executors
        let modes = config.modes.clone();
        let is_multi_mode = modes.len() > 1;
        let mut completed_runs: Vec<(ExecutionContext, ExecutorName)> = vec![];
        for mode in &modes {
            let mut per_mode_config = config.for_mode(mode);
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

        // Phase 2: Upload all results
        if !config.skip_upload {
            for (ctx, executor_name) in &mut completed_runs {
                if !self.is_local() {
                    // OIDC tokens can expire quickly, so refresh just before each upload
                    self.provider.set_oidc_token(&mut ctx.config).await?;
                }

                start_group!("Uploading results");
                let upload_result = crate::upload::upload(self, ctx, executor_name.clone()).await?;

                if self.is_local() {
                    poll_results(&upload_result).await?;
                }
                end_group!();
            }
        } else {
            debug!("Skipping upload of performance data");
        }

        Ok(())
    }
}
