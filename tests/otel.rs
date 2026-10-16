//! End-to-end OTLP collection of the spans and logs of a benchmark built from the codspeed-rust
//! checkout, timestamped on the clock of the benchmark markers.
//!
//! Needs codspeed-rust checked out next to this repository. Run with
//! `cargo test --test otel -- --ignored`.
//!
//! Set `CODSPEED_OTEL_TEST_PROFILE_FOLDER` to a path to keep the profile folder there for
//! inspection instead of in a temporary directory. Its previous content is removed.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use async_compression::tokio::bufread::ZstdDecoder;
use futures::StreamExt;
use instrument_hooks_bindings::MONOTONIC_OFFSET_ATTRIBUTE;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;
use runner_shared::artifacts::{ArtifactExt, ExecutionTimestamps};
use runner_shared::fifo::MarkerType;
use tokio::io::AsyncReadExt;

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

/// Returns the name and content of each entry of the zstd-compressed tar at `path`.
async fn archive_entries(path: &Path) -> Vec<(String, Vec<u8>)> {
    let compressed = std::fs::read(path).unwrap();
    let mut archive = tokio_tar::Archive::new(ZstdDecoder::new(compressed.as_slice()));
    let mut entries = archive.entries().unwrap();
    let mut result = Vec::new();
    while let Some(entry) = entries.next().await {
        let mut entry = entry.unwrap();
        let name = entry.path().unwrap().to_string_lossy().into_owned();
        let mut content = Vec::new();
        entry.read_to_end(&mut content).await.unwrap();
        result.push((name, content));
    }
    result
}

#[tokio::test]
#[ignore = "needs codspeed-rust checked out in ../codspeed-rust"]
async fn collects_the_spans_and_logs_of_the_divan_otel_example() {
    let cargo_codspeed = "cargo run -q -p cargo-codspeed -- codspeed";
    let bench_args = "-p codspeed-divan-compat --bench otel_example";

    codspeed_rust_command("bash")
        .args([
            "-c",
            &format!("{cargo_codspeed} build -m walltime {bench_args} --features otel"),
        ])
        .assert()
        .success();

    let (profile_folder, _temp_dir) = profile_folder();
    codspeed_rust_command(env!("CARGO_BIN_EXE_codspeed"))
        // Forces the local provider; the GitHub Actions one needs a workflow event.
        .env_remove("GITHUB_ACTIONS")
        // Isolation needs sudo.
        .env("CODSPEED_ISOLATION", "false")
        .args([
            "run",
            "--skip-upload",
            "--mode",
            "walltime",
            "--experimental-otel",
        ])
        .arg("--profile-folder")
        .arg(&profile_folder)
        .args(["--", &format!("{cargo_codspeed} run {bench_args}")])
        .assert()
        .success();

    assert!(!profile_folder.join("otel").exists());
    let entries = archive_entries(&profile_folder.join("otel.tar.zst")).await;
    let traces: Vec<ExportTraceServiceRequest> = decode_requests(&entries, "traces-");
    let logs: Vec<ExportLogsServiceRequest> = decode_requests(&entries, "logs-");
    assert!(!traces.is_empty(), "no trace request in {entries:?}");
    assert!(!logs.is_empty(), "no log request in {entries:?}");

    // (timestamp, offset to the monotonic clock) of each span start and end and log record.
    let mut timestamps = Vec::new();
    let mut span_names = Vec::new();
    for resource_spans in traces.iter().flat_map(|request| &request.resource_spans) {
        let offset = monotonic_offset(resource_spans.resource.as_ref());
        for span in resource_spans
            .scope_spans
            .iter()
            .flat_map(|scope| &scope.spans)
        {
            span_names.push(span.name.as_str());
            timestamps.push((span.start_time_unix_nano, offset));
            timestamps.push((span.end_time_unix_nano, offset));
        }
    }
    let mut log_bodies = Vec::new();
    for resource_logs in logs.iter().flat_map(|request| &request.resource_logs) {
        let offset = monotonic_offset(resource_logs.resource.as_ref());
        for record in resource_logs
            .scope_logs
            .iter()
            .flat_map(|scope| &scope.log_records)
        {
            if let Some(Value::StringValue(body)) =
                record.body.as_ref().and_then(|body| body.value.as_ref())
            {
                log_bodies.push(body.as_str());
            }
            timestamps.push((record.time_unix_nano, offset));
        }
    }
    assert!(
        span_names.contains(&"parse_and_sum"),
        "no `parse_and_sum` span in {span_names:?}"
    );
    assert!(
        log_bodies.contains(&"skipped invalid fields"),
        "no `skipped invalid fields` log in {log_bodies:?}"
    );

    let execution_timestamps = ExecutionTimestamps::decode_from_reader(
        std::fs::File::open(profile_folder.join(ExecutionTimestamps::file_name(None))).unwrap(),
    )
    .unwrap();
    let marker_timestamps = execution_timestamps
        .markers
        .iter()
        .map(|marker| match marker {
            MarkerType::SampleStart(timestamp)
            | MarkerType::SampleEnd(timestamp)
            | MarkerType::RoundStart(timestamp)
            | MarkerType::RoundEnd(timestamp) => *timestamp,
        });
    let markers = marker_timestamps.clone().min().unwrap()..=marker_timestamps.max().unwrap();
    for (timestamp, offset) in timestamps {
        let monotonic = timestamp - offset;
        assert!(
            markers.contains(&monotonic),
            "timestamp {timestamp} is {monotonic} on the monotonic clock, outside of the \
             benchmark markers {markers:?}"
        );
    }
}

/// Decodes the archived requests whose name starts with `prefix`.
fn decode_requests<M: Message + Default>(entries: &[(String, Vec<u8>)], prefix: &str) -> Vec<M> {
    entries
        .iter()
        .filter(|(name, _)| name.starts_with(prefix) && name.ends_with(".pb"))
        .map(|(_, content)| M::decode(content.as_slice()).unwrap())
        .collect()
}

/// The offset to subtract from the timestamps of `resource` to get the monotonic clock of the
/// benchmark markers.
fn monotonic_offset(resource: Option<&Resource>) -> u64 {
    let attribute = resource
        .into_iter()
        .flat_map(|resource| &resource.attributes)
        .find(|attribute| attribute.key == MONOTONIC_OFFSET_ATTRIBUTE)
        .expect("no monotonic clock offset in the resource");
    match attribute
        .value
        .as_ref()
        .and_then(|value| value.value.as_ref())
    {
        Some(Value::IntValue(offset)) => u64::try_from(*offset).unwrap(),
        value => panic!("unexpected monotonic clock offset {value:?}"),
    }
}
