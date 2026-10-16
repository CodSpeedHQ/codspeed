//! End-to-end OTLP collection of the spans and logs of a benchmark built from the codspeed-rust
//! checkout.
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
    let traces: Vec<_> = entries
        .iter()
        .filter(|(name, _)| name.starts_with("traces-") && name.ends_with(".pb"))
        .collect();
    assert!(!traces.is_empty(), "no trace request in {entries:?}");

    let has_benchmark_span = traces.iter().any(|(_, content)| {
        content
            .windows(b"parse_and_sum".len())
            .any(|window| window == b"parse_and_sum")
    });
    assert!(has_benchmark_span, "no `parse_and_sum` span in the traces");

    let logs: Vec<_> = entries
        .iter()
        .filter(|(name, _)| name.starts_with("logs-") && name.ends_with(".pb"))
        .collect();
    assert!(!logs.is_empty(), "no log request in {entries:?}");

    let has_benchmark_log = logs.iter().any(|(_, content)| {
        content
            .windows(b"skipped invalid fields".len())
            .any(|window| window == b"skipped invalid fields")
    });
    assert!(
        has_benchmark_log,
        "no `skipped invalid fields` log in the logs"
    );
}
