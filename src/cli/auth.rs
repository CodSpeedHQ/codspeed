use std::time::Duration;

use crate::api_client::{CodSpeedAPIClient, GetRepositoryVars};
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
    Login,
    /// Show the authentication status
    Status,
}

pub async fn run(
    args: AuthArgs,
    api_client: &CodSpeedAPIClient,
    config_name: Option<&str>,
) -> Result<()> {
    match args.command {
        AuthCommands::Login => login(api_client, config_name).await?,
        AuthCommands::Status => status(api_client).await?,
    }
    Ok(())
}

const LOGIN_SESSION_MAX_DURATION: Duration = Duration::from_secs(60 * 5); // 5 minutes

async fn login(api_client: &CodSpeedAPIClient, config_name: Option<&str>) -> Result<()> {
    debug!("Login to CodSpeed");
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
    let url = remote.url()?;
    parse_repository_from_remote(url).ok()
}

pub async fn status(api_client: &CodSpeedAPIClient) -> Result<()> {
    let config = CodSpeedConfig::load_with_override(None, None)?;
    let has_token = config.auth.token.is_some();
    let detected_repo = detect_repository();

    // 1. Check token validity
    let current_user = if has_token {
        api_client.get_current_user().await.ok().flatten()
    } else {
        None
    };
    let token_valid = current_user.is_some();

    info!("{}", style("Authentication").bold());
    if let Some(user) = current_user {
        info!(
            "  {} Logged in as {} ({})",
            check_mark(),
            style(&user.login).bold(),
            user.provider
        );
    } else if has_token {
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
    info!("");

    // 2. If token is valid and we detected a repo, check repository existence
    info!("{}", style("Repository").bold());
    let local_runs_fallback = match detected_repo {
        Some(parsed) => {
            let label = parsed.provider.to_string();
            if token_valid {
                let repo_exists = api_client
                    .get_repository(GetRepositoryVars {
                        owner: parsed.owner.clone(),
                        name: parsed.name.clone(),
                        provider: parsed.provider.clone(),
                    })
                    .await
                    .ok()
                    .flatten()
                    .is_some();
                if repo_exists {
                    info!(
                        "  {} {}/{} ({})",
                        check_mark(),
                        parsed.owner,
                        parsed.name,
                        label
                    );
                    false
                } else {
                    info!(
                        "  {} {}/{} ({}, not enabled on CodSpeed)",
                        cross_mark(),
                        parsed.owner,
                        parsed.name,
                        label
                    );
                    true
                }
            } else {
                info!("  {}/{} ({})", parsed.owner, parsed.name, label);
                false
            }
        }
        None => {
            info!("  Not inside a git repository");
            true
        }
    };

    if local_runs_fallback {
        warn!(
            "Runs will be uploaded to a {} CodSpeed project not associated with any repository.",
            crate::cli::exec::DEFAULT_REPOSITORY_NAME
        );
    }

    Ok(())
}
