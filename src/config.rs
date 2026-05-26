use std::{collections::BTreeMap, env, fs, path::PathBuf};

use crate::prelude::*;
use crate::shell_session_store::{self, SessionKind};
use serde::{Deserialize, Serialize};

pub const DEFAULT_API_URL: &str = "https://gql.codspeed.io/";
pub const DEFAULT_UPLOAD_URL: &str = "https://api.codspeed.io/upload";
pub const DEFAULT_PROFILE_NAME: &str = "default";

/// Current on-disk schema version. Bump when introducing a new migration.
const CURRENT_CONFIG_VERSION: u32 = 1;

/// Persist `profile_name` as the active profile for the current shell session.
pub fn register_shell_session_profile(profile_name: &str) -> Result<()> {
    shell_session_store::register(SessionKind::Profile, &profile_name.to_owned())
}

/// Look up the active profile for the current shell session, if any.
pub fn load_shell_session_profile() -> Result<Option<String>> {
    shell_session_store::load::<String>(SessionKind::Profile)
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AuthConfig {
    pub token: Option<String>,
}

impl AuthConfig {
    fn is_empty(&self) -> bool {
        self.token.is_none()
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ProfileConfig {
    #[serde(default, skip_serializing_if = "AuthConfig::is_empty")]
    pub auth: AuthConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_url: Option<String>,
}

/// Raw shape read from disk. Captures every YAML field we have ever
/// written, including legacy ones, so `migrate` can fold them into the
/// canonical [`PersistedConfig`]. This type is the only place legacy
/// fields are mentioned — it is private to this module.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct RawConfig {
    #[serde(default)]
    version: Option<u32>,
    /// v0 legacy: token used to live at the top level. Migrated into
    /// `profiles.default.auth.token` on load.
    #[serde(default)]
    auth: Option<AuthConfig>,
    #[serde(default)]
    profiles: BTreeMap<String, ProfileConfig>,
}

/// The on-disk shape: schema version + named profiles. This is what
/// `serde_yaml` writes back when we persist. It carries no resolved
/// runtime state — the runtime view lives in [`CodSpeedConfig`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
struct PersistedConfig {
    version: u32,
    profiles: BTreeMap<String, ProfileConfig>,
}

impl Default for PersistedConfig {
    fn default() -> Self {
        let mut profiles = BTreeMap::new();
        profiles.insert(DEFAULT_PROFILE_NAME.to_owned(), ProfileConfig::default());
        Self {
            version: CURRENT_CONFIG_VERSION,
            profiles,
        }
    }
}

/// CLI-supplied overrides applied on top of the selected profile when
/// loading the config. Each field, if `Some`, wins over the profile's
/// own value.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConfigOverrides<'a> {
    pub oauth_token: Option<&'a str>,
    pub api_url: Option<&'a str>,
    pub upload_url: Option<&'a str>,
}

/// Configuration as seen at runtime: the persisted state plus the
/// resolved auth/URLs/profile selected for this invocation.
///
/// Stored at `~/.config/codspeed/config.yaml` by default. Legacy YAML
/// formats are normalised by `migrate` at load time and re-persisted
/// immediately, so the on-disk shape always matches
/// [`PersistedConfig`].
#[derive(Debug, Clone)]
pub struct CodSpeedConfig {
    persisted: PersistedConfig,
    pub auth: AuthConfig,
    pub api_url: String,
    pub upload_url: String,
    selected_profile: String,
}

fn default_profile_name() -> String {
    DEFAULT_PROFILE_NAME.to_owned()
}

fn default_api_url() -> String {
    DEFAULT_API_URL.to_owned()
}

fn default_upload_url() -> String {
    DEFAULT_UPLOAD_URL.to_owned()
}

/// Get the path to the configuration file, following the XDG Base Directory Specification
/// at https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html
///
/// If config_name is None, returns ~/.config/codspeed/config.yaml (default)
/// If config_name is Some, returns ~/.config/codspeed/{config_name}.yaml
fn get_configuration_file_path(config_name: Option<&str>) -> PathBuf {
    let config_dir = env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = env::var("HOME").expect("HOME env variable not set");
            PathBuf::from(home).join(".config")
        });
    let config_dir = config_dir.join("codspeed");

    match config_name {
        Some(name) => config_dir.join(format!("{name}.yaml")),
        None => config_dir.join("config.yaml"),
    }
}

impl CodSpeedConfig {
    /// Wrap a [`PersistedConfig`] with placeholder runtime values. The
    /// caller is expected to invoke [`Self::resolve_selected_profile`]
    /// to populate them before exposing the result.
    fn from_persisted(persisted: PersistedConfig) -> Self {
        Self {
            persisted,
            auth: AuthConfig::default(),
            api_url: default_api_url(),
            upload_url: default_upload_url(),
            selected_profile: default_profile_name(),
        }
    }
}

