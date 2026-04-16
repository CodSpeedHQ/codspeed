use std::{fs, path::PathBuf};

use crate::prelude::*;
use nestify::nest;
use serde::{Deserialize, Serialize};

nest! {
    #[derive(Debug, Deserialize, Serialize)]*
    #[serde(rename_all = "kebab-case")]*
    /// Persistent configuration for CodSpeed CLI.
    ///
    /// This struct represents the user's persistent configuration stored in the filesystem,
    /// typically at `~/.config/codspeed/config.yaml`. It contains settings that persist
    /// across multiple benchmark runs, such as authentication credentials.
    ///
    /// The configuration follows the XDG Base Directory Specification and can be loaded
    /// with [`CodSpeedConfig::load_with_override`] or persisted with [`CodSpeedConfig::persist`].
    pub struct CodSpeedConfig {
        pub auth: pub struct AuthConfig {
            pub token: Option<String>,
        }
    }
}

/// Get the path to the configuration file, following the XDG Base Directory Specification
/// at https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html
///
/// If config_name is None, returns ~/.config/codspeed/config.yaml (default)
/// If config_name is Some, returns ~/.config/codspeed/{config_name}.yaml
fn get_configuration_file_path(config_name: Option<&str>) -> PathBuf {
    let config_dir = dirs::config_dir().expect("Could not determine config directory");
    let config_dir = config_dir.join("codspeed");

    match config_name {
        Some(name) => config_dir.join(format!("{name}.yaml")),
        None => config_dir.join("config.yaml"),
    }
}

impl Default for CodSpeedConfig {
    fn default() -> Self {
        Self {
            auth: AuthConfig { token: None },
        }
    }
}

impl CodSpeedConfig {
    /// Load the configuration. If it does not exist, return a default configuration.
    ///
    /// If oauth_token_override is provided, the token from the loaded configuration will be
    /// ignored, and the override will be used instead
    pub fn load_with_override(
        config_name: Option<&str>,
        oauth_token_override: Option<&str>,
    ) -> Result<Self> {
        let config_path = get_configuration_file_path(config_name);

        let mut config = match fs::read(&config_path) {
            Ok(config_str) => {
                let config: CodSpeedConfig =
                    serde_yaml::from_slice(&config_str).context(format!(
                        "Failed to parse CodSpeed config at {}",
                        config_path.display()
                    ))?;
                debug!("Config loaded from {}", config_path.display());
                config
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!("Config file not found at {}", config_path.display());
                CodSpeedConfig::default()
            }
            Err(e) => bail!("Failed to load config: {e}"),
        };

        if let Some(oauth_token) = oauth_token_override {
            config.auth.token = Some(oauth_token.to_owned());
        }

        Ok(config)
    }

    /// Persist changes to the configuration
    pub fn persist(&self, config_name: Option<&str>) -> Result<()> {
        let config_path = get_configuration_file_path(config_name);
        fs::create_dir_all(config_path.parent().unwrap())?;

        let config_str = serde_yaml::to_string(self)?;
        fs::write(&config_path, config_str)?;
        debug!("Config written to {}", config_path.display());

        Ok(())
    }
}
