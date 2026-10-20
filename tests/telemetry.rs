//! End-to-end collection of the spans and logs of benchmarks built from the codspeed-rust
//! checkout, written as telemetry files that line up with the benchmark markers.
//!
//! Needs codspeed-rust checked out next to this repository. Run with
//! `cargo test --test telemetry -- --ignored`.
//!
//! Set `CODSPEED_OTEL_TEST_PROFILE_FOLDER` to a path to keep the profile folder there for
//! inspection instead of in a temporary directory. Its previous content is removed.
#![cfg(target_os = "linux")]

use std::collections::BTreeSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use runner_shared::artifacts::{ArtifactExt, ExecutionTimestamps};
use runner_shared::fifo::MarkerType;
use runner_shared::telemetry::{
    LOGS_FILE, LogRow, SPANS_FILE, SpanRow, TelemetryReader, TelemetryRow,
};

/// The example of codspeed-rust, whose benchmarks emit telemetry in different ways.
const EXAMPLE_PATH: &str = "crates/divan_compat/examples/benches/telemetry.rs";

fn codspeed_rust_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../codspeed-rust");
    assert!(
        dir.is_dir(),
        "codspeed-rust must be checked out at {}",
        dir.display()
    );
    dir
}

/// A command running in the codspeed-rust checkout. The toolchain and package variables that
/// `cargo test` sets would otherwise make its cargo build with this repository's toolchain.
fn codspeed_rust_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    cmd.current_dir(codspeed_rust_dir());
    for (name, _) in std::env::vars_os() {
        let name_str = name.to_string_lossy();
        let is_set_by_cargo = name_str == "RUSTUP_TOOLCHAIN"
            || name_str == "CARGO"
            || (name_str.starts_with("CARGO_") && name_str != "CARGO_HOME");
        if is_set_by_cargo {
            cmd.env_remove(&name);
        }
    }
    cmd
}

/// The profile folder to run the benchmark with: `CODSPEED_OTEL_TEST_PROFILE_FOLDER` when set,
/// emptied first and kept after the test, a temporary directory otherwise.
fn profile_folder() -> (PathBuf, Option<tempfile::TempDir>) {
    match std::env::var_os("CODSPEED_OTEL_TEST_PROFILE_FOLDER") {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            if dir.exists() {
                std::fs::remove_dir_all(&dir).unwrap();
            }
            std::fs::create_dir_all(&dir).unwrap();
            (dir, None)
        }
        None => {
            let temp_dir = tempfile::tempdir().unwrap();
            (temp_dir.path().to_path_buf(), Some(temp_dir))
        }
    }
}

