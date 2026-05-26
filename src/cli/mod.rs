mod auth;
pub(crate) mod exec;
pub(crate) mod experimental;
mod profile;
pub(crate) mod run;
pub(crate) mod samply;
mod setup;
mod shared;
mod show;
mod status;
mod update;
mod use_mode;

pub(crate) use shared::*;

use std::path::PathBuf;

use crate::{
    api_client::CodSpeedAPIClient,
    config::{CodSpeedConfig, ConfigOverrides},
    executor::helpers::command::CommandBuilder,
    local_logger::{CODSPEED_U8_COLOR_CODE, init_local_logger},
    prelude::*,
    project_config::DiscoveredProjectConfig,
};
use clap::{
    Parser, Subcommand,
    builder::{Styles, styling},
};

fn create_styles() -> Styles {
    styling::Styles::styled()
        .header(styling::AnsiColor::Green.on_default() | styling::Effects::BOLD)
        .usage(styling::AnsiColor::Green.on_default() | styling::Effects::BOLD)
        .literal(
            styling::Ansi256Color(CODSPEED_U8_COLOR_CODE).on_default() | styling::Effects::BOLD,
        )
        .placeholder(styling::AnsiColor::Cyan.on_default())
}

#[derive(Parser, Debug)]
#[command(version, about = "The CodSpeed CLI tool", styles = create_styles())]
pub struct Cli {
    /// The URL of the CodSpeed GraphQL API
    #[arg(long, env = "CODSPEED_API_URL", global = true, hide = true)]
    pub api_url: Option<String>,

    /// The OAuth token to use for all requests
    #[arg(long, env = "CODSPEED_OAUTH_TOKEN", global = true, hide = true)]
    pub oauth_token: Option<String>,

    /// [deprecated] Load configuration from `~/.config/codspeed/{config-name}.yaml`
    /// instead of the default `config.yaml`. Prefer `--profile` instead.
    #[arg(long, env = "CODSPEED_CONFIG_NAME", global = true, hide = true)]
    #[deprecated(note = "use `--profile` / `CODSPEED_PROFILE` instead")]
    pub config_name: Option<String>,

    /// The CodSpeed profile to use
    #[arg(long, env = "CODSPEED_PROFILE", global = true)]
    pub profile: Option<String>,

    /// Path to project configuration file (codspeed.yaml)
    /// If provided, loads config from this path. Otherwise, searches for config files
    /// in the current directory and upward to the git root.
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// The directory to use for caching installed tools
    /// The runner will restore cached tools from this directory before installing them.
    /// After successful installation, the runner will cache the installed tools to this directory.
    /// Only supported on ubuntu and debian systems.
    #[arg(long, env = "CODSPEED_SETUP_CACHE_DIR", global = true)]
    pub setup_cache_dir: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run a benchmark program that already contains the CodSpeed instrumentation and upload the results to CodSpeed
    #[command(alias = "r")]
    Run(Box<run::RunArgs>),
    /// Run a command after adding CodSpeed instrumentation to it and upload the results to
    /// CodSpeed
    #[command(alias = "x")]
    Exec(Box<exec::ExecArgs>),
    /// Manage the CLI authentication state
    Auth(auth::AuthArgs),
    /// Manage CodSpeed profiles
    Profile(profile::ProfileArgs),
    /// Pre-install the codspeed executors
    Setup(setup::SetupArgs),
    /// Show the overall status of CodSpeed (authentication, tools, system)
    Status,
    /// Set the codspeed mode for the rest of the shell session
    Use(use_mode::UseArgs),
    /// Show the codspeed mode previously set in this shell session with `codspeed use`
    Show,
    /// Update the CodSpeed CLI to the latest version
    Update,

    #[command(flatten)]
    Internal(InternalCommands),
}

/// Subcommands the CLI uses to re-invoke itself; not user-facing entry points.
#[derive(Subcommand, Debug)]
pub(crate) enum InternalCommands {
    /// Run the bundled samply profiler. Args are forwarded to samply.
    #[command(disable_help_flag = true, disable_help_subcommand = true)]
    Samply(samply::SamplyArgs),
}

impl InternalCommands {
    /// Build a [`CommandBuilder`] that re-execs the current binary into this
    /// internal subcommand. Each variant owns its own arg layout.
    pub fn get_command_builder(&self) -> Result<CommandBuilder> {
        let current_exe = std::env::current_exe()
            .context("failed to resolve current executable for internal subcommand")?;
        let mut builder = CommandBuilder::new(current_exe);
        match self {
            InternalCommands::Samply(args) => {
                builder.arg("samply");
                builder.args(args.args.iter().cloned());
            }
        }
        Ok(builder)
    }
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    let codspeed_config = load_config(&cli)?;
    let mut api_client = build_api_client(&cli, &codspeed_config);

