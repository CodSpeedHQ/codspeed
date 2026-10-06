use crate::executor::ExecutorConfig;
use crate::executor::helpers::{introspected_golang, introspected_nodejs};
use crate::prelude::*;
use crate::runner_mode::RunnerMode;
use runner_shared::measurement_mode::MeasurementMode;
use std::{collections::HashMap, env::consts::ARCH, path::Path};

pub fn get_base_injected_env(
    mode: RunnerMode,
    profile_folder: &Path,
    config: &ExecutorConfig,
) -> HashMap<String, String> {
    let mut env = HashMap::from([
        ("ARCH".into(), ARCH.into()),
        ("CODSPEED_ENV".into(), "runner".into()),
        (
            "CODSPEED_PROFILE_FOLDER".into(),
            profile_folder.to_string_lossy().to_string(),
        ),
    ]);
    env.extend(
        runner_shared::runtime_env::env(MeasurementMode::from(&mode))
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value)),
    );

    if let Some(version) = &config.go_runner_version {
        env.insert("CODSPEED_GO_RUNNER_VERSION".into(), version.to_string());
    }

    env.extend(config.extra_env.clone());

    env
}

/// Set the env variable to not warn users about Go's perf unwinding mode when running Go benchmarks
pub fn suppress_go_perf_unwinding_warning() {
    // Safety: no multithreading
    unsafe {
        std::env::set_var("CODSPEED_GO_SUPPRESS_PERF_UNWINDING_MODE_WARNING", "true");
    }
}

/// Build the `PATH` value with optional language introspection wrappers prepended.
///
/// When `enable_introspection` is true, the Node.js and Go wrapper script
/// directories are prepended to the current `PATH`. Otherwise the current
/// `PATH` is returned unchanged.
pub fn build_path_env(enable_introspection: bool) -> Result<String> {
    let path_env = std::env::var("PATH").unwrap_or_default();
    if !enable_introspection {
        return Ok(path_env);
    }

    let node_path = introspected_nodejs::setup()
        .map_err(|e| anyhow!("failed to setup NodeJS introspection. {e}"))?;
    let go_path = introspected_golang::setup()
        .map_err(|e| anyhow!("failed to setup Go introspection. {e}"))?;

    Ok(format!(
        "{}:{}:{}",
        node_path.to_string_lossy(),
        go_path.to_string_lossy(),
        path_env,
    ))
}

pub fn is_codspeed_debug_enabled() -> bool {
    std::env::var("CODSPEED_LOG")
        .ok()
        .and_then(|log_level| {
            log_level
                .parse::<log::LevelFilter>()
                .map(|level| level >= log::LevelFilter::Debug)
                .ok()
        })
        .unwrap_or_default()
}
