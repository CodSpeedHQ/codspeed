//! WARNING: Has to be in sync with `instrument-hooks`.

pub const RUNNER_CTL_FIFO: &str = "/tmp/runner.ctl.fifo";
pub const RUNNER_ACK_FIFO: &str = "/tmp/runner.ack.fifo";

/// Be very careful when changing this, as this will break support for integrations built with versions stricly lower than this.
/// Any change of this should be planned ahead of time, with deprecation warnings, and the release
/// of integrations supporting the new protocol version a significant amount of time before
/// releasing the runner.
pub const MINIMAL_SUPPORTED_PROTOCOL_VERSION: u64 = 1;
pub const CURRENT_PROTOCOL_VERSION: u64 = 2;

const _: () = assert!(
    MINIMAL_SUPPORTED_PROTOCOL_VERSION <= CURRENT_PROTOCOL_VERSION,
    "MINIMAL_SUPPORTED_PROTOCOL_VERSION must be less than or equal to CURRENT_PROTOCOL_VERSION"
);

/// The different markers that can be set in the perf.data.
///
/// `SampleStart/End`: Marks the start and end of a sampling period. This is used to differentiate between benchmarks.
/// `BenchmarkStart/End`: Marks the start and end of a benchmark. This is used to measure the duration of a benchmark, without the benchmark harness code.
#[derive(
    serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone,
)]
pub enum MarkerType {
    SampleStart(u64),
    SampleEnd(u64),
    BenchmarkStart(u64),
    BenchmarkEnd(u64),
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationMode {
    Walltime,
    Simulation,
    Analysis,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
pub enum Command {
    CurrentBenchmark {
        pid: i32,
        uri: String,
    },
    StartBenchmark,
    StopBenchmark,
    Ack,
    #[deprecated(note = "Use `GetIntegrationMode` instead")]
    PingProfiler,
    SetIntegration {
        name: String,
        version: String,
    },
    Err,
    AddMarker {
        pid: i32,
        marker: MarkerType,
    },
    SetVersion(u64),
    GetIntegrationMode,
    IntegrationModeResponse(IntegrationMode),
}