    // Discover project configuration file
    let discovered_config = DiscoveredProjectConfig::discover_and_load(
        cli.config.as_deref(),
        &std::env::current_dir()?,
    )?;

    // In the context of the CI, it is likely that a ~ made its way here without being expanded by the shell
    let setup_cache_dir = cli
        .setup_cache_dir
        .as_ref()
        .map(|d| PathBuf::from(shellexpand::tilde(d).as_ref()));
    let setup_cache_dir = setup_cache_dir.as_deref();

    match cli.command {
        Commands::Run(_) | Commands::Exec(_) | Commands::Internal(InternalCommands::Samply(_)) => {} // these are responsible for their own logger initialization
        _ => {
            init_local_logger()?;
        }
    }

    match cli.command {
        Commands::Run(args) => {
            let mut args = *args;
            args.shared
                .upload_url
                .get_or_insert_with(|| codspeed_config.upload_url.clone());
            args.shared.experimental.warn_if_active();
            run::run(
                args,
                &mut api_client,
                discovered_config.as_ref(),
                setup_cache_dir,
            )
            .await?
        }
        Commands::Exec(args) => {
            let mut args = *args;
            args.shared
                .upload_url
                .get_or_insert_with(|| codspeed_config.upload_url.clone());
            args.shared.experimental.warn_if_active();
            exec::run(
                args,
                &mut api_client,
                discovered_config.as_ref().map(|d| &d.config),
                setup_cache_dir,
            )
            .await?
        }
        Commands::Auth(args) => {
            #[allow(deprecated)]
            let config_name = cli.config_name.as_deref();
            auth::run(args, &api_client, config_name, codspeed_config).await?
        }
        Commands::Profile(args) => {
            #[allow(deprecated)]
            let config_name = cli.config_name.as_deref();
            profile::run(args, config_name, cli.profile.as_deref())?
        }
        Commands::Setup(args) => setup::run(args, setup_cache_dir).await?,
        Commands::Status => status::run(&api_client, &codspeed_config).await?,
        Commands::Use(args) => use_mode::run(args)?,
        Commands::Show => show::run()?,
        Commands::Update => update::run().await?,
        Commands::Internal(InternalCommands::Samply(args)) => samply::run(args)?,
    }
    Ok(())
}

/// Load the CodSpeed config for this invocation, resolving the active
/// profile (CLI `--profile` / `CODSPEED_PROFILE` / shell-session / built-in
/// `default`) and applying CLI overrides for the OAuth token and api URL.
///
/// `auth` and `profile` subcommands are allowed to run against a config
/// where the selected profile does not yet exist (e.g. first-time setup).
fn load_config(cli: &Cli) -> Result<CodSpeedConfig> {
    // The field carries a `#[deprecated]` marker but we still need to
    // honour it during the deprecation window.
    #[allow(deprecated)]
    let config_name = cli.config_name.as_deref();
    if config_name.is_some() {
        warn!(
            "`--config-name` / `CODSPEED_CONFIG_NAME` is deprecated; use `--profile` / `CODSPEED_PROFILE` instead."
        );
    }
    CodSpeedConfig::load_with_profile(
        config_name,
        cli.profile.as_deref(),
        ConfigOverrides {
            oauth_token: cli.oauth_token.as_deref(),
            api_url: cli.api_url.as_deref(),
            upload_url: None,
        },
        matches!(&cli.command, Commands::Auth(_) | Commands::Profile(_)),
    )
}

/// Build the api client for this invocation, resolving the auth token
/// from the most specific source available. This is the single source
/// of truth for token resolution; the result lives on the returned
/// client and every downstream consumer (GraphQL queries, upload
/// `Authorization` header, executor env injection) reads it from there.
///
/// Priority (most specific first):
///   1. `--token` / `CODSPEED_TOKEN`           — run/exec-level override
///   2. `--oauth-token` / `CODSPEED_OAUTH_TOKEN` and the persisted CLI
///      token from the selected profile.
fn build_api_client(cli: &Cli, config: &CodSpeedConfig) -> CodSpeedAPIClient {
    let explicit = match &cli.command {
        Commands::Run(args) => args.shared.token.clone(),
        Commands::Exec(args) => args.shared.token.clone(),
        _ => None,
    };
    let token = match explicit {
        Some(token) => Some(token),
        None => config.auth.token.clone(),
    };
    CodSpeedAPIClient::new(token, config.api_url.clone())
}
