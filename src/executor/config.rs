use crate::cli::UnwindingMode;
use crate::instruments::Instruments;
use crate::prelude::*;
use crate::run_environment::RepositoryProvider;
use crate::runner_mode::RunnerMode;
use clap::ValueEnum;
use semver::Version;
use std::path::PathBuf;
use url::Url;

/// A benchmark target from project configuration.
///
/// Defines how a benchmark is executed:
/// - `Exec`: a plain command measured by exec-harness (all exec targets share one invocation)
/// - `Entrypoint`: a command that already contains benchmark harnessing (run independently)
#[derive(Debug, Clone)]
pub enum BenchmarkTarget {
    /// A command measured by exec-harness (e.g. `ls -al /nix/store`)
    Exec {
        command: Vec<String>,
        name: Option<String>,
        walltime_args: exec_harness::walltime::WalltimeExecutionArgs,
    },
    /// A command with built-in harness (e.g. `pytest --codspeed src`)
    Entrypoint {
        command: String,
        // We do not use it yet, temporarily allow
        #[allow(dead_code)]
        name: Option<String>,
    },
}

/// The Valgrind tool to use for simulation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum SimulationTool {
    /// Use Callgrind for aggregated text-based cost profiles (.out files)
    #[default]
    Callgrind,
    /// Use Tracegrind for streaming binary event traces (.tgtrace files)
    Tracegrind,
}

/// Run-level configuration owned by the orchestrator.
///
/// Holds all parameters that are constant across benchmark targets within a run,
/// plus the list of targets to execute.
/// Constructed from CLI arguments and passed to [`Orchestrator::new`].
/// Use [`OrchestratorConfig::executor_config_for_command`] to produce a per-execution [`ExecutorConfig`].
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    pub upload_url: Url,
    pub token: Option<String>,
    pub repository_override: Option<RepositoryOverride>,
    pub working_directory: Option<String>,

    pub targets: Vec<BenchmarkTarget>,

    pub modes: Vec<RunnerMode>,
    pub instruments: Instruments,
    pub enable_perf: bool,
    /// Stack unwinding mode for perf (if enabled)
    pub perf_unwinding_mode: Option<UnwindingMode>,

    pub simulation_tool: SimulationTool,

    pub profile_folder: Option<PathBuf>,
    pub skip_upload: bool,
    pub skip_run: bool,
    pub skip_setup: bool,
    /// If true, allow execution even when no benchmarks are found
    pub allow_empty: bool,
    /// The version of go-runner to install (if None, installs latest)
    pub go_runner_version: Option<Version>,
}

/// Per-execution configuration passed to executors.
///
/// Produced by [`OrchestratorConfig::executor_config_for_command`]; holds the `command` string
/// that the executor will run, plus executor-specific fields.
/// Fields that are only needed at the orchestrator level (e.g. `upload_url`,
/// `skip_upload`, `repository_override`) live on [`OrchestratorConfig`].
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    pub token: Option<String>,
    pub working_directory: Option<String>,
    pub command: String,

    pub instruments: Instruments,
    pub enable_perf: bool,
    /// Stack unwinding mode for perf (if enabled)
    pub perf_unwinding_mode: Option<UnwindingMode>,

    pub simulation_tool: SimulationTool,

    pub skip_run: bool,
    pub skip_setup: bool,
    /// If true, allow execution even when no benchmarks are found
    pub allow_empty: bool,
    /// The version of go-runner to install (if None, installs latest)
    pub go_runner_version: Option<Version>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RepositoryOverride {
    pub owner: String,
    pub repository: String,
    pub repository_provider: RepositoryProvider,
}

impl RepositoryOverride {
    /// Creates a RepositoryOverride from an "owner/repository" string
    pub fn from_arg(
        repository_and_owner: String,
        provider: Option<RepositoryProvider>,
    ) -> Result<Self> {
        let (owner, repository) = repository_and_owner
            .split_once('/')
            .context("Invalid owner/repository format")?;
        Ok(Self {
            owner: owner.to_string(),
            repository: repository.to_string(),
            repository_provider: provider.unwrap_or_default(),
        })
    }
}

pub const DEFAULT_UPLOAD_URL: &str = "https://api.codspeed.io/upload";

impl OrchestratorConfig {
    pub fn set_token(&mut self, token: Option<String>) {
        self.token = token;
    }

    /// Compute the total number of executor runs that will be performed.
    ///
    /// All `Exec` targets are combined into a single invocation, while each
    /// `Entrypoint` target runs independently. Both are multiplied by the
    /// number of configured modes.
    pub fn expected_run_parts_count(&self) -> u32 {
        let has_exec = self
            .targets
            .iter()
            .any(|t| matches!(t, BenchmarkTarget::Exec { .. }));
        let entrypoint_count = self
            .targets
            .iter()
            .filter(|t| matches!(t, BenchmarkTarget::Entrypoint { .. }))
            .count();
        let invocation_count = (if has_exec { 1 } else { 0 }) + entrypoint_count;
        (invocation_count * self.modes.len()) as u32
    }

