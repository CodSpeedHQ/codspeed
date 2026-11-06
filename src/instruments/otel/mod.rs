//! Collection of the OpenTelemetry spans and logs the benchmarks export.
//!
//! The [receiver](receiver::OtelReceiver) stores the OTLP/HTTP requests the benchmarks send while
//! they run. Once they exit, the requests are decoded and [written](artifacts) as the
//! [`runner_shared::telemetry`] artifacts.

mod artifacts;
mod receiver;
mod rows;

pub use artifacts::write_telemetry_artifacts;
pub use receiver::{OTEL_ENDPOINT_ENV, OtelReceiver, ReceivedRequests};
