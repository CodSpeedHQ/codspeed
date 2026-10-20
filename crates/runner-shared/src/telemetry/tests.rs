use super::*;

fn strings(attributes: &[(&str, &str)]) -> Attributes {
    attributes
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// The rows of the version 1 fixtures.
fn v1_spans() -> Vec<SpanRow> {
    let resource_attributes = strings(&[
        ("service.name", "example"),
        ("telemetry.sdk.language", "rust"),
    ]);
    vec![
        SpanRow {
            timestamp: 1_700_000_000_000_001_010,
            monotonic_start_ns: Some(1_010),
            monotonic_end_ns: Some(1_110),
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
            resource_attributes: resource_attributes.clone(),
            span_attributes: strings(&[("input_len", "7"), ("input_len", "8")]),
            events: vec![SpanEventV1 {
                timestamp: 1_700_000_000_000_001_050,
                name: "parsed".into(),
                attributes: strings(&[("valid", "true")]),
            }],
        },
        SpanRow {
            timestamp: 1_700_000_000_000_001_020,
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
            resource_attributes,
            span_attributes: Vec::new(),
            events: Vec::new(),
        },
    ]
}

fn v1_logs() -> Vec<LogRow> {
    vec![LogRow {
        timestamp: 1_700_000_000_000_001_060,
        monotonic_time_ns: Some(1_060),
        trace_id: "ab".repeat(16),
        span_id: "01".repeat(8),
        severity_text: "INFO".into(),
        severity_number: 9,
        service_name: "example".into(),
        event_name: "event benches/example.rs:9".into(),
        body: "parsed values".into(),
        scope_name: "example".into(),
        scope_version: String::new(),
        resource_attributes: strings(&[("service.name", "example")]),
        log_attributes: strings(&[("count", "3")]),
    }]
}

const V1_SPANS_FIXTURE: &str = "testdata/telemetry_spans_v1.msgpack.zst";
const V1_LOGS_FIXTURE: &str = "testdata/telemetry_logs_v1.msgpack.zst";

fn fixture(path: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn write<R: TelemetryRow + Clone>(path: &Path, batches: &[&[R]]) {
    let mut writer = TelemetryWriter::<R>::create(path).unwrap();
    for rows in batches {
        writer.write(rows.to_vec()).unwrap();
    }
    writer.close().unwrap();
}

fn read<R: TelemetryRow>(file: &[u8]) -> anyhow::Result<Vec<R>> {
    TelemetryReader::<R, _>::new(file)?.collect()
}

fn read_path<R: TelemetryRow>(path: &Path) -> Vec<R> {
    read(&std::fs::read(path).unwrap()).unwrap()
}

fn encode(record: &impl Serialize) -> Vec<u8> {
    rmp_serde::to_vec(record).unwrap()
}

fn file_with(records: &[Vec<u8>]) -> Vec<u8> {
    zstd::encode_all(records.concat().as_slice(), ZSTD_LEVEL).unwrap()
}

#[test]
fn reads_back_the_written_rows() {
    let dir = tempfile::tempdir().unwrap();
    let spans_path = dir.path().join(SPANS_FILE);
    let logs_path = dir.path().join(LOGS_FILE);
    let spans = v1_spans();

    write(&spans_path, &[&spans[..1], &[], &spans[1..]]);
    write(&logs_path, &[&v1_logs()]);

    assert_eq!(read_path::<SpanRow>(&spans_path), spans);
    assert_eq!(read_path::<LogRow>(&logs_path), v1_logs());
}

#[test]
fn reads_a_file_without_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(SPANS_FILE);

    write::<SpanRow>(&path, &[]);

    assert_eq!(read_path::<SpanRow>(&path), []);
}

#[test]
fn rejects_a_truncated_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(SPANS_FILE);
    write(&path, &[&v1_spans()]);
    let file = std::fs::read(&path).unwrap();

    for length in [0, file.len() / 2, file.len() - 1] {
        assert!(
            read::<SpanRow>(&file[..length]).is_err(),
            "{length} of {} bytes",
            file.len()
        );
    }
}

#[test]
fn reads_the_version_1_fixtures() {
    let read_fixture = |path| std::fs::read(fixture(path)).unwrap();

    assert_eq!(
        read::<SpanRow>(&read_fixture(V1_SPANS_FIXTURE)).unwrap(),
        v1_spans()
    );
    assert_eq!(
        read::<LogRow>(&read_fixture(V1_LOGS_FIXTURE)).unwrap(),
        v1_logs()
    );
}

/// Run with `cargo test -p runner-shared -- --ignored` when adding the fixtures of a new
/// version, after renaming the constants and rows above.
#[test]
#[ignore = "writes the fixtures of the current version"]
fn write_the_current_version_fixtures() {
    write(&fixture(V1_SPANS_FIXTURE), &[&v1_spans()]);
    write(&fixture(V1_LOGS_FIXTURE), &[&v1_logs()]);
}

/// What a newer runner writes once a version is added.
#[derive(Serialize)]
enum NewerSpanRecord {
    V1(Box<SpanRowV1>),
    V2(String),
}

#[test]
fn rejects_a_row_of_a_newer_version() {
    let file = file_with(&[
        encode(&NewerSpanRecord::V1(Box::new(v1_spans()[0].clone()))),
        encode(&NewerSpanRecord::V2("parse".into())),
        encode(&NewerSpanRecord::V1(Box::new(v1_spans()[1].clone()))),
    ]);

    let mut reader = TelemetryReader::<SpanRow, _>::new(file.as_slice()).unwrap();
    assert_eq!(reader.next().unwrap().unwrap(), v1_spans()[0]);
    let error = reader.next().unwrap().unwrap_err();
    assert_eq!(
        error.to_string(),
        "Failed to decode a row of the telemetry file"
    );
    assert!(reader.next().is_none());
}
