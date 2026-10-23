//! Flattening of the OTLP export requests into one row per span or log.
//!
//! Values are formatted like the OpenTelemetry Collector's ClickHouse exporter does, so that
//! these rows can be queried alongside its tables.

use base64::Engine;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue, any_value};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::span::SpanKind;
use opentelemetry_proto::tonic::trace::v1::status::StatusCode;

use runner_shared::telemetry::{Attributes, LogRow, SpanEventRow, SpanRow};

const SERVICE_NAME_ATTRIBUTE: &str = "service.name";

/// Span attribute holding the span start on the monotonic clock of the benchmark markers.
pub const MONOTONIC_START_ATTRIBUTE: &str = "codspeed.monotonic_start_ns";

/// Span attribute holding the span end on the monotonic clock of the benchmark markers.
pub const MONOTONIC_END_ATTRIBUTE: &str = "codspeed.monotonic_end_ns";

/// Log attribute holding the log time on the monotonic clock of the benchmark markers.
pub const MONOTONIC_TIME_ATTRIBUTE: &str = "codspeed.monotonic_time_ns";

pub fn span_rows(request: ExportTraceServiceRequest) -> Vec<SpanRow> {
    let mut rows = Vec::new();
    for resource_spans in request.resource_spans {
        let resource = ResourceContext::new(resource_spans.resource);
        for scope_spans in resource_spans.scope_spans {
            let (scope_name, scope_version) = scope_name_and_version(scope_spans.scope);
            for mut span in scope_spans.spans {
                let monotonic_start_ns =
                    take_monotonic_time(&mut span.attributes, MONOTONIC_START_ATTRIBUTE);
                let monotonic_end_ns =
                    take_monotonic_time(&mut span.attributes, MONOTONIC_END_ATTRIBUTE);
                rows.push(SpanRow {
                    timestamp: unix_nanos(span.start_time_unix_nano),
                    monotonic_start_ns,
                    monotonic_end_ns,
                    trace_id: hex_id(&span.trace_id),
                    span_id: hex_id(&span.span_id),
                    parent_span_id: hex_id(&span.parent_span_id),
                    span_name: span.name,
                    span_kind: span_kind(span.kind).into(),
                    service_name: resource.service_name.clone(),
                    scope_name: scope_name.clone(),
                    scope_version: scope_version.clone(),
                    duration_ns: span
                        .end_time_unix_nano
                        .saturating_sub(span.start_time_unix_nano),
                    status_code: status_code(span.status.as_ref().map_or(0, |status| status.code))
                        .into(),
                    status_message: span.status.map(|status| status.message).unwrap_or_default(),
                    resource_attributes: resource.attributes.clone(),
                    span_attributes: attributes(span.attributes),
                    events: span
                        .events
                        .into_iter()
                        .map(|event| SpanEventRow {
                            timestamp: unix_nanos(event.time_unix_nano),
                            name: event.name,
                            attributes: attributes(event.attributes),
                        })
                        .collect(),
                });
            }
        }
    }
    rows
}

pub fn log_rows(request: ExportLogsServiceRequest) -> Vec<LogRow> {
    let mut rows = Vec::new();
    for resource_logs in request.resource_logs {
        let resource = ResourceContext::new(resource_logs.resource);
        for scope_logs in resource_logs.scope_logs {
            let (scope_name, scope_version) = scope_name_and_version(scope_logs.scope);
            for mut record in scope_logs.log_records {
                let timestamp = match record.time_unix_nano {
                    0 => record.observed_time_unix_nano,
                    timestamp => timestamp,
                };
                let monotonic_time_ns =
                    take_monotonic_time(&mut record.attributes, MONOTONIC_TIME_ATTRIBUTE);
                rows.push(LogRow {
                    timestamp: unix_nanos(timestamp),
                    monotonic_time_ns,
                    trace_id: hex_id(&record.trace_id),
                    span_id: hex_id(&record.span_id),
                    severity_text: record.severity_text,
                    severity_number: u8::try_from(record.severity_number).unwrap_or_default(),
                    service_name: resource.service_name.clone(),
                    event_name: record.event_name,
                    body: record.body.map(any_value_string).unwrap_or_default(),
                    scope_name: scope_name.clone(),
                    scope_version: scope_version.clone(),
                    resource_attributes: resource.attributes.clone(),
                    log_attributes: attributes(record.attributes),
                });
            }
        }
    }
    rows
}

