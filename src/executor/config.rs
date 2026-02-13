use crate::cli::UnwindingMode;
use crate::cli::exec::wrap_with_exec_harness;
use crate::cli::run::RunArgs;
use crate::instruments::Instruments;
use crate::prelude::*;
use crate::run_environment::RepositoryProvider;
use crate::runner_mode::RunnerMode;
use clap::ValueEnum;
use semver::Version;
use std::path::PathBuf;
use url::Url;

/// The Valgrind tool to use for simulation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum SimulationTool {
    /// Use Callgrind for aggregated text-based cost profiles (.out files)
    #[default]
    Callgrind,
    /// Use Tracegrind for streaming binary event traces (.tgtrace files)
    Tracegrind,
}

/// Execution configuration for running benchmarks.
///
/// This struct contains all the configuration parameters needed to execute benchmarks,
/// including API settings, execution mode, instrumentation options, and various flags
/// for controlling the execution flow. It is typically constructed from command-line
/// arguments via [`RunArgs`] and combined with [`CodSpeedConfig`] to create an
/// [`ExecutionContext`].
#[derive(Debug)]
pub struct Config {
    pub upload_url: Url,
    pub token: Option<String>,
    pub repository_override: Option<RepositoryOverride>,
    pub working_directory: Option<String>,
    pub command: String,

    pub mode: RunnerMode,
    pub instruments: Instruments,
    pub enable_perf: bool,
    /// Stack unwinding mode for perf (if enabled)
    pub perf_unwinding_mode: Option<UnwindingMode>,

    pub simulation_tool: SimulationTool,

    pub profile_folder: Option<PathBuf>,
    pub skip_upload: bool,
    pub skip_polling: bool,
    pub skip_run: bool,
    pub skip_setup: bool,
    /// If true, allow execution even when no benchmarks are found
    pub allow_empty: bool,
    /// The version of go-runner to install (if None, installs latest)
    pub go_runner_version: Option<Version>,
}

#[derive(Debug, PartialEq, Clone)]
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

impl Config {
    pub fn set_token(&mut self, token: Option<String>) {
        self.token = token;
    }
}

#[cfg(test)]
impl Config {
    /// Constructs a new `Config` with default values for testing purposes
    pub fn test() -> Self {
        Self {
            upload_url: Url::parse(DEFAULT_UPLOAD_URL).unwrap(),
            token: None,
            repository_override: None,
            working_directory: None,
            command: "".into(),
            mode: RunnerMode::Simulation,
            instruments: Instruments::test(),
            perf_unwinding_mode: None,
            enable_perf: false,
            simulation_tool: SimulationTool::default(),
            profile_folder: None,
            skip_upload: false,
            skip_polling: false,
            skip_run: false,
            skip_setup: false,
            allow_empty: false,
            go_runner_version: None,
        }
    }
}

const DEFAULT_UPLOAD_URL: &str = "https://api.codspeed.io/upload";

impl TryFrom<RunArgs> for Config {
    type Error = Error;
    fn try_from(args: RunArgs) -> Result<Self> {
        let instruments = Instruments::try_from(&args)?;
        let mode = args.shared.resolve_mode()?;
        let raw_upload_url = args
            .shared
            .upload_url
            .unwrap_or_else(|| DEFAULT_UPLOAD_URL.into());
        let upload_url = Url::parse(&raw_upload_url)
            .map_err(|e| anyhow!("Invalid upload URL: {raw_upload_url}, {e}"))?;

        Ok(Self {
            upload_url,
            token: args.shared.token,
            repository_override: args
                .shared
                .repository
                .map(|repo| RepositoryOverride::from_arg(repo, args.shared.provider))
                .transpose()?,
            working_directory: args.shared.working_directory,
            mode,
            instruments,
            perf_unwinding_mode: args.shared.perf_run_args.perf_unwinding_mode,
            enable_perf: args.shared.perf_run_args.enable_perf,
            command: args.command.join(" "),
            simulation_tool: args.shared.simulation_tool.unwrap_or_default(),
            profile_folder: args.shared.profile_folder,
            skip_upload: args.shared.skip_upload,
            skip_polling: args.shared.skip_polling,
            skip_run: args.shared.skip_run,
            skip_setup: args.shared.skip_setup,
            allow_empty: args.shared.allow_empty,
            go_runner_version: args.shared.go_runner_version,
        })
    }
}

impl Config {
    /// Create a Config from ExecArgs with a custom command (used for targets mode)
    pub fn try_from_with_command(
        args: crate::cli::exec::ExecArgs,
        command: String,
    ) -> Result<Self> {
        let mode = args.shared.resolve_mode()?;
        let raw_upload_url = args
            .shared
            .upload_url
            .unwrap_or_else(|| DEFAULT_UPLOAD_URL.into());
        let upload_url = Url::parse(&raw_upload_url)
            .map_err(|e| anyhow!("Invalid upload URL: {raw_upload_url}, {e}"))?;

        Ok(Self {
            upload_url,
            token: args.shared.token,
            repository_override: args
                .shared
                .repository
                .map(|repo| RepositoryOverride::from_arg(repo, args.shared.provider))
                .transpose()?,
            working_directory: args.shared.working_directory,
            mode,
            instruments: Instruments { mongodb: None }, // exec doesn't support MongoDB
            perf_unwinding_mode: args.shared.perf_run_args.perf_unwinding_mode,
            enable_perf: args.shared.perf_run_args.enable_perf,
            command,
            simulation_tool: args.shared.simulation_tool.unwrap_or_default(),
            profile_folder: args.shared.profile_folder,
            skip_upload: args.shared.skip_upload,
            skip_polling: args.shared.skip_polling,
            skip_run: args.shared.skip_run,
            skip_setup: args.shared.skip_setup,
            allow_empty: args.shared.allow_empty,
            go_runner_version: args.shared.go_runner_version,
        })
    }
}

