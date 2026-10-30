//! Collection of the OpenTelemetry spans and logs the benchmarks export.
//!
//! The [receiver](receiver::OtelReceiver) stores the OTLP/HTTP requests the benchmarks send while
//! they run. Once they exit, the requests are decoded and [written](files) as the
//! [`runner_shared::telemetry`] files.

mod files;
mod receiver;
mod rows;

pub use files::write_telemetry_files;
pub use receiver::{OTEL_ENDPOINT_ENV, OtelReceiver, ReceivedRequests};