    /// Produce a per-execution [`ExecutorConfig`] for the given command and mode.
    pub fn executor_config_for_command(&self, command: String) -> ExecutorConfig {
        ExecutorConfig {
            token: self.token.clone(),
            working_directory: self.working_directory.clone(),
            command,
            instruments: self.instruments.clone(),
            enable_perf: self.enable_perf,
            perf_unwinding_mode: self.perf_unwinding_mode,
            simulation_tool: self.simulation_tool,
            skip_run: self.skip_run,
            skip_setup: self.skip_setup,
            allow_empty: self.allow_empty,
            go_runner_version: self.go_runner_version.clone(),
        }
    }
}

impl ExecutorConfig {
    pub fn set_token(&mut self, token: Option<String>) {
        self.token = token;
    }
}

#[cfg(test)]
impl OrchestratorConfig {
    /// Constructs a new `OrchestratorConfig` with default values for testing purposes
    pub fn test() -> Self {
        Self {
            upload_url: Url::parse(DEFAULT_UPLOAD_URL).unwrap(),
            token: None,
            repository_override: None,
            working_directory: None,
            targets: vec![BenchmarkTarget::Entrypoint {
                command: String::new(),
                name: None,
            }],
            modes: vec![RunnerMode::Simulation],
            instruments: Instruments::test(),
            perf_unwinding_mode: None,
            enable_perf: false,
            simulation_tool: SimulationTool::default(),
            profile_folder: None,
            skip_upload: false,
            skip_run: false,
            skip_setup: false,
            allow_empty: false,
            go_runner_version: None,
        }
    }
}

#[cfg(test)]
impl ExecutorConfig {
    /// Constructs a new `ExecutorConfig` with default values for testing purposes
    pub fn test() -> Self {
        OrchestratorConfig::test().executor_config_for_command("".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expected_run_parts_count() {
        use crate::runner_mode::RunnerMode;

        let base = OrchestratorConfig::test();

        // Single entrypoint, single mode → 1
        let config = OrchestratorConfig {
            targets: vec![BenchmarkTarget::Entrypoint {
                command: "cmd".into(),
                name: None,
            }],
            modes: vec![RunnerMode::Simulation],
            ..base.clone()
        };
        assert_eq!(config.expected_run_parts_count(), 1);

        // Two entrypoints, single mode → 2
        let config = OrchestratorConfig {
            targets: vec![
                BenchmarkTarget::Entrypoint {
                    command: "cmd1".into(),
                    name: None,
                },
                BenchmarkTarget::Entrypoint {
                    command: "cmd2".into(),
                    name: None,
                },
            ],
            modes: vec![RunnerMode::Simulation],
            ..base.clone()
        };
        assert_eq!(config.expected_run_parts_count(), 2);

        // Multiple exec targets count as one invocation, single mode → 1
        let config = OrchestratorConfig {
            targets: vec![
                BenchmarkTarget::Exec {
                    command: vec!["exec1".into()],
                    name: None,
                    walltime_args: Default::default(),
                },
                BenchmarkTarget::Exec {
                    command: vec!["exec2".into()],
                    name: None,
                    walltime_args: Default::default(),
                },
            ],
            modes: vec![RunnerMode::Simulation],
            ..base.clone()
        };
        assert_eq!(config.expected_run_parts_count(), 1);

        // Mix of exec and entrypoint, single mode → 2
        let config = OrchestratorConfig {
            targets: vec![
                BenchmarkTarget::Exec {
                    command: vec!["exec1".into()],
                    name: None,
                    walltime_args: Default::default(),
                },
                BenchmarkTarget::Entrypoint {
                    command: "cmd".into(),
                    name: None,
                },
            ],
            modes: vec![RunnerMode::Simulation],
            ..base.clone()
        };
        assert_eq!(config.expected_run_parts_count(), 2);

        // Single entrypoint, two modes → 2
        #[allow(deprecated)]
        let config = OrchestratorConfig {
            targets: vec![BenchmarkTarget::Entrypoint {
                command: "cmd".into(),
                name: None,
            }],
            modes: vec![RunnerMode::Simulation, RunnerMode::Walltime],
            ..base.clone()
        };
        assert_eq!(config.expected_run_parts_count(), 2);

        // Mix of exec and entrypoint, two modes → 4
        #[allow(deprecated)]
        let config = OrchestratorConfig {
            targets: vec![
                BenchmarkTarget::Exec {
                    command: vec!["exec1".into()],
                    name: None,
                    walltime_args: Default::default(),
                },
                BenchmarkTarget::Entrypoint {
                    command: "cmd".into(),
                    name: None,
                },
            ],
            modes: vec![RunnerMode::Simulation, RunnerMode::Walltime],
            ..base.clone()
        };
        assert_eq!(config.expected_run_parts_count(), 4);
    }

    #[test]
    fn test_repository_override_from_arg() {
        let override_result =
            RepositoryOverride::from_arg("CodSpeedHQ/codspeed".to_string(), None).unwrap();
        assert_eq!(override_result.owner, "CodSpeedHQ");
        assert_eq!(override_result.repository, "codspeed");
        assert_eq!(
            override_result.repository_provider,
            RepositoryProvider::GitHub
        );

        let override_with_provider = RepositoryOverride::from_arg(
            "CodSpeedHQ/codspeed".to_string(),
            Some(RepositoryProvider::GitLab),
        )
        .unwrap();
        assert_eq!(
            override_with_provider.repository_provider,
            RepositoryProvider::GitLab
        );

        let result = RepositoryOverride::from_arg("CodSpeedHQ_runner".to_string(), None);
        assert!(result.is_err());
    }
}