/// Fold a [`RawConfig`] into the canonical [`PersistedConfig`],
/// returning the upgraded persisted state and a flag indicating
/// whether the on-disk shape needed changes (in which case the caller
/// should re-persist).
///
/// Bails if the file was written by a newer schema version than this
/// CLI knows about, to avoid silently downgrading it (which would
/// drop any fields the newer schema added).
fn migrate(raw: RawConfig) -> Result<(PersistedConfig, bool)> {
    // Fast path: already at the current schema version. Legacy fields
    // cannot exist in a v1 file we wrote, so anything stray is a hand
    // edit and we deliberately ignore it.
    if raw.version == Some(CURRENT_CONFIG_VERSION) {
        let mut profiles = raw.profiles;
        profiles.entry(DEFAULT_PROFILE_NAME.to_owned()).or_default();
        return Ok((
            PersistedConfig {
                version: CURRENT_CONFIG_VERSION,
                profiles,
            },
            false,
        ));
    }

    let raw_version = raw.version.unwrap_or(0);
    if raw_version > CURRENT_CONFIG_VERSION {
        bail!(
            "Config file was written by a newer version of CodSpeed (schema v{raw_version}, this CLI supports v{CURRENT_CONFIG_VERSION}). Upgrade the CLI to read it."
        );
    }
    let mut dirty = raw_version != CURRENT_CONFIG_VERSION;

    let mut profiles = raw.profiles;

    // v0 → v1: move legacy top-level auth.token into profiles.default
    // (only if the profile slot is empty, so we don't clobber a value
    // the user explicitly set per-profile).
    if let Some(legacy_auth) = raw.auth
        && let Some(token) = legacy_auth.token
    {
        dirty = true;
        let default_profile = profiles.entry(DEFAULT_PROFILE_NAME.to_owned()).or_default();
        if default_profile.auth.token.is_none() {
            default_profile.auth.token = Some(token);
        }
    }

    // Ensure the default profile exists so consumers can rely on it.
    profiles.entry(DEFAULT_PROFILE_NAME.to_owned()).or_default();

    Ok((
        PersistedConfig {
            version: CURRENT_CONFIG_VERSION,
            profiles,
        },
        dirty,
    ))
}

/// Write the canonical [`PersistedConfig`] to disk.
fn write_persisted(persisted: &PersistedConfig, config_name: Option<&str>) -> Result<()> {
    let config_path = get_configuration_file_path(config_name);
    fs::create_dir_all(config_path.parent().unwrap())?;
    let config_str = serde_yaml::to_string(persisted)?;
    fs::write(&config_path, config_str)?;
    debug!("Config written to {}", config_path.display());
    Ok(())
}

impl CodSpeedConfig {
    pub fn load_with_profile(
        config_name: Option<&str>,
        profile_name: Option<&str>,
        overrides: ConfigOverrides<'_>,
        allow_missing_profile: bool,
    ) -> Result<Self> {
        let config_path = get_configuration_file_path(config_name);

        let (persisted, was_migrated) = match fs::read(&config_path) {
            Ok(config_str) => {
                let raw: RawConfig = serde_yaml::from_slice(&config_str).context(format!(
                    "Failed to parse CodSpeed config at {}",
                    config_path.display()
                ))?;
                debug!("Config loaded from {}", config_path.display());
                migrate(raw)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!("Config file not found at {}", config_path.display());
                (PersistedConfig::default(), false)
            }
            Err(e) => bail!("Failed to load config: {e}"),
        };

        if was_migrated {
            debug!(
                "Upgrading config at {} to v{CURRENT_CONFIG_VERSION}",
                config_path.display()
            );
            write_persisted(&persisted, config_name)?;
        }

        let mut config = Self::from_persisted(persisted);

        config.resolve_selected_profile(profile_name, overrides, allow_missing_profile)?;

        Ok(config)
    }

    /// Persist the canonical on-disk configuration. Runtime resolved
    /// fields (auth, api_url, upload_url) are not written — only what
    /// is in [`PersistedConfig`].
    pub fn persist(&self, config_name: Option<&str>) -> Result<()> {
        write_persisted(&self.persisted, config_name)
    }

    pub fn selected_profile_name(&self) -> &str {
        &self.selected_profile
    }

    pub fn profiles(&self) -> &BTreeMap<String, ProfileConfig> {
        &self.persisted.profiles
    }

    pub fn profile(&self, profile_name: &str) -> Option<&ProfileConfig> {
        self.persisted.profiles.get(profile_name)
    }

    pub fn profile_mut(&mut self, profile_name: &str) -> &mut ProfileConfig {
        self.persisted
            .profiles
            .entry(profile_name.to_owned())
            .or_default()
    }

