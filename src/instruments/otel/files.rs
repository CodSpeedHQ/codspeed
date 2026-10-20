//! The received spans and logs, written as the telemetry files of the profile folder.

use std::path::Path;

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use runner_shared::telemetry::{LOGS_FILE, SPANS_FILE, TelemetryRow, TelemetryWriter};
use serde::de::DeserializeOwned;

use super::ReceivedRequests;
use super::receiver::{Encoding, Signal, StoredRequest};
use super::rows::{log_rows, span_rows};
use crate::prelude::*;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TelemetryStats {
    pub spans: u64,
    pub logs: u64,
    /// Spans and logs without their time on the monotonic clock of the benchmark markers.
    pub without_monotonic_time: u64,
    /// Requests that failed to decode.
    pub skipped_requests: u64,
}

/// Decodes `requests` into the [`SPANS_FILE`] and [`LOGS_FILE`] of `profile_folder`, then removes
/// them. A file is only written when it has rows.
pub async fn write_telemetry_files(
    requests: ReceivedRequests,
    profile_folder: &Path,
) -> Result<TelemetryStats> {
    let profile_folder = profile_folder.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let stats = write_files_blocking(&requests, &profile_folder)?;
        requests.remove()?;
        Ok(stats)
    })
    .await?
}

fn write_files_blocking(
    requests: &ReceivedRequests,
    profile_folder: &Path,
) -> Result<TelemetryStats> {
    let mut spans = LazyWriter::new(profile_folder.join(SPANS_FILE));
    let mut logs = LazyWriter::new(profile_folder.join(LOGS_FILE));
    let mut stats = TelemetryStats::default();

    for request in requests.list()? {
        let written = match request.signal {
            Signal::Traces => decode::<ExportTraceServiceRequest>(&request).map(|request| {
                let rows = span_rows(request);
                stats.spans += rows.len() as u64;
                stats.without_monotonic_time += rows
                    .iter()
                    .filter(|row| row.monotonic_start_ns.is_none())
                    .count() as u64;
                spans.write(rows)
            }),
            Signal::Logs => decode::<ExportLogsServiceRequest>(&request).map(|request| {
                let rows = log_rows(request);
                stats.logs += rows.len() as u64;
                stats.without_monotonic_time += rows
                    .iter()
                    .filter(|row| row.monotonic_time_ns.is_none())
                    .count() as u64;
                logs.write(rows)
            }),
        };
        match written {
            Ok(written) => written?,
            Err(error) => {
                warn!(
                    "Skipping the OTLP request {}: {error:#}",
                    request.path.display()
                );
                stats.skipped_requests += 1;
            }
        }
    }

    spans.close()?;
    logs.close()?;
    Ok(stats)
}

fn decode<M: Message + Default + DeserializeOwned>(request: &StoredRequest) -> Result<M> {
    let body = std::fs::read(&request.path)?;
    Ok(match request.encoding {
        Encoding::Protobuf => M::decode(body.as_slice())?,
        Encoding::Json => serde_json::from_slice(&body)?,
    })
}

/// Creates its file on the first rows.
struct LazyWriter<R: TelemetryRow> {
    path: std::path::PathBuf,
    writer: Option<TelemetryWriter<R>>,
}

impl<R: TelemetryRow> LazyWriter<R> {
    fn new(path: std::path::PathBuf) -> Self {
        Self { path, writer: None }
    }

    fn write(&mut self, rows: Vec<R>) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let writer = match &mut self.writer {
            Some(writer) => writer,
            None => self.writer.insert(TelemetryWriter::create(&self.path)?),
        };
        writer
            .write(rows)
            .with_context(|| format!("Failed to write {}", self.path.display()))
    }

    fn close(self) -> Result<()> {
        if let Some(writer) = self.writer {
            writer
                .close()
                .with_context(|| format!("Failed to write {}", self.path.display()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use runner_shared::telemetry::{LogRow, SpanRow, TelemetryReader};

    use super::super::OtelReceiver;
    use super::super::receiver::tests::post;
    use super::super::rows::tests::{START, logs_request, trace_request};
    use super::*;

    fn read<R: TelemetryRow>(path: &Path) -> Vec<R> {
        TelemetryReader::<R, _>::new(std::fs::File::open(path).unwrap())
            .unwrap()
            .collect::<anyhow::Result<_>>()
            .unwrap()
    }

    /// An OTLP/JSON request, with hex ids and 64-bit integers as strings.
    fn json_logs_request() -> String {
        let timestamp = START + 70;
        serde_json::json!({
            "resourceLogs": [{
                "resource": {"attributes": [
                    {"key": "service.name", "value": {"stringValue": "example"}},
                ]},
                "scopeLogs": [{
                    "scope": {"name": "example"},
                    "logRecords": [{
                        "timeUnixNano": timestamp.to_string(),
                        "severityNumber": 13,
                        "severityText": "WARN",
                        "body": {"stringValue": "skipped invalid fields"},
                        "attributes": [
                            {"key": "skipped", "value": {"intValue": "1"}},
                        ],
                        "traceId": "ab".repeat(16),
                        "spanId": "01".repeat(8),
                    }],
                }],
            }],
        })
        .to_string()
    }

    #[tokio::test]
    async fn writes_the_received_spans_and_logs() {
        let profile_folder = tempfile::tempdir().unwrap();
        let receiver = OtelReceiver::start(profile_folder.path()).await.unwrap();
        let protobuf = [("content-type", "application/x-protobuf")];
        let json = [("content-type", "application/json")];
        for (path, headers, body) in [
            ("/v1/traces", &protobuf, trace_request().encode_to_vec()),
            ("/v1/logs", &protobuf, logs_request().encode_to_vec()),
            ("/v1/logs", &json, json_logs_request().into_bytes()),
            ("/v1/traces", &protobuf, b"not protobuf".to_vec()),
        ] {
            let response = post(&receiver, path, headers, body).await;
            assert!(response.status().is_success());
        }

        let stats = write_telemetry_files(receiver.stop().await, profile_folder.path())
            .await
            .unwrap();

        assert_eq!(
            stats,
            TelemetryStats {
                spans: 2,
                logs: 2,
                without_monotonic_time: 2,
                skipped_requests: 1,
            }
        );
        assert!(!profile_folder.path().join("otel-requests").exists());
        assert_eq!(
            read::<SpanRow>(&profile_folder.path().join(SPANS_FILE)),
            span_rows(trace_request())
        );
        let logs = read::<LogRow>(&profile_folder.path().join(LOGS_FILE));
        assert_eq!(logs[..1], log_rows(logs_request()));
        assert_eq!(
            (
                logs[1].body.as_str(),
                logs[1].trace_id.as_str(),
                logs[1].timestamp,
                &logs[1].log_attributes,
            ),
            (
                "skipped invalid fields",
                "ab".repeat(16).as_str(),
                (START + 70) as i64,
                &vec![("skipped".to_string(), "1".to_string())],
            )
        );
    }

    #[tokio::test]
    async fn writes_nothing_without_requests() {
        let profile_folder = tempfile::tempdir().unwrap();
        let receiver = OtelReceiver::start(profile_folder.path()).await.unwrap();

        let stats = write_telemetry_files(receiver.stop().await, profile_folder.path())
            .await
            .unwrap();

        assert_eq!(stats, TelemetryStats::default());
        assert_eq!(std::fs::read_dir(profile_folder.path()).unwrap().count(), 0);
    }
}
