use std::{
    env,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    str::FromStr,
    thread,
};

use reqwest::Client;
use tokio::fs;
use url::Url;

use crate::binary_pins::PinnedBinary;
use crate::cli::run::helpers::{download_pinned_file, get_env_variable};
use crate::prelude::*;

use super::MongoDBConfig;

#[derive(Debug, PartialEq, Eq)]
pub struct UserInput {
    mongo_uri: String,
    uri_env_name: String,
}

#[derive(Debug)]
pub struct MongoTracer {
    process: Option<Child>,
    server_address: String,
    profile_folder: PathBuf,
    proxy_mongo_uri: String,
    user_input: Option<UserInput>,
}

/// TODO: This implementation is not optimal: full lines might get split in multiple chunks.
/// This is not a problem for the current use case, as the tracer will be directly invoked as a .so library in the future,
/// inheriting the current process' stdout/stderr.
fn dump_tracer_log(mut stream: impl Read) -> Result<()> {
    let mut buf = [0u8; 1024];
    loop {
        let num_read = stream.read(&mut buf)?;
        if num_read == 0 {
            break;
        }

        let buf = &buf[..num_read];
        debug!("[MONGO TRACER LOGS] {}", String::from_utf8_lossy(buf));
    }

    Ok(())
}

impl MongoTracer {
    pub fn try_from(profile_folder: &Path, mongodb_config: &MongoDBConfig) -> Result<Self> {
        let user_input = match &mongodb_config.uri_env_name {
            Some(uri_env_name) => {
                debug!("Retrieving the value of {uri_env_name} to patch the MongoDB URL");
                Some(UserInput {
                    mongo_uri: get_env_variable(uri_env_name.as_str())?,
                    uri_env_name: uri_env_name.to_string(),
                })
            }
            None => None,
        };

        Ok(Self {
            process: None,
            server_address: "http://0.0.0.0:55581".into(),
            profile_folder: profile_folder.into(),
            // TODO: later choose a random available port dynamically, and/or make it configurable
            // we set the host to an ip address instead of localhost to prevent having to resolve the hostname
            proxy_mongo_uri: "mongodb://127.0.0.1:27018".into(),
            user_input,
        })
    }

    fn get_host_port_from_uris(&self) -> Result<(String, String, Option<String>)> {
        let server_address_uri = Url::parse(&self.server_address)?;
        let proxy_uri = Url::parse(&self.proxy_mongo_uri)?;
        let destination_uri = match &self.user_input {
            Some(user_input) => Some(Url::parse(&user_input.mongo_uri)?),
            None => None,
        };

        let server_address_host_port = format!(
            "{}:{}",
            server_address_uri.host_str().unwrap_or_default(),
            server_address_uri.port().unwrap_or_default()
        );
        let proxy_host_port = format!(
            "{}:{}",
            proxy_uri.host_str().unwrap_or_default(),
            proxy_uri.port().unwrap_or_default()
        );
        let destination_host_port = match destination_uri {
            Some(destination_uri) => {
                let parsing_error_fn = || {
                    anyhow!(
                        "Failed to parse the Mongo URI: {}. Be sure to follow the MongoDB URI format described here: https://www.mongodb.com/docs/manual/reference/connection-string/#connection-string-formats",
                        destination_uri.as_str()
                    )
                };

                Some(format!(
                    "{}:{}",
                    destination_uri.host_str().ok_or_else(parsing_error_fn)?,
                    destination_uri.port().ok_or_else(parsing_error_fn)?
                ))
            }
            None => None,
        };

        Ok((
            server_address_host_port,
            proxy_host_port,
            destination_host_port,
        ))
    }

