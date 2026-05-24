mod auth;
pub(crate) mod exec;
pub(crate) mod experimental;
pub(crate) mod run;
mod setup;
mod shared;
mod show;
mod status;
pub(crate) mod tool;
mod update;
mod use_mode;

pub(crate) use shared::*;

use std::path::PathBuf;

use crate::{
    api_client::CodSpeedAPIClient,
    config::CodSpeedConfig,
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
    #[arg(
        long,
        env = "CODSPEED_API_URL",
        global = true,
        hide = true,
        default_value = "https://gql.codspeed.io/"
    )]
    pub api_url: String,

    /// The OAuth token to use for all requests
    #[arg(long, env = "CODSPEED_OAUTH_TOKEN", global = true, hide = true)]
    pub oauth_token: Option<String>,

    /// The configuration name to use
    /// If provided, the configuration will be loaded from ~/.config/codspeed/{config-name}.yaml
    /// Otherwise, loads from ~/.config/codspeed/config.yaml
    #[arg(long, env = "CODSPEED_CONFIG_NAME", global = true)]
    pub config_name: Option<String>,

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

    /// Hidden tool group used by the runner to re-exec itself into bundled
    /// helpers (samply, exec-harness, memtrack). Help generation is disabled
    /// so the wrapped binaries can handle `--help` themselves (and so the
    /// root CLI's global flags don't leak into their help output).
    #[command(hide = true, disable_help_flag = true, disable_help_subcommand = true)]
    Tool(tool::ToolArgs),
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();

    // `tool` subcommands are re-exec targets for bundled helper binaries — they
    // don't talk to the API and don't read project/CLI config, so short-circuit
    // before any of that init runs (a broken config file shouldn't block them).
    if let Commands::Tool(args) = cli.command {
        return tool::run(args);
    }

    let mut api_client = build_api_client(&cli)?;

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
        // These are responsible for their own logger initialization
        Commands::Run(_) | Commands::Exec(_) => {}
        _ => {
            init_local_logger()?;
        }
    }

    match cli.command {
        Commands::Run(args) => {
            args.shared.experimental.warn_if_active();
            run::run(
                *args,
                &mut api_client,
                discovered_config.as_ref(),
                setup_cache_dir,
            )
            .await?
        }
        Commands::Exec(args) => {
            args.shared.experimental.warn_if_active();
            exec::run(
                *args,
                &mut api_client,
                discovered_config.as_ref().map(|d| &d.config),
                setup_cache_dir,
            )
            .await?
        }
        Commands::Auth(args) => auth::run(args, &api_client, cli.config_name.as_deref()).await?,
        Commands::Setup(args) => setup::run(args, setup_cache_dir).await?,
        Commands::Status => status::run(&api_client).await?,
        Commands::Use(args) => use_mode::run(args)?,
        Commands::Show => show::run()?,
        Commands::Update => update::run().await?,
        Commands::Tool(_) => unreachable!("handled at the top of `run` before init"),
    }
    Ok(())
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
///      token — both live on disk and are loaded together by
///      [`CodSpeedConfig::load_with_override`].
///
/// The CLI config file is only read when no explicit token was passed,
/// so an invocation like `codspeed run --token <X>` never touches the
/// user's `~/.config/codspeed/`.
fn build_api_client(cli: &Cli) -> Result<CodSpeedAPIClient> {
    let explicit = match &cli.command {
        Commands::Run(args) => args.shared.token.clone(),
        Commands::Exec(args) => args.shared.token.clone(),
        _ => None,
    };
    let token = match explicit {
        Some(token) => Some(token),
        None => {
            CodSpeedConfig::load_with_override(
                cli.config_name.as_deref(),
                cli.oauth_token.as_deref(),
            )?
            .auth
            .token
        }
    };
    Ok(CodSpeedAPIClient::new(token, cli.api_url.clone()))
}