    fn resolve_selected_profile(
        &mut self,
        profile_name: Option<&str>,
        overrides: ConfigOverrides<'_>,
        allow_missing_profile: bool,
    ) -> Result<()> {
        let selected_profile_name = match profile_name {
            Some(name) => name.to_owned(),
            None => {
                load_shell_session_profile()?.unwrap_or_else(|| DEFAULT_PROFILE_NAME.to_owned())
            }
        };
        let profile = match self.profile(&selected_profile_name) {
            Some(profile) => profile.clone(),
            None if allow_missing_profile => ProfileConfig::default(),
            None => {
                bail!(
                    "CodSpeed profile `{selected_profile_name}` does not exist. Run `codspeed profile set {selected_profile_name}` to create it."
                );
            }
        };

        self.selected_profile = selected_profile_name;
        self.auth = AuthConfig {
            token: overrides
                .oauth_token
                .map(ToOwned::to_owned)
                .or(profile.auth.token),
        };
        self.api_url = overrides
            .api_url
            .map(ToOwned::to_owned)
            .or(profile.api_url)
            .unwrap_or_else(default_api_url);
        self.upload_url = overrides
            .upload_url
            .map(ToOwned::to_owned)
            .or(profile.upload_url)
            .unwrap_or_else(default_upload_url);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn parse_raw(yaml: &str) -> RawConfig {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn migrate_v0_with_top_level_auth_token() {
        let (persisted, dirty) = migrate(parse_raw(
            r#"
auth:
  token: old-token
"#,
        ))
        .unwrap();

        assert!(dirty);
        assert_eq!(persisted.version, CURRENT_CONFIG_VERSION);
        assert_eq!(
            persisted.profiles[DEFAULT_PROFILE_NAME]
                .auth
                .token
                .as_deref(),
            Some("old-token")
        );
    }

    #[test]
    fn migrate_canonical_is_idempotent() {
        let (persisted, dirty) = migrate(parse_raw(
            r#"
version: 1
profiles:
  default:
    auth:
      token: tok
"#,
        ))
        .unwrap();

        assert!(!dirty);
        assert_eq!(persisted.version, CURRENT_CONFIG_VERSION);
        assert_eq!(
            persisted.profiles[DEFAULT_PROFILE_NAME]
                .auth
                .token
                .as_deref(),
            Some("tok")
        );
    }

    #[test]
    fn migrate_preserves_existing_profile_token() {
        let (persisted, dirty) = migrate(parse_raw(
            r#"
auth:
  token: legacy-token
profiles:
  default:
    auth:
      token: profile-token
"#,
        ))
        .unwrap();

        assert!(dirty);
        // existing per-profile token wins
        assert_eq!(
            persisted.profiles[DEFAULT_PROFILE_NAME]
                .auth
                .token
                .as_deref(),
            Some("profile-token")
        );
    }

    #[test]
    fn migrate_refuses_newer_schema_version() {
        let err = migrate(parse_raw(
            r#"
version: 999
profiles:
  default: {}
"#,
        ))
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("newer version") && msg.contains("v999"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn resolves_profile_values_with_overrides() {
        let mut config = CodSpeedConfig::from_persisted(PersistedConfig::default());
        let profile = config.profile_mut("staging");
        profile.auth.token = Some("profile-token".into());
        profile.api_url = Some("https://gql.staging.example/".into());
        profile.upload_url = Some("https://api.staging.example/upload".into());

        config
            .resolve_selected_profile(
                Some("staging"),
                ConfigOverrides {
                    oauth_token: Some("override-token"),
                    api_url: Some("https://gql.override.example/"),
                    upload_url: None,
                },
                false,
            )
            .unwrap();

        assert_eq!(config.selected_profile_name(), "staging");
        assert_eq!(config.auth.token.as_deref(), Some("override-token"));
        assert_eq!(config.api_url, "https://gql.override.example/");
        assert_eq!(config.upload_url, "https://api.staging.example/upload");
    }

    #[test]
    fn load_rewrites_legacy_file_in_canonical_form() {
        let tmp = TempDir::new().unwrap();
        // SAFETY: tests run single-threaded by default but env mutations
        // affect the whole process; this test does not parallelise with
        // others that mutate XDG_CONFIG_HOME.
        unsafe {
            env::set_var("XDG_CONFIG_HOME", tmp.path());
        }

        let config_dir = tmp.path().join("codspeed");
        fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("config.yaml");
        fs::write(
            &config_path,
            "auth:\n  token: legacy-token\nprofiles:\n  staging: {}\n",
        )
        .unwrap();

        CodSpeedConfig::load_with_profile(None, Some("staging"), ConfigOverrides::default(), false)
            .unwrap();

        let on_disk = fs::read_to_string(&config_path).unwrap();
        assert!(
            on_disk.starts_with("version: 1\n"),
            "expected version preamble, got:\n{on_disk}"
        );
        // top-level legacy auth: gone (only profile-level auth: remains)
        assert!(
            !on_disk.contains("\nauth:\n"),
            "legacy top-level auth should be gone, got:\n{on_disk}"
        );
        assert!(
            on_disk.contains("legacy-token"),
            "token should be migrated into profiles.default, got:\n{on_disk}"
        );

        // second load is a no-op on disk
        let mtime_before = fs::metadata(&config_path).unwrap().modified().unwrap();
        CodSpeedConfig::load_with_profile(None, Some("staging"), ConfigOverrides::default(), false)
            .unwrap();
        let mtime_after = fs::metadata(&config_path).unwrap().modified().unwrap();
        assert_eq!(
            mtime_before, mtime_after,
            "canonical file should not be rewritten"
        );
    }
}
