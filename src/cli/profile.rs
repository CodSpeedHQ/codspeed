use crate::config::{
    CodSpeedConfig, ConfigOverrides, load_shell_session_profile, register_shell_session_profile,
};
use crate::prelude::*;
use clap::{Args, Subcommand};
use console::style;

#[derive(Debug, Args)]
pub struct ProfileArgs {
    #[command(subcommand)]
    command: ProfileCommands,
}

#[derive(Debug, Subcommand)]
enum ProfileCommands {
    /// List configured profiles
    List,
    /// Show a profile
    Show {
        /// Profile name. Defaults to the configured default profile.
        name: Option<String>,
    },
    /// Set profile URLs, creating the profile if it does not exist
    Set {
        /// Profile name
        name: String,
        /// The URL of the CodSpeed GraphQL API
        #[arg(long)]
        api_url: Option<String>,
        /// The URL to use for uploading results
        #[arg(long)]
        upload_url: Option<String>,
    },
    /// Set the active profile for the current shell session
    Use {
        /// Profile name
        name: String,
    },
}

pub fn run(
    args: ProfileArgs,
    config_name: Option<&str>,
    selected_profile: Option<&str>,
) -> Result<()> {
    match args.command {
        ProfileCommands::List => list(config_name),
        ProfileCommands::Show { name } => show(config_name, name.as_deref().or(selected_profile)),
        ProfileCommands::Set {
            name,
            api_url,
            upload_url,
        } => set(config_name, &name, api_url, upload_url),
        ProfileCommands::Use { name } => use_profile(config_name, &name),
    }
}

fn list(config_name: Option<&str>) -> Result<()> {
    let config =
        CodSpeedConfig::load_with_profile(config_name, None, ConfigOverrides::default(), true)?;
    let active = load_shell_session_profile()?;

    info!("{}", style("Profiles").bold());
    for name in config.profiles().keys() {
        let marker = if Some(name) == active.as_ref() {
            "*"
        } else {
            " "
        };
        info!("  {marker} {name}");
    }

    Ok(())
}

fn show(config_name: Option<&str>, profile_name: Option<&str>) -> Result<()> {
    let config = CodSpeedConfig::load_with_profile(
        config_name,
        profile_name,
        ConfigOverrides::default(),
        false,
    )?;

    info!(
        "{} ({})",
        style("Profile").bold(),
        config.selected_profile_name()
    );
    info!("  api url: {}", config.api_url);
    info!("  upload url: {}", config.upload_url);
    info!(
        "  authenticated: {}",
        if config.auth.token.is_some() {
            "yes"
        } else {
            "no"
        }
    );

    Ok(())
}

fn set(
    config_name: Option<&str>,
    profile_name: &str,
    api_url: Option<String>,
    upload_url: Option<String>,
) -> Result<()> {
    let mut config =
        CodSpeedConfig::load_with_profile(config_name, None, ConfigOverrides::default(), true)?;
    let profile = config.profile_mut(profile_name);

    if let Some(api_url) = api_url {
        profile.api_url = Some(api_url);
    }
    if let Some(upload_url) = upload_url {
        profile.upload_url = Some(upload_url);
    }

    config.persist(config_name)?;
    info!("Profile `{profile_name}` saved");

    Ok(())
}

fn use_profile(config_name: Option<&str>, profile_name: &str) -> Result<()> {
    let config =
        CodSpeedConfig::load_with_profile(config_name, None, ConfigOverrides::default(), true)?;
    ensure!(
        config.profile(profile_name).is_some(),
        "CodSpeed profile `{profile_name}` does not exist. Run `codspeed profile set {profile_name}` to create it."
    );

    register_shell_session_profile(profile_name)?;
    info!("Active CodSpeed profile set to `{profile_name}` for this shell session");

    Ok(())
}
