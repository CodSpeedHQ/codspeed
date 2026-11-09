//! Writing and reading the telemetry artifacts, on the spans and logs of a telemetry-heavy benchmark:
//! about 10k spans and 10k logs, from nested `tracing` spans with attributes and an event in each.

use std::io::Cursor;
use std::path::Path;

use divan::Bencher;
use divan::counter::{BytesCount, ItemsCount};
use runner_shared::telemetry::{LogRow, SpanRow, TelemetryReader, TelemetryRow, TelemetryWriter};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    divan::main();
}

const SPANS: &[u8] = include_bytes!("../testdata/telemetry_volume_spans.msgpack.zst");
const LOGS: &[u8] = include_bytes!("../testdata/telemetry_volume_logs.msgpack.zst");

fn read<T: TelemetryRow>(artifact: &[u8]) -> Vec<T> {
    TelemetryReader::<T, _>::new(Cursor::new(artifact))
        .unwrap()
        .collect::<anyhow::Result<_>>()
        .unwrap()
}

fn bench_read<T: TelemetryRow>(bencher: Bencher, artifact: &[u8]) {
    let rows = read::<T>(artifact).len();
    bencher
        .counter(ItemsCount::new(rows))
        .counter(BytesCount::new(artifact.len()))
        .bench_local(|| read::<T>(artifact).len());
}

fn bench_write<T: TelemetryRow + Clone>(bencher: Bencher, artifact: &[u8]) {
    let rows = read::<T>(artifact);
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let path = dir.path().join("telemetry.msgpack.zst");
    let write = |rows: Vec<T>, path: &Path| {
        let mut writer = TelemetryWriter::<T>::create(path).unwrap();
        writer.write(rows).unwrap();
        writer.close().unwrap();
    };
    bencher
        .counter(ItemsCount::new(rows.len()))
        .with_inputs(|| rows.clone())
        .bench_local_values(|rows| write(rows, &path));
}

#[divan::bench]
fn read_spans(bencher: Bencher) {
    bench_read::<SpanRow>(bencher, SPANS);
}

#[divan::bench]
fn read_logs(bencher: Bencher) {
    bench_read::<LogRow>(bencher, LOGS);
}

#[divan::bench]
fn write_spans(bencher: Bencher) {
    bench_write::<SpanRow>(bencher, SPANS);
}

#[divan::bench]
fn write_logs(bencher: Bencher) {
    bench_write::<LogRow>(bencher, LOGS);
}