    pub async fn start(&mut self) -> Result<()> {
        let mut command = Command::new("cs-mongo-tracer");
        let (server_address, proxy_host_port, destination_host_port) = self
            .get_host_port_from_uris()
            .context("Failed to parse the uris")?;

        let mut envs = vec![
            ("RUST_LOG", "debug"),
            (
                "CODSPEED_MONGO_INSTR_SERVER_ADDRESS",
                server_address.as_str(),
            ),
            ("CODSPEED_MONGO_PROXY_HOST_PORT", proxy_host_port.as_str()),
        ];
        if let Some(destination_host_port) = destination_host_port.as_ref() {
            envs.push((
                "CODSPEED_MONGO_DEST_HOST_PORT",
                destination_host_port.as_str(),
            ));
        }
        command.envs(envs);

        debug!("Start the MongoDB tracer: {command:?}");
        if let Some(destination_host_port) = destination_host_port {
            debug!("Proxy MongoDB from {proxy_host_port} to {destination_host_port}");
        } else {
            info!(
                "No MongoDB URI provided, user will have to provide it dynamically through the CodSpeed integration"
            );
        }
        let mut process = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let process_stdout = process.stdout.take().expect("error taking child stdout");
        let process_stderr = process.stderr.take().expect("error taking child stderr");
        thread::spawn(move || {
            dump_tracer_log(process_stdout).expect("error communicating with child stdout")
        });
        thread::spawn(move || {
            dump_tracer_log(process_stderr).expect("error communicating with child stderr")
        });

        self.process = Some(process);

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        let response = Client::new()
            .post(format!("{}/terminate", self.server_address))
            .send()
            .await?;
        if !response.status().is_success() {
            bail!("Failed to terminate the MongoDB tracer");
        }

        if let Some(process) = self.process.as_mut() {
            process
                .kill()
                .context("Failed to kill the MongoDB tracer")?;
        }

        let instruments_out_dir = Path::new(&self.profile_folder).join("instruments");
        fs::create_dir_all(&instruments_out_dir).await?;

        let mongo_data = response.bytes().await?;
        let mongo_data_path = instruments_out_dir.join("mongo.json");
        fs::write(mongo_data_path, mongo_data).await?;

        Ok(())
    }

    /// Applies the necessary transformations to the command to run the benchmark
    /// TODO: move this to a `Instrument` trait, refactor and implement it for valgring as well
    pub fn apply_run_command_transformations(&self, command: &mut Command) -> Result<()> {
        let mut envs = vec![(
            "CODSPEED_MONGO_INSTR_SERVER_ADDRESS",
            self.server_address.as_str(),
        )];

        let mut new_uri = Url::from_str(&self.proxy_mongo_uri)?;
        if let Some(user_input) = &self.user_input {
            let destination_uri = Url::parse(&user_input.mongo_uri)?;
            new_uri.set_path(destination_uri.path());
            let cleaned_query_pairs = destination_uri
                .query_pairs()
                .filter(|(k, _)| {
                    if k == "directConnection" {
                        info!("Overriding directionConnection to true. This is necessary to make the MongoDB tracer work.");
                        return false;
                    }
                    true
                });
            new_uri
                .query_pairs_mut()
                .extend_pairs(cleaned_query_pairs)
                .append_pair("directConnection", "true");

            envs.push((user_input.uri_env_name.as_str(), new_uri.as_str()));
        }

        command.envs(envs);

        Ok(())
    }
}

