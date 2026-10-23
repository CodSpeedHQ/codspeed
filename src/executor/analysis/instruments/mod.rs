//! Instruments hooked onto the [analysis executor](super::executor::AnalysisExecutor).

use crate::instruments::Instruments;
use crate::prelude::*;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;

pub mod telemetry;

/// Collects data while the benchmarks run in analysis mode.
#[async_trait(?Send)]
pub trait AnalysisInstrument {
    /// Called before the benchmark process is spawned, to set up the instrument and the
    /// environment it needs in the benchmark process.
    async fn start(
        &mut self,
        profile_folder: &Path,
        env: &mut HashMap<String, String>,
    ) -> Result<()>;

    /// Called when the integration starts measuring a benchmark.
    async fn on_start_profiler(&mut self) -> Result<()> {
        Ok(())
    }

    /// Called when the integration stops measuring a benchmark.
    async fn on_stop_profiler(&mut self) -> Result<()> {
        Ok(())
    }

    /// Called once the benchmark process has exited, successfully or not.
    async fn stop(&mut self) -> Result<()>;
}

/// The analysis instruments enabled in `instruments`.
pub fn enabled_instruments(instruments: &Instruments) -> Vec<Box<dyn AnalysisInstrument>> {
    let mut enabled: Vec<Box<dyn AnalysisInstrument>> = Vec::new();
    if instruments.telemetry {
        enabled.push(Box::new(telemetry::TelemetryInstrument::default()));
    }
    enabled
}
