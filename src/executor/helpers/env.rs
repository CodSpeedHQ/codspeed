use std::{collections::HashMap, env::consts::ARCH, path::Path};

use crate::executor::ExecutorConfig;
use crate::runner_mode::RunnerMode;

pub fn get_base_injected_env(
    mode: RunnerMode,
    profile_folder: &Path,
    config: &ExecutorConfig,
) -> HashMap<&'static str, String> {
    let runner_mode_internal_env_value = match mode {
        // While the runner now deprecates the usage of instrumentation with a message, we
        // internally still use instrumentation temporarily to give time to users to upgrade their
        // integrations to a version that accepts both instrumentation and simulation.
        // TODO: Remove Instrumentation mode completely in the next major release, and set this
        // value to simulation instead.
        #[allow(deprecated)]
        RunnerMode::Instrumentation | RunnerMode::Simulation => "instrumentation",
        RunnerMode::Walltime => "walltime",
        RunnerMode::Memory => "memory",
    };
    let mut env = HashMap::from([
        ("PYTHONHASHSEED", "0".into()),
        (
            "PYTHON_PERF_JIT_SUPPORT",
            if mode == RunnerMode::Walltime {
                "1".into()
            } else {
                "0".into()
            },
        ),
        ("ARCH", ARCH.into()),
        ("CODSPEED_ENV", "runner".into()),
        (
            "CODSPEED_RUNNER_MODE",
            runner_mode_internal_env_value.into(),
        ),
        (
            "CODSPEED_PROFILE_FOLDER",
            profile_folder.to_string_lossy().to_string(),
        ),
    ]);

    if let Some(version) = &config.go_runner_version {
        env.insert("CODSPEED_GO_RUNNER_VERSION", version.to_string());
    }

    env
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