pub async fn install_mongodb_tracer() -> Result<()> {
    debug!("Installing mongodb-tracer");
    let installer_path = env::temp_dir().join("cs-mongo-tracer-installer.sh");
    download_pinned_file(PinnedBinary::MongoTracerInstaller, &installer_path).await?;

    let output = Command::new("bash")
        .arg(installer_path.to_str().unwrap())
        .stdout(Stdio::piped())
        .output()
        .map_err(|_| anyhow!("Failed to install mongo-tracer"))?;

    if !output.status.success() {
        info!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        error!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        bail!("Failed to install mongo-tracer");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use temp_env::with_var;

    use super::*;

    #[test]
    fn test_try_from() {
        with_var("MONGO_URL", "mongodb://localhost:27017".into(), || {
            let profile_folder = PathBuf::from("/tmp/codspeed");
            let mongodb_config = MongoDBConfig {
                uri_env_name: Some("MONGO_URL".into()),
            };

            let tracer = MongoTracer::try_from(&profile_folder, &mongodb_config).unwrap();

            assert!(tracer.process.is_none());
            assert_eq!(tracer.server_address, "http://0.0.0.0:55581");
            assert_eq!(tracer.profile_folder, profile_folder);
            assert_eq!(tracer.proxy_mongo_uri, "mongodb://127.0.0.1:27018");
            assert_eq!(
                tracer.user_input,
                Some(UserInput {
                    mongo_uri: "mongodb://localhost:27017".into(),
                    uri_env_name: "MONGO_URL".into(),
                })
            );
        });
    }

    #[test]
    fn test_try_from_empty_env() {
        let profile_folder = PathBuf::from("/tmp/codspeed");
        let mongodb_config = MongoDBConfig {
            uri_env_name: Some("MONGO_URL_NOT_FOUND".into()),
        };

        let tracer = MongoTracer::try_from(&profile_folder, &mongodb_config);

        assert!(tracer.is_err());
        assert_eq!(
            tracer.unwrap_err().to_string(),
            "MONGO_URL_NOT_FOUND environment variable not found"
        );
    }

    #[test]
    fn test_get_host_port_from_uris() {
        let tracer = MongoTracer {
            process: None,
            server_address: "http://0.0.0.0:55581".into(),
            profile_folder: "".into(),
            proxy_mongo_uri: "mongodb://127.0.0.1:27018".into(),
            user_input: Some(UserInput {
                mongo_uri: "mongodb://localhost:27017".into(),
                uri_env_name: "".into(),
            }),
        };

        let (server_address, proxy_host_port, destination_host_port) = tracer
            .get_host_port_from_uris()
            .expect("Failed to parse the uris");

        assert_eq!(server_address, "0.0.0.0:55581");
        assert_eq!(proxy_host_port, "127.0.0.1:27018");
        assert_eq!(destination_host_port, Some("localhost:27017".into()));
    }

    #[test]
    fn test_get_host_port_from_uris_no_input() {
        let tracer = MongoTracer {
            process: None,
            server_address: "http://0.0.0.0:55581".into(),
            profile_folder: "".into(),
            proxy_mongo_uri: "mongodb://127.0.0.1:27018".into(),
            user_input: None,
        };

        let (server_address, proxy_host_port, destination_host_port) = tracer
            .get_host_port_from_uris()
            .expect("Failed to parse the uris");

        assert_eq!(server_address, "0.0.0.0:55581");
        assert_eq!(proxy_host_port, "127.0.0.1:27018");
        assert_eq!(destination_host_port, None);
    }

    #[test]
    fn test_get_host_port_from_uris_error() {
        let tracer = MongoTracer {
            process: None,
            server_address: "http://0.0.0.0:55581".into(),
            profile_folder: "".into(),
            proxy_mongo_uri: "mongodb://127.0.0.1:27018".into(),
            user_input: Some(UserInput {
                mongo_uri: "localhost:27017".into(),
                uri_env_name: "".into(),
            }),
        };

        let result = tracer.get_host_port_from_uris();

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Failed to parse the Mongo URI: localhost:27017. Be sure to follow the MongoDB URI format described here: https://www.mongodb.com/docs/manual/reference/connection-string/#connection-string-formats"
        );
    }

    #[test]
    fn test_apply_run_command_transformations() {
        let mut command = Command::new("cargo");
        command.args(vec!["codspeed", "bench"]);

        let tracer = MongoTracer {
            process: None,
            server_address: "http://0.0.0.0:55581".into(),
            profile_folder: "".into(),
            proxy_mongo_uri: "mongodb://127.0.0.1:27018".into(),
            user_input: Some(UserInput {
                mongo_uri: "mongodb://localhost:27017/my-database".into(),
                uri_env_name: "MONGO_URL".into(),
            }),
        };

        tracer
            .apply_run_command_transformations(&mut command)
            .expect("Failed to apply the transformations");

        assert_eq!(
            command.get_envs().collect_vec(),
            vec![
                (
                    OsStr::new("CODSPEED_MONGO_INSTR_SERVER_ADDRESS"),
                    Some(OsStr::new("http://0.0.0.0:55581"))
                ),
                (
                    OsStr::new("MONGO_URL"),
                    Some(OsStr::new(
                        "mongodb://127.0.0.1:27018/my-database?directConnection=true"
                    ))
                ),
            ]
        );
    }

    #[test]
    fn test_apply_run_command_transformations_no_paths() {
        let mut command = Command::new("cargo");
        command.args(vec!["codspeed", "bench"]);

        let tracer = MongoTracer {
            process: None,
            server_address: "http://0.0.0.0:55581".into(),
            profile_folder: "".into(),
            proxy_mongo_uri: "mongodb://127.0.0.1:27018".into(),
            user_input: Some(UserInput {
                mongo_uri: "mongodb://localhost:27017".into(),
                uri_env_name: "MONGO_URL".into(),
            }),
        };

        tracer
            .apply_run_command_transformations(&mut command)
            .expect("Failed to apply the transformations");

        assert_eq!(
            command.get_envs().collect_vec(),
            vec![
                (
                    OsStr::new("CODSPEED_MONGO_INSTR_SERVER_ADDRESS"),
                    Some(OsStr::new("http://0.0.0.0:55581"))
                ),
                (
                    OsStr::new("MONGO_URL"),
                    Some(OsStr::new(
                        "mongodb://127.0.0.1:27018?directConnection=true"
                    ))
                ),
            ]
        );
    }

    #[test]
    fn test_apply_run_command_transformations_direct_connection() {
        let mut command = Command::new("cargo");
        command.args(vec!["codspeed", "bench"]);

        let tracer = MongoTracer {
            process: None,
            server_address: "http://0.0.0.0:55581".into(),
            profile_folder: "".into(),
            proxy_mongo_uri: "mongodb://127.0.0.1:27018".into(),
            user_input: Some(UserInput {
                mongo_uri: "mongodb://localhost:27017?w=majority&directConnection=false".into(),
                uri_env_name: "MONGO_URL".into(),
            }),
        };

        tracer
            .apply_run_command_transformations(&mut command)
            .expect("Failed to apply the transformations");

        assert_eq!(
            command.get_envs().collect_vec(),
            vec![
                (
                    OsStr::new("CODSPEED_MONGO_INSTR_SERVER_ADDRESS"),
                    Some(OsStr::new("http://0.0.0.0:55581"))
                ),
                (
                    OsStr::new("MONGO_URL"),
                    Some(OsStr::new(
                        "mongodb://127.0.0.1:27018?w=majority&directConnection=true"
                    ))
                ),
            ]
        );
    }

    #[test]
    fn test_apply_run_command_transformations_no_user_input() {
        let mut command = Command::new("cargo");
        command.args(vec!["codspeed", "bench"]);

        let tracer = MongoTracer {
            process: None,
            server_address: "http://0.0.0.0:55581".into(),
            profile_folder: "".into(),
            proxy_mongo_uri: "mongodb://127.0.0.1:27018".into(),
            user_input: None,
        };

        tracer
            .apply_run_command_transformations(&mut command)
            .expect("Failed to apply the transformations");

        assert_eq!(
            command.get_envs().collect_vec(),
            vec![(
                OsStr::new("CODSPEED_MONGO_INSTR_SERVER_ADDRESS"),
                Some(OsStr::new("http://0.0.0.0:55581"))
            )]
        );
    }
}
