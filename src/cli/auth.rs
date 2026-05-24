use std::io::Read;
use std::time::Duration;

use crate::api_client::{
    CodSpeedAPIClient, RepositoryOverviewPayload, SessionAndRepositoryOverviewError,
    SessionAndRepositoryOverviewVars, SessionError, SessionPayload,
};
use crate::cli::run::helpers::{
    ParsedRepository, find_repository_root, parse_repository_from_remote,
};
use crate::config::CodSpeedConfig;
use crate::prelude::*;
use clap::{Args, Subcommand};
use console::style;
use git2::Repository;
use tokio::time::{Instant, sleep};

use super::status::{check_mark, cross_mark};

#[derive(Debug, Args)]
pub struct AuthArgs {
    #[command(subcommand)]
    command: AuthCommands,
}

#[derive(Debug, Subcommand)]
enum AuthCommands {
    /// Login to CodSpeed
    Login {
        /// Read the token from standard input instead of running the OAuth flow
        #[arg(long)]
        with_token: bool,
    },
    /// Show the authentication status
    Status,
}

pub async fn run(
    args: AuthArgs,
    api_client: &CodSpeedAPIClient,
    config_name: Option<&str>,
) -> Result<()> {
    match args.command {
        AuthCommands::Login { with_token } => login(api_client, config_name, with_token).await?,
        AuthCommands::Status => status(api_client).await?,
    }
    Ok(())
}

const LOGIN_SESSION_MAX_DURATION: Duration = Duration::from_secs(60 * 5); // 5 minutes

async fn login(
    api_client: &CodSpeedAPIClient,
    config_name: Option<&str>,
    with_token: bool,
) -> Result<()> {
    debug!("Login to CodSpeed");

    let token = if with_token {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        let token = buf.trim().to_owned();
        if token.is_empty() {
            bail!("No token provided on stdin");
        }
        token
    } else {
        start_group!("Creating login session");
        let login_session_payload = api_client.create_login_session().await?;
        end_group!();

        if open::that(&login_session_payload.callback_url).is_ok() {
            info!("Your browser has been opened to complete the login process");
        } else {
            warn!("Failed to open the browser automatically, please open the URL manually");
        }
        info!(
            "Authentication URL: {}\n",
            style(login_session_payload.callback_url)
                .blue()
                .bold()
                .underlined()
        );

        start_group!("Waiting for the login to be completed");
        let token;
        let start = Instant::now();
        loop {
            if start.elapsed() > LOGIN_SESSION_MAX_DURATION {
                bail!("Login session expired, please try again");
            }

            match api_client
                .consume_login_session(&login_session_payload.session_id)
                .await?
                .token
            {
                Some(token_from_api) => {
                    token = token_from_api;
                    break;
                }
                None => sleep(Duration::from_secs(5)).await,
            }
        }
        end_group!();
        token
    };

    // Validate the token before persisting
    let api_client_with_token = api_client.with_token(token.clone());
    api_client_with_token
        .session()
        .await
        .map_err(|err| match err {
            SessionError::Unauthenticated => {
                anyhow!("Invalid token. The token is either malformed or has expired.")
            }
            SessionError::Other(err) => err,
        })?;

    let mut config = CodSpeedConfig::load_with_override(config_name, None)?;
    config.auth.token = Some(token);
    config.persist(config_name)?;
    debug!("Token saved to configuration file");

    info!("Login successful, your are now authenticated on CodSpeed");

    Ok(())
}

/// Detect the repository from the git remote of the current directory
fn detect_repository() -> Option<ParsedRepository> {
    let current_dir = std::env::current_dir().ok()?;
    let root_path = find_repository_root(&current_dir)?;
    let git_repository = Repository::open(&root_path).ok()?;
    let remote = git_repository.find_remote("origin").ok()?;
    let url = remote.url().ok()?;
    parse_repository_from_remote(url).ok()
}

/// Outcome of resolving the auth status, before rendering.
struct AuthStatus {
    session: Option<SessionPayload>,
    /// `Some(parsed)` when we detected a git remote and tried to look it up;
    /// the inner `Option<RepositoryOverviewPayload>` is `None` if the repo
    /// is not on CodSpeed (or we don't have a token to verify it with).
    detected_repository: Option<(ParsedRepository, Option<RepositoryOverviewPayload>)>,
}