impl TryFrom<crate::cli::exec::ExecArgs> for Config {
    type Error = Error;
    fn try_from(args: crate::cli::exec::ExecArgs) -> Result<Self> {
        let wrapped_command = wrap_with_exec_harness(&args.walltime_args, &args.command);
        Self::try_from_with_command(args, wrapped_command)
    }
}

#[cfg(test)]
mod tests {
    use crate::cli::PerfRunArgs;
    use crate::instruments::MongoDBConfig;

    use super::*;

    #[test]
    fn test_try_from_env_empty() {
        let config = Config::try_from(RunArgs {
            shared: crate::cli::ExecAndRunSharedArgs {
                upload_url: None,
                token: None,
                repository: None,
                provider: None,
                working_directory: None,
                mode: Some(RunnerMode::Simulation),
                simulation_tool: None,
                profile_folder: None,
                skip_upload: false,
                skip_polling: false,
                skip_run: false,
                skip_setup: false,
                allow_empty: false,
                go_runner_version: None,
                perf_run_args: PerfRunArgs {
                    enable_perf: false,
                    perf_unwinding_mode: None,
                },
            },
            instruments: vec![],
            mongo_uri_env_name: None,
            message_format: None,
            command: vec!["cargo".into(), "codspeed".into(), "bench".into()],
        })
        .unwrap();
        assert_eq!(config.upload_url, Url::parse(DEFAULT_UPLOAD_URL).unwrap());
        assert_eq!(config.token, None);
        assert_eq!(config.repository_override, None);
        assert_eq!(config.working_directory, None);
        assert_eq!(config.instruments, Instruments { mongodb: None });
        assert!(!config.skip_upload);
        assert!(!config.skip_run);
        assert!(!config.skip_setup);
        assert!(!config.allow_empty);
        assert_eq!(config.command, "cargo codspeed bench");
    }

    #[test]
    fn test_try_from_args() {
        let config = Config::try_from(RunArgs {
            shared: crate::cli::ExecAndRunSharedArgs {
                upload_url: Some("https://example.com/upload".into()),
                token: Some("token".into()),
                repository: Some("owner/repo".into()),
                provider: Some(RepositoryProvider::GitLab),
                working_directory: Some("/tmp".into()),
                mode: Some(RunnerMode::Simulation),
                simulation_tool: None,
                profile_folder: Some("./codspeed.out".into()),
                skip_upload: true,
                skip_polling: true,
                skip_run: true,
                skip_setup: true,
                allow_empty: true,
                go_runner_version: None,
                perf_run_args: PerfRunArgs {
                    enable_perf: false,
                    perf_unwinding_mode: Some(UnwindingMode::FramePointer),
                },
            },
            instruments: vec!["mongodb".into()],
            mongo_uri_env_name: Some("MONGODB_URI".into()),
            message_format: Some(crate::cli::run::MessageFormat::Json),
            command: vec!["cargo".into(), "codspeed".into(), "bench".into()],
        })
        .unwrap();

        assert_eq!(
            config.upload_url,
            Url::parse("https://example.com/upload").unwrap()
        );
        assert_eq!(config.token, Some("token".into()));
        assert_eq!(
            config.repository_override,
            Some(RepositoryOverride {
                owner: "owner".into(),
                repository: "repo".into(),
                repository_provider: RepositoryProvider::GitLab,
            })
        );
        assert_eq!(config.working_directory, Some("/tmp".into()));
        assert_eq!(
            config.instruments,
            Instruments {
                mongodb: Some(MongoDBConfig {
                    uri_env_name: Some("MONGODB_URI".into())
                })
            }
        );
        assert_eq!(config.profile_folder, Some("./codspeed.out".into()));
        assert!(config.skip_upload);
        assert!(config.skip_run);
        assert!(config.skip_setup);
        assert!(config.allow_empty);
        assert_eq!(config.command, "cargo codspeed bench");
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

    #[test]
    fn test_try_from_exec_args_default_url() {
        let exec_args = crate::cli::exec::ExecArgs {
            shared: crate::cli::ExecAndRunSharedArgs {
                upload_url: None,
                token: None,
                repository: None,
                provider: None,
                working_directory: None,
                mode: Some(RunnerMode::Simulation),
                simulation_tool: None,
                profile_folder: None,
                skip_upload: false,
                skip_polling: false,
                skip_run: false,
                skip_setup: false,
                allow_empty: false,
                go_runner_version: None,
                perf_run_args: PerfRunArgs {
                    enable_perf: false,
                    perf_unwinding_mode: None,
                },
            },
            walltime_args: Default::default(),
            name: None,
            command: vec!["my-binary".into(), "arg1".into(), "arg2".into()],
        };

        let config = Config::try_from(exec_args).unwrap();

        assert_eq!(
            config.upload_url,
            Url::parse("https://api.codspeed.io/upload").unwrap()
        );
        assert_eq!(config.command, "exec-harness my-binary arg1 arg2");
    }
}
