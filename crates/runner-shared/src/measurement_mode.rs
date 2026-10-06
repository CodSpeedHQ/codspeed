use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(ValueEnum, Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MeasurementMode {
    Walltime,
    Memory,
    #[value(alias = "instrumentation")]
    Simulation,
}

impl MeasurementMode {
    /// Value injected as `CODSPEED_RUNNER_MODE`, read by the integrations and the node wrapper.
    pub fn runner_mode_env_value(self) -> &'static str {
        match self {
            // Integrations older than the simulation rename only accept `instrumentation`.
            // TODO: switch to "simulation" in the next major release.
            Self::Simulation => "instrumentation",
            Self::Walltime => "walltime",
            Self::Memory => "memory",
        }
    }
}
