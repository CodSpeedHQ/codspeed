//! The OpenTelemetry spans and logs the benchmarks emitted, as artifacts in the profile folder:
//! [`SPANS_ARTIFACT`] and [`LOGS_ARTIFACT`].
//!
//! Each artifact is a zstd-compressed stream of MessagePack records, one per span or log. It is read
//! front to back, so it decodes as its bytes arrive: see [`TelemetryReader`].
//!
//! [`SpanRow`] and [`LogRow`] alias the current versions. Each row is stored wrapped in its
//! versioned record ([`SpanRecord`], [`LogRecord`]), so the rows of older versions still decode
//! and are upgraded through `From` impls. Fields are positional: a version's struct never changes
//! once released, and the fixtures of each version must keep decoding.
//!
//! # Version history
//!
//! - **V1** — Original format.
//!
//! When adding a version: add a `SpanRowV{N}` or `LogRowV{N}` struct, a `From<V{N-1}>` impl, a
//! record variant, update the alias and `from_record`, add the version's fixtures, and append an
//! entry above. A reader has to be deployed before the runners writing its version are released.

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::marker::PhantomData;
use std::path::Path;

use anyhow::Context;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub const SPANS_ARTIFACT: &str = "otel/spans.msgpack.zst";
pub const LOGS_ARTIFACT: &str = "otel/logs.msgpack.zst";

const ZSTD_LEVEL: i32 = 3;

pub type SpanRow = SpanRowV1;
pub type SpanEventRow = SpanEventV1;
pub type LogRow = LogRowV1;

#[derive(Debug, Serialize, Deserialize)]
pub enum SpanRecord {
    V1(SpanRowV1),
}

#[derive(Debug, Serialize, Deserialize)]
pub enum LogRecord {
    V1(LogRowV1),
}

/// A row type stored in its own telemetry artifact, as a versioned record.
pub trait TelemetryRow: Sized {
    type Record: Serialize + DeserializeOwned + From<Self>;

    /// Upgrades a record of any version to the current row.
    fn from_record(record: Self::Record) -> anyhow::Result<Self>;
}

impl From<SpanRow> for SpanRecord {
    fn from(row: SpanRow) -> Self {
        Self::V1(row)
    }
}

impl TelemetryRow for SpanRow {
    type Record = SpanRecord;

    fn from_record(record: SpanRecord) -> anyhow::Result<Self> {
        match record {
            SpanRecord::V1(row) => Ok(row),
        }
    }
}

impl From<LogRow> for LogRecord {
    fn from(row: LogRow) -> Self {
        Self::V1(row)
    }
}

impl TelemetryRow for LogRow {
    type Record = LogRecord;

    fn from_record(record: LogRecord) -> anyhow::Result<Self> {
        match record {
            LogRecord::V1(row) => Ok(row),
        }
    }
}