/// What the rows of a resource share.
struct ResourceContext {
    service_name: String,
    attributes: Attributes,
}

impl ResourceContext {
    fn new(resource: Option<Resource>) -> Self {
        let attributes = attributes(
            resource
                .map(|resource| resource.attributes)
                .unwrap_or_default(),
        );
        let service_name = attributes
            .iter()
            .find(|(key, _)| key == SERVICE_NAME_ATTRIBUTE)
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        Self {
            service_name,
            attributes,
        }
    }
}

fn scope_name_and_version(scope: Option<InstrumentationScope>) -> (String, String) {
    scope
        .map(|scope| (scope.name, scope.version))
        .unwrap_or_default()
}

/// Moves the monotonic time attribute `key` out of `attributes`. An attribute that isn't a
/// non-negative integer stays.
fn take_monotonic_time(attributes: &mut Vec<KeyValue>, key: &str) -> Option<u64> {
    let index = attributes
        .iter()
        .position(|attribute| attribute.key == key)?;
    let time = match attributes[index].value.as_ref()?.value {
        Some(any_value::Value::IntValue(time)) => u64::try_from(time).ok()?,
        _ => return None,
    };
    attributes.remove(index);
    Some(time)
}

fn attributes(attributes: Vec<KeyValue>) -> Attributes {
    attributes
        .into_iter()
        .map(|attribute| {
            let value = attribute.value.map(any_value_string).unwrap_or_default();
            (attribute.key, value)
        })
        .collect()
}

/// A timestamp past the range of the timestamp columns is clamped to it.
fn unix_nanos(timestamp: u64) -> i64 {
    i64::try_from(timestamp).unwrap_or(i64::MAX)
}

/// Lowercase hex, or empty for an empty or all-zero (invalid) id.
fn hex_id(id: &[u8]) -> String {
    if id.iter().all(|byte| *byte == 0) {
        return String::new();
    }
    id.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn span_kind(kind: i32) -> &'static str {
    match SpanKind::try_from(kind).unwrap_or(SpanKind::Unspecified) {
        SpanKind::Unspecified => "Unspecified",
        SpanKind::Internal => "Internal",
        SpanKind::Server => "Server",
        SpanKind::Client => "Client",
        SpanKind::Producer => "Producer",
        SpanKind::Consumer => "Consumer",
    }
}

fn status_code(code: i32) -> &'static str {
    match StatusCode::try_from(code).unwrap_or(StatusCode::Unset) {
        StatusCode::Unset => "Unset",
        StatusCode::Ok => "Ok",
        StatusCode::Error => "Error",
    }
}

/// Strings as is, and other scalars as text. Bytes are base64-encoded, and arrays and maps are
/// JSON.
fn any_value_string(value: AnyValue) -> String {
    match value.value {
        // An index into a string table that OTLP traces and logs don't carry.
        None | Some(any_value::Value::StringValueStrindex(_)) => String::new(),
        Some(any_value::Value::StringValue(value)) => value,
        Some(any_value::Value::BoolValue(value)) => value.to_string(),
        Some(any_value::Value::IntValue(value)) => value.to_string(),
        Some(any_value::Value::DoubleValue(value)) => value.to_string(),
        Some(any_value::Value::BytesValue(value)) => {
            base64::engine::general_purpose::STANDARD.encode(value)
        }
        Some(value @ (any_value::Value::ArrayValue(_) | any_value::Value::KvlistValue(_))) => {
            any_value_json(Some(value)).to_string()
        }
    }
}