pub async fn status(api_client: &CodSpeedAPIClient) -> Result<()> {
    let config = CodSpeedConfig::load_with_override(None, None)?;
    let has_token = config.auth.token.is_some();
    let parsed = detect_repository();

    let auth_status = if has_token {
        resolve_auth_status(api_client, parsed).await?
    } else {
        AuthStatus {
            session: None,
            detected_repository: parsed.map(|p| (p, None)),
        }
    };

    info!("{}", style("Authentication").bold());
    print_authentication_section(has_token, auth_status.session.as_ref());
    info!("");

    info!("{}", style("Repository").bold());
    let local_runs_fallback = print_repository_section(&auth_status.detected_repository, has_token);

    if local_runs_fallback {
        warn!(
            "Runs will be uploaded to a {} CodSpeed project not associated with any repository.",
            crate::cli::exec::DEFAULT_REPOSITORY_NAME
        );
    }

    Ok(())
}

/// Resolve the session and (when a git remote is detected) the repository overview.
async fn resolve_auth_status(
    api_client: &CodSpeedAPIClient,
    parsed: Option<ParsedRepository>,
) -> Result<AuthStatus> {
    let Some(parsed) = parsed else {
        let session = match api_client.session().await {
            Ok(payload) => Some(payload),
            Err(SessionError::Unauthenticated) => None,
            Err(SessionError::Other(err)) => return Err(err),
        };
        return Ok(AuthStatus {
            session,
            detected_repository: None,
        });
    };

    let combined = api_client
        .session_and_repository_overview(SessionAndRepositoryOverviewVars {
            owner: parsed.owner.clone(),
            name: parsed.name.clone(),
            provider: Some(parsed.provider.clone()),
        })
        .await;

    match combined {
        Ok(payload) => Ok(AuthStatus {
            session: Some(payload.session),
            detected_repository: Some((parsed, payload.repository_overview)),
        }),
        Err(SessionAndRepositoryOverviewError::Unauthenticated) => Ok(AuthStatus {
            session: None,
            detected_repository: Some((parsed, None)),
        }),
        Err(SessionAndRepositoryOverviewError::Other(err)) => Err(err),
    }
}

fn print_authentication_section(has_token: bool, session: Option<&SessionPayload>) {
    let Some(session) = session else {
        if has_token {
            info!(
                "  {} Token expired (run {} to re-authenticate)",
                cross_mark(),
                style("codspeed auth login").cyan()
            );
        } else {
            info!(
                "  {} Not logged in (run {} to authenticate)",
                cross_mark(),
                style("codspeed auth login").cyan()
            );
        }
        return;
    };

    if let Some(user) = &session.user {
        info!(
            "  {} Logged in as {} ({})",
            check_mark(),
            style(&user.login).bold(),
            user.provider
        );
    } else {
        // Repo-bound, non-user token (automation today, repo tokens later).
        info!("  {} Authenticated", check_mark());
    }
}

/// Render the repository section. Returns whether the local-runs fallback
/// warning should be printed.
fn print_repository_section(
    detected_repository: &Option<(ParsedRepository, Option<RepositoryOverviewPayload>)>,
    has_token: bool,
) -> bool {
    let Some((parsed, overview)) = detected_repository else {
        info!("  Not inside a git repository");
        return true;
    };

    let label = parsed.provider.to_string();

    let Some(overview) = overview else {
        if has_token {
            info!(
                "  {} {}/{} ({}, not enabled on CodSpeed)",
                cross_mark(),
                parsed.owner,
                parsed.name,
                label
            );
            return true;
        }
        info!("  {}/{} ({})", parsed.owner, parsed.name, label);
        return false;
    };

    if overview.has_write_access {
        info!(
            "  {} {}/{} ({})",
            check_mark(),
            overview.owner,
            overview.name,
            label
        );
        false
    } else {
        info!(
            "  {} {}/{} ({}, token not authorized for this repository)",
            cross_mark(),
            overview.owner,
            overview.name,
            label
        );
        false
    }
}
