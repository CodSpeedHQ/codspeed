use crate::executor::ExecutorConfig;
use crate::executor::helpers::{introspected_golang, introspected_nodejs};
use crate::prelude::*;
use crate::runner_mode::RunnerMode;
use std::{collections::HashMap, env::consts::ARCH, path::Path};

pub fn get_base_injected_env(
    mode: RunnerMode,
    profile_folder: &Path,
    config: &ExecutorConfig,
) -> HashMap<String, String> {
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
        ("PYTHONHASHSEED".into(), "0".into()),
        (
            "PYTHON_PERF_JIT_SUPPORT".into(),
            if mode == RunnerMode::Walltime {
                "1".into()
            } else {
                "0".into()
            },
        ),
        ("ARCH".into(), ARCH.into()),
        ("CODSPEED_ENV".into(), "runner".into()),
        (
            "CODSPEED_RUNNER_MODE".into(),
            runner_mode_internal_env_value.into(),
        ),
        (
            "CODSPEED_PROFILE_FOLDER".into(),
            profile_folder.to_string_lossy().to_string(),
        ),
    ]);

    // Java: Enable frame pointers and perf map generation for flamegraph profiling.
    // - UnlockDiagnosticVMOptions must come before DumpPerfMapAtExit (diagnostic option).
    // - PreserveFramePointer: Preserves frame pointers for profiling.
    // - DumpPerfMapAtExit: Writes /tmp/perf-<pid>.map on JVM exit for symbol resolution.
    // - DebugNonSafepoints: Enables debug info for JIT-compiled non-safepoint code.
    // - EnableDynamicAgentLoading: Suppresses warning when loading JVMTI agents at runtime.
    // - jdk.attach.allowAttachSelf: Allows the JVM to attach a JVMTI agent to itself
    //   (used by codspeed-jvm's perf-map agent for @Fork(0) benchmarks).
    if mode == RunnerMode::Walltime {
        env.insert(
            "JAVA_TOOL_OPTIONS".into(),
            "-XX:+PreserveFramePointer -XX:+UnlockDiagnosticVMOptions -XX:+DebugNonSafepoints -XX:+EnableDynamicAgentLoading -Djdk.attach.allowAttachSelf=true".into(),
        );
    }

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