fn any_value_json(value: Option<any_value::Value>) -> serde_json::Value {
    use serde_json::Value as Json;
    match value {
        None | Some(any_value::Value::StringValueStrindex(_)) => Json::Null,
        Some(any_value::Value::StringValue(value)) => Json::String(value),
        Some(any_value::Value::BoolValue(value)) => Json::Bool(value),
        Some(any_value::Value::IntValue(value)) => Json::from(value),
        Some(any_value::Value::DoubleValue(value)) => Json::from(value),
        Some(any_value::Value::BytesValue(value)) => {
            Json::String(base64::engine::general_purpose::STANDARD.encode(value))
        }
        Some(any_value::Value::ArrayValue(array)) => Json::Array(
            array
                .values
                .into_iter()
                .map(|value| any_value_json(value.value))
                .collect(),
        ),
        Some(any_value::Value::KvlistValue(list)) => Json::Object(
            list.values
                .into_iter()
                .map(|attribute| {
                    let value = attribute.value.and_then(|value| value.value);
                    (attribute.key, any_value_json(value))
                })
                .collect(),
        ),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use opentelemetry_proto::tonic::common::v1::{ArrayValue, KeyValueList};
    use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use opentelemetry_proto::tonic::trace::v1::span::Event;
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, Status};

    use super::*;

    /// The time the requests are relative to.
    pub const START: u64 = 1_700_000_000_000_001_000;
    /// [`START`] on the monotonic clock.
    pub const MONOTONIC_START: u64 = 1_000;

    pub fn string_value(value: &str) -> AnyValue {
        AnyValue {
            value: Some(any_value::Value::StringValue(value.into())),
        }
    }

    pub fn key_value(key: &str, value: AnyValue) -> KeyValue {
        KeyValue {
            key: key.into(),
            value: Some(value),
            ..Default::default()
        }
    }

    pub fn resource() -> Option<Resource> {
        Some(Resource {
            attributes: vec![key_value("service.name", string_value("example"))],
            ..Default::default()
        })
    }

    fn scope() -> Option<InstrumentationScope> {
        Some(InstrumentationScope {
            name: "codspeed".into(),
            version: "1.0.0".into(),
            ..Default::default()
        })
    }

    pub fn trace_request() -> ExportTraceServiceRequest {
        let root = Span {
            trace_id: vec![0xab; 16],
            span_id: vec![0x01; 8],
            name: "parse".into(),
            kind: SpanKind::Internal as i32,
            start_time_unix_nano: START + 10,
            end_time_unix_nano: START + 110,
            attributes: vec![
                key_value(
                    "input_len",
                    AnyValue {
                        value: Some(any_value::Value::IntValue(7)),
                    },
                ),
                key_value(
                    MONOTONIC_START_ATTRIBUTE,
                    AnyValue {
                        value: Some(any_value::Value::IntValue(MONOTONIC_START as i64 + 10)),
                    },
                ),
                key_value(
                    MONOTONIC_END_ATTRIBUTE,
                    AnyValue {
                        value: Some(any_value::Value::IntValue(MONOTONIC_START as i64 + 110)),
                    },
                ),
            ],
            events: vec![Event {
                time_unix_nano: START + 50,
                name: "parsed".into(),
                attributes: vec![key_value(
                    "valid",
                    AnyValue {
                        value: Some(any_value::Value::BoolValue(true)),
                    },
                )],
                ..Default::default()
            }],
            status: Some(Status {
                message: "bad input".into(),
                code: StatusCode::Error as i32,
            }),
            ..Default::default()
        };
        let child = Span {
            trace_id: vec![0xab; 16],
            span_id: vec![0x02; 8],
            parent_span_id: vec![0x01; 8],
            name: "tokenize".into(),
            start_time_unix_nano: START + 20,
            end_time_unix_nano: START + 30,
            ..Default::default()
        };
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: resource(),
                scope_spans: vec![ScopeSpans {
                    scope: scope(),
                    spans: vec![root, child],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
    }

    pub fn logs_request() -> ExportLogsServiceRequest {
        let record = LogRecord {
            observed_time_unix_nano: START + 60,
            severity_number: 9,
            severity_text: "INFO".into(),
            event_name: "event benches/example.rs:9".into(),
            body: Some(string_value("parsed values")),
            attributes: vec![
                key_value(
                    "count",
                    AnyValue {
                        value: Some(any_value::Value::IntValue(3)),
                    },
                ),
                key_value(
                    MONOTONIC_TIME_ATTRIBUTE,
                    AnyValue {
                        value: Some(any_value::Value::IntValue(MONOTONIC_START as i64 + 60)),
                    },
                ),
            ],
            trace_id: vec![0xab; 16],
            span_id: vec![0x01; 8],
            ..Default::default()
        };
        ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: resource(),
                scope_logs: vec![ScopeLogs {
                    scope: scope(),
                    log_records: vec![record],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
    }

    fn strings(attributes: &[(&str, &str)]) -> Attributes {
        attributes
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    fn expected_resource_attributes() -> Attributes {
        strings(&[("service.name", "example")])
    }

    #[test]
    fn flattens_the_spans() {
        let rows = span_rows(trace_request());

        let timestamp = START as i64;
        assert_eq!(
            rows,
            vec![
                SpanRow {
                    timestamp: timestamp + 10,
                    monotonic_start_ns: Some(MONOTONIC_START + 10),
                    monotonic_end_ns: Some(MONOTONIC_START + 110),
                    trace_id: "ab".repeat(16),
                    span_id: "01".repeat(8),
                    parent_span_id: String::new(),
                    span_name: "parse".into(),
                    span_kind: "Internal".into(),
                    service_name: "example".into(),
                    scope_name: "codspeed".into(),
                    scope_version: "1.0.0".into(),
                    duration_ns: 100,
                    status_code: "Error".into(),
                    status_message: "bad input".into(),
                    resource_attributes: expected_resource_attributes(),
                    span_attributes: strings(&[("input_len", "7")]),
                    events: vec![SpanEventRow {
                        timestamp: timestamp + 50,
                        name: "parsed".into(),
                        attributes: strings(&[("valid", "true")]),
                    }],
                },
                SpanRow {
                    timestamp: timestamp + 20,
                    monotonic_start_ns: None,
                    monotonic_end_ns: None,
                    trace_id: "ab".repeat(16),
                    span_id: "02".repeat(8),
                    parent_span_id: "01".repeat(8),
                    span_name: "tokenize".into(),
                    span_kind: "Unspecified".into(),
                    service_name: "example".into(),
                    scope_name: "codspeed".into(),
                    scope_version: "1.0.0".into(),
                    duration_ns: 10,
                    status_code: "Unset".into(),
                    status_message: String::new(),
                    resource_attributes: expected_resource_attributes(),
                    span_attributes: Vec::new(),
                    events: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn flattens_the_logs_timestamped_when_observed_without_a_time() {
        let rows = log_rows(logs_request());

        assert_eq!(
            rows,
            vec![LogRow {
                timestamp: (START + 60) as i64,
                monotonic_time_ns: Some(MONOTONIC_START + 60),
                trace_id: "ab".repeat(16),
                span_id: "01".repeat(8),
                severity_text: "INFO".into(),
                severity_number: 9,
                service_name: "example".into(),
                event_name: "event benches/example.rs:9".into(),
                body: "parsed values".into(),
                scope_name: "codspeed".into(),
                scope_version: "1.0.0".into(),
                resource_attributes: expected_resource_attributes(),
                log_attributes: strings(&[("count", "3")]),
            }]
        );
    }

    #[test]
    fn has_no_service_name_without_a_resource() {
        let mut request = trace_request();
        request.resource_spans[0].resource = None;

        let rows = span_rows(request);

        assert_eq!(rows[0].service_name, "");
        assert_eq!(rows[0].resource_attributes, []);
    }

    #[test]
    fn formats_the_attribute_values_as_text() {
        let value = |value| AnyValue { value: Some(value) };
        let cases = [
            (value(any_value::Value::DoubleValue(1.5)), "1.5"),
            (value(any_value::Value::DoubleValue(2.0)), "2"),
            (value(any_value::Value::BytesValue(b"hi".to_vec())), "aGk="),
            (
                value(any_value::Value::ArrayValue(ArrayValue {
                    values: vec![string_value("a"), value(any_value::Value::IntValue(1))],
                })),
                r#"["a",1]"#,
            ),
            (
                value(any_value::Value::KvlistValue(KeyValueList {
                    values: vec![key_value("key", string_value("value"))],
                })),
                r#"{"key":"value"}"#,
            ),
            (AnyValue { value: None }, ""),
        ];
        for (value, expected) in cases {
            assert_eq!(any_value_string(value.clone()), expected, "{value:?}");
        }
    }
}
