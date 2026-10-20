use super::AnalysisInstrument;
use crate::instruments::otel::{OTEL_ENDPOINT_ENV, OtelReceiver, write_telemetry_files};
use crate::prelude::*;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Receives the OpenTelemetry data the benchmarks export, through an OTLP/HTTP receiver whose
/// URL is passed in [`OTEL_ENDPOINT_ENV`], and writes its spans and logs as telemetry files in the
/// profile folder.
#[derive(Default)]
pub struct TelemetryInstrument {
    started: Option<(OtelReceiver, PathBuf)>,
}

#[async_trait(?Send)]
impl AnalysisInstrument for TelemetryInstrument {
    async fn start(
        &mut self,
        profile_folder: &Path,
        env: &mut HashMap<String, String>,
    ) -> Result<()> {
        let receiver = OtelReceiver::start(profile_folder).await?;
        env.insert(OTEL_ENDPOINT_ENV.into(), receiver.endpoint().into());
        self.started = Some((receiver, profile_folder.to_path_buf()));
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        let Some((receiver, profile_folder)) = self.started.take() else {
            return Ok(());
        };
        let requests = receiver.stop().await;
        let (request_count, received_bytes) = (requests.count, requests.received_bytes);
        let stats = write_telemetry_files(requests, &profile_folder)
            .await
            .context("Failed to write the OpenTelemetry data")?;

        if request_count == 0 {
            warn!(
                "The benchmarks exported no OpenTelemetry data. Make sure their integration \
                 exports it, e.g. with the `telemetry` feature of `codspeed-divan-compat`."
            );
        } else {
            info!(
                "Collected {} spans and {} logs from {request_count} OpenTelemetry export \
                 requests ({received_bytes} bytes)",
                stats.spans, stats.logs
            );
        }
        if stats.without_monotonic_time > 0 {
            warn!(
                "{} spans and logs carry no time on the clock of the benchmarks, they won't be \
                 attributed to their benchmark. Update their integration.",
                stats.without_monotonic_time
            );
        }
        Ok(())
    }
}