/// Attributes in their original order, which may repeat a key.
pub type Attributes = Vec<(String, String)>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpanRowV1 {
    /// Span start, in nanoseconds since the Unix epoch.
    pub timestamp: i64,
    /// Span start on the monotonic clock of the benchmark markers, in nanoseconds, `None` when the
    /// span didn't carry it.
    pub monotonic_start_ns: Option<u64>,
    /// Span end on the monotonic clock of the benchmark markers, in nanoseconds, `None` when the
    /// span didn't carry it.
    pub monotonic_end_ns: Option<u64>,
    /// The OTLP id bytes, read as a big-endian integer, so its `{:032x}` is the usual hex id.
    pub trace_id: u128,
    /// The OTLP id bytes, read as a big-endian integer, so its `{:016x}` is the usual hex id.
    pub span_id: u64,
    /// `None` for a root span.
    pub parent_span_id: Option<u64>,
    pub span_name: String,
    /// `Unspecified`, `Internal`, `Server`, `Client`, `Producer` or `Consumer`.
    pub span_kind: String,
    pub service_name: String,
    pub scope_name: String,
    pub scope_version: String,
    pub duration_ns: u64,
    /// `Unset`, `Ok` or `Error`.
    pub status_code: String,
    pub status_message: String,
    pub resource_attributes: Attributes,
    /// Without the monotonic start and end.
    pub span_attributes: Attributes,
    pub events: Vec<SpanEventV1>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpanEventV1 {
    /// Nanoseconds since the Unix epoch.
    pub timestamp: i64,
    pub name: String,
    pub attributes: Attributes,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogRowV1 {
    /// The log time, or the time it was observed when it has none, in nanoseconds since the
    /// Unix epoch.
    pub timestamp: i64,
    /// The log time on the monotonic clock of the benchmark markers, in nanoseconds, `None` when
    /// the log didn't carry it.
    pub monotonic_time_ns: Option<u64>,
    /// The ids of the span the log was emitted in, as in [`SpanRowV1`], `None` outside of a
    /// span.
    pub trace_id: Option<u128>,
    pub span_id: Option<u64>,
    pub severity_text: String,
    pub severity_number: u8,
    pub service_name: String,
    pub event_name: String,
    pub body: String,
    pub scope_name: String,
    pub scope_version: String,
    pub resource_attributes: Attributes,
    /// Without the monotonic time.
    pub log_attributes: Attributes,
}

/// Writes rows to a telemetry artifact, as records of the current version.
pub struct TelemetryWriter<R: TelemetryRow> {
    serializer: rmp_serde::Serializer<BufWriter<zstd::Encoder<'static, BufWriter<File>>>>,
    _rows: PhantomData<fn(R)>,
}

impl<R: TelemetryRow> TelemetryWriter<R> {
    pub fn create(path: &Path) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("Failed to create {}", dir.display()))?;
        }
        let file =
            File::create(path).with_context(|| format!("Failed to create {}", path.display()))?;
        let encoder = zstd::Encoder::new(BufWriter::new(file), ZSTD_LEVEL)?;
        Ok(Self {
            serializer: rmp_serde::Serializer::new(BufWriter::new(encoder)),
            _rows: PhantomData,
        })
    }

    pub fn write(&mut self, rows: impl IntoIterator<Item = R>) -> anyhow::Result<()> {
        for row in rows {
            R::Record::from(row).serialize(&mut self.serializer)?;
        }
        Ok(())
    }

    pub fn close(self) -> anyhow::Result<()> {
        let encoder = self
            .serializer
            .into_inner()
            .into_inner()
            .map_err(|error| error.into_error())?;
        encoder.finish()?.flush()?;
        Ok(())
    }
}

/// Reads the rows of a telemetry artifact as its bytes are read from `R`, such as an archive entry,
/// upgraded to the current version.
pub struct TelemetryReader<T: TelemetryRow, R: Read> {
    deserializer: rmp_serde::Deserializer<
        rmp_serde::decode::ReadReader<BufReader<zstd::Decoder<'static, BufReader<R>>>>,
    >,
    /// Set after an error, from which the stream can't recover.
    failed: bool,
    _rows: PhantomData<fn() -> T>,
}

impl<T: TelemetryRow, R: Read> TelemetryReader<T, R> {
    pub fn new(reader: R) -> anyhow::Result<Self> {
        let decoder = zstd::Decoder::new(reader)?;
        Ok(Self {
            // The deserializer reads a few bytes at a time.
            deserializer: rmp_serde::Deserializer::new(BufReader::new(decoder)),
            failed: false,
            _rows: PhantomData,
        })
    }
}

impl<T: TelemetryRow, R: Read> Iterator for TelemetryReader<T, R> {
    type Item = anyhow::Result<T>;

    /// Ends at the end of the artifact, or after the first error, such as an artifact cut in the middle
    /// of a row.
    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        let row = match self.deserializer.get_mut().fill_buf() {
            Ok([]) => return None,
            Ok(_) => T::Record::deserialize(&mut self.deserializer)
                .context("Failed to decode a row of the telemetry artifact")
                .and_then(T::from_record),
            Err(error) => {
                Err(anyhow::Error::from(error).context("Failed to read the telemetry artifact"))
            }
        };
        self.failed = row.is_err();
        Some(row)
    }
}

#[cfg(test)]
mod tests;