#[test]
#[ignore = "needs codspeed-rust checked out in ../codspeed-rust"]
fn collects_the_spans_and_logs_of_the_divan_telemetry_example() {
    let cargo_codspeed = "cargo run -q -p cargo-codspeed -- codspeed";
    let bench_args = "-p codspeed-divan-compat-examples --bench telemetry";

    codspeed_rust_command("bash")
        .args([
            "-c",
            &format!("{cargo_codspeed} build -m telemetry {bench_args}"),
        ])
        .assert()
        .success();

    let (profile_folder, _temp_dir) = profile_folder();
    let run_start = unix_nanos_now();
    codspeed_rust_command(env!("CARGO_BIN_EXE_codspeed"))
        // Forces the local provider; the GitHub Actions one needs a workflow event.
        .env_remove("GITHUB_ACTIONS")
        // Isolation needs sudo.
        .env("CODSPEED_ISOLATION", "false")
        .args(["run", "--skip-upload", "--mode", "telemetry"])
        .arg("--profile-folder")
        .arg(&profile_folder)
        // `cargo codspeed run` picks the telemetry mode from `CODSPEED_RUNNER_MODE`.
        .args(["--", &format!("{cargo_codspeed} run {bench_args}")])
        .assert()
        .success();
    let run_end = unix_nanos_now();

    assert!(!profile_folder.join("otel-requests").exists());
    let spans: Vec<SpanRow> = read(&profile_folder.join(SPANS_FILE));
    let logs: Vec<LogRow> = read(&profile_folder.join(LOGS_FILE));
    let execution_timestamps = ExecutionTimestamps::decode_from_reader(
        std::fs::File::open(profile_folder.join(ExecutionTimestamps::file_name(None))).unwrap(),
    )
    .unwrap();
    let regions = benchmark_regions(&execution_timestamps);
    for benchmark in [
        "parse_and_sum",
        "event_without_span",
        "log_records",
        "no_telemetry",
    ] {
        assert!(
            regions
                .iter()
                .any(|(region_uri, _)| *region_uri == uri(benchmark)),
            "{benchmark} did not run: {regions:?}"
        );
    }
    // The benchmark whose rounds a time on the monotonic clock falls within.
    let benchmark_at = |monotonic_time: Option<u64>| {
        let monotonic_time = monotonic_time?;
        regions
            .iter()
            .find(|(_, range)| range.contains(&monotonic_time))
            .map(|(uri, _)| uri.clone())
    };
    let span_benchmark = |span: &SpanRow| benchmark_at(span.monotonic_start_ns);
    let log_benchmark = |log: &LogRow| benchmark_at(log.monotonic_time_ns);
    let log = |body: &str| {
        logs.iter().find(|log| log.body == body).unwrap_or_else(|| {
            let bodies = logs.iter().map(|log| &log.body).collect::<Vec<_>>();
            panic!("no `{body}` log in {bodies:?}")
        })
    };

    // Timestamps are wall clock times.
    let timestamps = spans
        .iter()
        .map(|span| span.timestamp)
        .chain(logs.iter().map(|log| log.timestamp));
    for timestamp in timestamps {
        assert!(
            (run_start..=run_end).contains(&timestamp),
            "{timestamp} is outside of the run, from {run_start} to {run_end}"
        );
    }

    // Every span and log happens within the rounds of a benchmark.
    for span in &spans {
        let end = span.monotonic_end_ns;
        assert!(
            span.monotonic_start_ns <= end,
            "span {} ends before it starts",
            span.span_name
        );
        assert!(
            span_benchmark(span).is_some() && span_benchmark(span) == benchmark_at(end),
            "span {} runs outside of the rounds of a benchmark",
            span.span_name
        );
    }
    for log in &logs {
        assert!(
            log_benchmark(log).is_some(),
            "log `{}` is outside of the rounds of a benchmark",
            log.body
        );
    }

    // Nested `tracing` spans, with events in them.
    let parse_and_sum_spans = spans
        .iter()
        .filter(|span| span_benchmark(span) == Some(uri("parse_and_sum")))
        .map(|span| span.span_name.as_str())
        .collect::<BTreeSet<_>>();
    assert!(
        ["parse_and_sum", "parse", "sum"]
            .iter()
            .all(|name| parse_and_sum_spans.contains(name)),
        "missing spans of parse_and_sum in {parse_and_sum_spans:?}"
    );
    assert!(!parse_and_sum_spans.contains("sum_inner"));
    let skipped = log("skipped invalid fields");
    assert_eq!(log_benchmark(skipped), Some(uri("parse_and_sum")));
    let skipped_span = spans
        .iter()
        .find(|span| span.span_id == skipped.span_id)
        .expect("the `skipped invalid fields` log is in no span");
    assert_eq!(skipped_span.span_name, "parse_and_sum");
    assert_eq!(skipped_span.trace_id, skipped.trace_id);

    // A `tracing` event outside of any span.
    let event = log("summed without a span");
    assert_eq!(log_benchmark(event), Some(uri("event_without_span")));
    assert_eq!(event.severity_text, "INFO");
    assert_eq!(event.trace_id, "");

    // Records of the `log` crate.
    let record = log("summing 3 values, logged with the log crate");
    assert_eq!(log_benchmark(record), Some(uri("log_records")));
    assert_eq!(record.severity_text, "INFO");
    assert_eq!(record.severity_number, 9);
    let example = std::fs::read_to_string(codspeed_rust_dir().join(EXAMPLE_PATH)).unwrap();
    let line = example
        .lines()
        .position(|line| line.contains("logged with the log crate"))
        .unwrap()
        + 1;
    let attribute = |key: &str| {
        record
            .log_attributes
            .iter()
            .find(|(attribute_key, _)| attribute_key == key)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(attribute("code.filepath"), Some(EXAMPLE_PATH));
    assert_eq!(attribute("code.lineno"), Some(line.to_string().as_str()));

    // Records below `info` are filtered out, whether they come from `tracing` or `log`.
    assert!(
        logs.iter()
            .all(|log| log.body != "filtered out by the default filter")
    );

    // No telemetry at all.
    assert!(
        spans
            .iter()
            .all(|span| span_benchmark(span) != Some(uri("no_telemetry")))
    );
    assert!(
        logs.iter()
            .all(|log| log_benchmark(log) != Some(uri("no_telemetry")))
    );
}

fn unix_nanos_now() -> i64 {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    i64::try_from(since_epoch.as_nanos()).unwrap()
}

fn uri(benchmark: &str) -> String {
    format!("{EXAMPLE_PATH}::{benchmark}")
}

fn read<R: TelemetryRow>(path: &Path) -> Vec<R> {
    TelemetryReader::<R, _>::new(std::fs::File::open(path).unwrap())
        .unwrap()
        .collect::<anyhow::Result<_>>()
        .unwrap()
}

/// Each benchmark with the time its rounds took, from the first round start to the last round
/// end of its sample, on the clock of the markers.
fn benchmark_regions(timestamps: &ExecutionTimestamps) -> Vec<(String, Range<u64>)> {
    let mut regions = Vec::new();
    let (mut round_start, mut rounds) = (None, None::<Range<u64>>);
    let mut markers = timestamps.markers.clone();
    markers.sort_by_key(|marker| match *marker {
        MarkerType::SampleStart(timestamp)
        | MarkerType::SampleEnd(timestamp)
        | MarkerType::RoundStart(timestamp)
        | MarkerType::RoundEnd(timestamp) => timestamp,
    });
    for marker in markers {
        match marker {
            MarkerType::SampleStart(_) => rounds = None,
            MarkerType::RoundStart(start) => round_start = Some(start),
            MarkerType::RoundEnd(end) => {
                let start = round_start.take().expect("a round ends before it starts");
                let first_start = rounds.as_ref().map_or(start, |rounds| rounds.start);
                rounds = Some(first_start..end);
            }
            MarkerType::SampleEnd(_) => {
                regions.push(rounds.take().expect("a sample has no complete round"));
            }
        }
    }
    assert_eq!(regions.len(), timestamps.uri_by_ts.len());
    timestamps
        .uri_by_ts
        .iter()
        .map(|(_, uri)| uri.clone())
        .zip(regions)
        .collect()
}
