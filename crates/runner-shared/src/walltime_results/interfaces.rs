// WARN: Keep in sync with codspeed-rust

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BenchmarkMetadata {
    pub name: String,
    pub uri: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BenchmarkStats {
    pub min_ns: f64,
    pub max_ns: f64,
    pub mean_ns: f64,
    pub stdev_ns: f64,

    pub q1_ns: f64,
    pub median_ns: f64,
    pub q3_ns: f64,
    /// p1..p99 of the time per iteration: `[24]`, `[49]` and `[74]` equal `q1_ns`, `median_ns` and `q3_ns`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percentiles_ns: Option<Vec<f64>>,

    pub rounds: u64,
    /// Total time in **seconds**
    pub total_time: f64,
    pub iqr_outlier_rounds: u64,
    pub stdev_outlier_rounds: u64,
    pub iter_per_round: u64,
    pub warmup_iters: u64,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub(super) struct BenchmarkConfig {
    pub warmup_time_ns: Option<f64>,
    pub min_round_time_ns: Option<f64>,
    pub max_time_ns: Option<f64>,
    pub max_rounds: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WalltimeBenchmark {
    #[serde(flatten)]
    pub metadata: BenchmarkMetadata,

    pub(super) config: BenchmarkConfig,
    pub stats: BenchmarkStats,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Instrument {
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Creator {
    pub name: String,
    pub version: String,
    pub pid: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WalltimeResults {
    pub creator: Creator,
    pub instrument: Instrument,
    pub benchmarks: Vec<WalltimeBenchmark>,
}
