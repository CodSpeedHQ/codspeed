use crate::VERSION;
use crate::executor::config::SimulationTool;
use crate::prelude::*;
use crate::run_environment::interfaces::RepositoryProvider;
use crate::runner_mode::{RunnerMode, load_shell_session_mode};
use clap::Args;
use clap::ValueEnum;
use std::path::PathBuf;

pub(crate) fn show_banner() {
    let banner = format!(
        r#"
   ______            __ _____                         __
  / ____/____   ____/ // ___/ ____   ___   ___   ____/ /
 / /    / __ \ / __  / \__ \ / __ \ / _ \ / _ \ / __  /
/ /___ / /_/ // /_/ / ___/ // /_/ //  __//  __// /_/ /
\____/ \____/ \__,_/ /____// .___/ \___/ \___/ \__,_/
  https://codspeed.io     /_/          runner v{VERSION}
"#
    );
    println!("{banner}");
    debug!("codspeed v{VERSION}");
}

/// Arguments shared between run and exec commands
#[derive(Args, Debug, Clone)]
pub struct ExecAndRunSharedArgs {
    /// The upload URL to use for uploading the results, useful for on-premises installations
    #[arg(long, env = "CODSPEED_UPLOAD_URL")]
    pub upload_url: Option<String>,

    /// The token to use for uploading the results,
    ///
    /// It can be either a CodSpeed token retrieved from the repository setting
    /// or an OIDC token issued by the identity provider.
    #[arg(long, env = "CODSPEED_TOKEN")]
    pub token: Option<String>,

    /// The repository the benchmark is associated with, under the format `owner/repo`.
    #[arg(short, long, env = "CODSPEED_REPOSITORY")]
    pub repository: Option<String>,

    /// The repository provider to use in case --repository is used. Defaults to github
    #[arg(
        long,
        env = "CODSPEED_PROVIDER",
        requires = "repository",
        ignore_case = true
    )]
    pub provider: Option<RepositoryProvider>,

    /// The directory where the command will be executed.
    #[arg(long)]
    pub working_directory: Option<String>,

    /// The mode to run the benchmarks in.
    /// If not provided, the mode will be loaded from the shell session (set via `codspeed use <mode>`).
    #[arg(short, long, value_enum, env = "CODSPEED_RUNNER_MODE")]
    pub mode: Option<RunnerMode>,

    /// The Valgrind simulation tool to use (callgrind or tracegrind).
    #[arg(long, value_enum, env = "CODSPEED_SIMULATION_TOOL", hide = true)]
    pub simulation_tool: Option<SimulationTool>,

    /// Profile folder to use for the run.
    #[arg(long)]
    pub profile_folder: Option<PathBuf>,

    /// Only for debugging purposes, skips the upload of the results
    #[arg(
        long,
        default_value = "false",
        hide = true,
        env = "CODSPEED_SKIP_UPLOAD"
    )]
    pub skip_upload: bool,

    /// Skips polling for the results after upload.
    /// This is useful for local runs where the user doesn't want to wait for the results to be available on the server.
    #[arg(
        long,
        default_value = "false",
        hide = true,
        env = "CODSPEED_SKIP_POLLING"
    )]
    pub skip_polling: bool,

    /// Used internally to upload the results after running the benchmarks in a sandbox environment
    /// with no internet access
    #[arg(long, default_value = "false", hide = true)]
    pub skip_run: bool,

    /// Only for debugging purposes, skips the setup of the runner
    #[arg(long, default_value = "false", hide = true)]
    pub skip_setup: bool,

    /// Allow runs without any benchmarks to succeed instead of failing
    #[arg(long, default_value = "false", hide = true)]
    pub allow_empty: bool,

    /// The version of the go-runner to use (e.g., 1.2.3, 1.0.0-beta.1)
    /// If not specified, the latest version will be installed
    #[arg(long, env = "CODSPEED_GO_RUNNER_VERSION", value_parser = parse_version)]
    pub go_runner_version: Option<semver::Version>,

    #[command(flatten)]
    pub perf_run_args: PerfRunArgs,
}

impl ExecAndRunSharedArgs {
    /// Resolves the runner mode from CLI argument, shell session, or returns an error.
    ///
    /// Priority:
    /// 1. CLI argument (--mode or -m)
    /// 2. Shell session mode (set via `codspeed use <mode>`)
    /// 3. Error if neither is available
    pub fn resolve_mode(&self) -> Result<RunnerMode> {
        if let Some(mode) = &self.mode {
            return Ok(mode.clone());
        }

        if let Some(mode) = load_shell_session_mode()? {
            debug!("Loaded mode from shell session: {mode:?}");
            return Ok(mode);
        }

        Err(anyhow!(
            "No runner mode specified. Use --mode <mode> or set the mode for this shell session with `codspeed use <mode>`."
        ))
    }
}

#[derive(Debug, Copy, Clone, PartialEq, ValueEnum, Default)]
pub enum UnwindingMode {
    /// Use the frame pointer for unwinding. Requires the binary to be compiled with frame pointers enabled.
    #[clap(name = "fp")]
    FramePointer,

    /// Use DWARF unwinding. This does not require any special compilation flags and is enabled by default.
    #[default]
    Dwarf,
}

#[derive(Args, Debug, Clone)]
pub struct PerfRunArgs {
    /// Enable the linux perf profiler to collect granular performance data.
    /// This is only supported on Linux.
    #[arg(long, env = "CODSPEED_PERF_ENABLED", default_value_t = true)]
    pub enable_perf: bool,

    /// The unwinding mode that should be used with perf to collect the call stack.
    #[arg(long, env = "CODSPEED_PERF_UNWINDING_MODE")]
    pub perf_unwinding_mode: Option<UnwindingMode>,
}

/// Parser for go-runner version that validates semver format
fn parse_version(s: &str) -> Result<semver::Version, String> {
    semver::Version::parse(s).map_err(|e| format!("Invalid semantic version: {e}"))
}
