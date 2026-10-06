use crate::measurement_mode::MeasurementMode;

mod java;
mod python;

pub const RUNNER_MODE_ENV: &str = "CODSPEED_RUNNER_MODE";

/// Env vars to set on benchmark processes for `mode`.
pub fn env(mode: MeasurementMode) -> Vec<(&'static str, String)> {
    let mut env = vec![(RUNNER_MODE_ENV, mode.runner_mode_env_value().to_owned())];
    env.extend(python::env(mode));
    env.extend(java::env(mode));
    env
}
