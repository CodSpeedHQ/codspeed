use itertools::Itertools;

use super::{BenchmarkConfig, BenchmarkMetadata, BenchmarkStats, WalltimeBenchmark};

impl WalltimeBenchmark {
    /// Create a WalltimeBenchmark from runtime data.
    /// Stats computations are designed to match pytest-codspeed's behavior.
    pub fn from_runtime_data(
        name: String,
        uri: String,
        iters_per_round: Vec<u128>,
        times_per_round_ns: Vec<u128>,
        _max_time_ns: Option<u128>,
    ) -> Self {
        // Calculate total time in ⚠️ seconds ⚠️
        let total_time_s = times_per_round_ns.iter().sum::<u128>() as f64 / 1_000_000_000.0;

        // Calculate statistics
        let times_per_iteration_per_round_ns_sorted: Vec<_> = times_per_round_ns
            .into_iter()
            .zip(&iters_per_round)
            .map(|(time_per_round, iter_per_round)| time_per_round / iter_per_round)
            .map(|t| t as f64)
            .sorted_by(|a, b| a.partial_cmp(b).unwrap())
            .collect::<Vec<f64>>();

        let rounds = times_per_iteration_per_round_ns_sorted.len();
        let mean_ns = if rounds > 0 {
            times_per_iteration_per_round_ns_sorted.iter().sum::<f64>() / rounds as f64
        } else {
            0.0
        };

        let min_ns = times_per_iteration_per_round_ns_sorted
            .first()
            .copied()
            .unwrap_or(0.0);
        let max_ns = times_per_iteration_per_round_ns_sorted
            .last()
            .copied()
            .unwrap_or(0.0);

        // Calculate percentiles
        let median_ns = if rounds > 0 {
            let mid = rounds / 2;
            if rounds % 2 == 0 {
                (times_per_iteration_per_round_ns_sorted[mid - 1]
                    + times_per_iteration_per_round_ns_sorted[mid])
                    / 2.0
            } else {
                times_per_iteration_per_round_ns_sorted[mid]
            }
        } else {
            0.0
        };

        let q1_ns = quantile(&times_per_iteration_per_round_ns_sorted, 0.25);
        let q3_ns = quantile(&times_per_iteration_per_round_ns_sorted, 0.75);
        let stdev_ns = sample_stdev(&times_per_iteration_per_round_ns_sorted, mean_ns);

        // Calculate outliers (simplified - using IQR method)
        let iqr = q3_ns - q1_ns;
        let lower_bound = q1_ns - 1.5 * iqr;
        let upper_bound = q3_ns + 1.5 * iqr;
        let iqr_outlier_rounds = times_per_iteration_per_round_ns_sorted
            .iter()
            .filter(|&&t| t < lower_bound || t > upper_bound)
            .count() as u64;

        // Standard deviation outliers (2 sigma)
        let stdev_outlier_rounds = times_per_iteration_per_round_ns_sorted
            .iter()
            .filter(|&&t| (t - mean_ns).abs() > 2.0 * stdev_ns)
            .count() as u64;

        // TODO(COD-1056): We currently only support single iteration count per round
        let iter_per_round = if iters_per_round.is_empty() {
            0
        } else {
            (iters_per_round.iter().sum::<u128>() / iters_per_round.len() as u128) as u64
        };

        WalltimeBenchmark {
            metadata: BenchmarkMetadata { name, uri },
            config: BenchmarkConfig::default(),
            stats: BenchmarkStats {
                min_ns,
                max_ns,
                mean_ns,
                stdev_ns,
                q1_ns,
                median_ns,
                q3_ns,
                percentiles_ns: None,
                rounds: rounds as u64,
                total_time: total_time_s,
                iqr_outlier_rounds,
                stdev_outlier_rounds,
                iter_per_round,
                warmup_iters: 0,
            },
        }
    }
}

/// Calculate sample standard deviation (n-1 denominator).
/// This is intended to match pytest-codspeed's computation, which uses python's
/// statistics.stdev
fn sample_stdev(data: &[f64], mean: f64) -> f64 {
    let n = data.len();
    if n <= 1 {
        return 0.0;
    }
    let variance: f64 = data
        .iter()
        .map(|&t| {
            let diff = t - mean;
            diff * diff
        })
        .sum::<f64>()
        / (n - 1) as f64;
    variance.sqrt()
}

/// Calculate quantile with linear interpolation.
/// This is intended to match pytest-codspeed's computation, which uses python's
/// statistics.quantiles
///
/// `p` is the quantile (e.g., 0.25 for Q1, 0.75 for Q3).
fn quantile(sorted_data: &[f64], p: f64) -> f64 {
    let n = sorted_data.len();
    if n == 0 {
        return 0.0;
    }
    if n == 1 {
        return sorted_data[0];
    }
    if n == 2 {
        // Linear interpolation between the two values
        return sorted_data[0] * (1.0 - p) + sorted_data[1] * p;
    }

    // Python's exclusive method: position = p * (n + 1) - 1 (0-based indexing)
    let pos = p * (n as f64 + 1.0) - 1.0;
    let idx = pos.floor() as usize;
    let frac = pos - pos.floor();

    if idx + 1 < n {
        sorted_data[idx] * (1.0 - frac) + sorted_data[idx + 1] * frac
    } else {
        sorted_data[idx.min(n - 1)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = "benchmark";
    const URI: &str = "test::benchmark";

    #[test]
    fn test_parse_single_benchmark() {
        let benchmark = WalltimeBenchmark::from_runtime_data(
            NAME.to_string(),
            URI.to_string(),
            vec![1],
            vec![42],
            None,
        );
        assert_eq!(benchmark.stats.stdev_ns, 0.);
        assert_eq!(benchmark.stats.min_ns, 42.);
        assert_eq!(benchmark.stats.max_ns, 42.);
        assert_eq!(benchmark.stats.mean_ns, 42.);
    }

    #[test]
    fn test_parse_bench_with_variable_iterations() {
        let iters_per_round = vec![1, 2, 3, 4, 5, 6];
        let total_rounds = iters_per_round.iter().sum::<u128>() as f64;

        let benchmark = WalltimeBenchmark::from_runtime_data(
            NAME.to_string(),
            URI.to_string(),
            iters_per_round,
            vec![42, 42 * 2, 42 * 3, 42 * 4, 42 * 5, 42 * 6],
            None,
        );

        assert_eq!(benchmark.stats.stdev_ns, 0.);
        assert_eq!(benchmark.stats.min_ns, 42.);
        assert_eq!(benchmark.stats.max_ns, 42.);
        assert_eq!(benchmark.stats.mean_ns, 42.);
        assert_eq!(
            benchmark.stats.total_time,
            42. * total_rounds / 1_000_000_000.0
        );
    }

    #[test]
    fn test_percentiles_are_optional_in_results_files() {
        let benchmark = WalltimeBenchmark::from_runtime_data(
            NAME.to_string(),
            URI.to_string(),
            vec![1],
            vec![42],
            None,
        );
        let mut json = serde_json::to_value(&benchmark).unwrap();
        assert!(json["stats"].get("percentiles_ns").is_none());

        let parsed: WalltimeBenchmark = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(parsed.stats.percentiles_ns, None);

        json["stats"]["percentiles_ns"] = serde_json::json!([41.0, 42.0, 43.0]);
        let parsed: WalltimeBenchmark = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.stats.percentiles_ns, Some(vec![41.0, 42.0, 43.0]));
    }

    #[test]
    fn test_basic_stats_computation() {
        // Test with a simple benchmark with consistent iterations
        // 5 rounds, each with 10 iterations
        // Total round times: 1000ns, 2000ns, 3000ns, 4000ns, 6000ns
        // Per-iteration times: 100ns, 200ns, 300ns, 400ns, 600ns
        // This creates a right-skewed distribution where mean != median
        let round_times = vec![6000, 3000, 1000, 2000, 4000];
        let iters_per_round = vec![10; 5];

        let benchmark = WalltimeBenchmark::from_runtime_data(
            NAME.to_string(),
            URI.to_string(),
            iters_per_round,
            round_times.clone(),
            None,
        );

        // Per-iteration times are: 100, 200, 300, 400, 600
        assert_eq!(benchmark.stats.min_ns, 100.0);
        assert_eq!(benchmark.stats.max_ns, 600.0);
        assert_eq!(benchmark.stats.mean_ns, 320.0); // (100+200+300+400+600)/5
        assert_eq!(benchmark.stats.median_ns, 300.0); // Middle value (different from mean)
        assert_eq!(benchmark.stats.q1_ns, 150.0);
        assert_eq!(benchmark.stats.q3_ns, 500.0);
        assert_eq!(benchmark.stats.rounds, 5);

        // Total time: (1000+2000+3000+4000+6000)ns = 16000ns = 0.000016s
        assert_eq!(benchmark.stats.total_time, 16000.0 / 1e9);

        // Average iterations per round: (10+10+10+10+10)/5 = 10
        assert_eq!(benchmark.stats.iter_per_round, 10);

        // Standard deviation (sample stdev, n-1 denominator):
        // variance = [(100-320)^2 + (200-320)^2 + (300-320)^2 + (400-320)^2 + (600-320)^2] / 4
        //          = [48400 + 14400 + 400 + 6400 + 78400] / 4 = 37000
        // stdev = sqrt(37000) ≈ 192.354
        let expected_stdev = 37000.0_f64.sqrt();
        assert!((benchmark.stats.stdev_ns - expected_stdev).abs() < 0.01);

        // No outliers in this right-skewed data (600 is not extreme enough)
        assert_eq!(benchmark.stats.iqr_outlier_rounds, 0);
        assert_eq!(benchmark.stats.stdev_outlier_rounds, 0);
    }

    #[test]
    fn test_stdev_outlier_detection() {
        // Test standard deviation outlier detection (2-sigma rule in Rust, vs 3-sigma in C++)
        // Create data with a clear outlier
        let mut round_times = vec![1; 16];
        round_times.push(50);
        let iters_per_round = vec![1; 17];

        let benchmark = WalltimeBenchmark::from_runtime_data(
            NAME.to_string(),
            URI.to_string(),
            iters_per_round,
            round_times,
            None,
        );

        // Mean should be around 3.88
        assert_eq!(benchmark.stats.mean_ns, 3.8823529411764706);

        assert_eq!(benchmark.stats.median_ns, 1.0);

        // Stdev should be around 11.88 (sample stdev)
        assert_eq!(benchmark.stats.stdev_ns, 11.884245626780316);

        // Should detect the outlier (50 is > mean + 2*stdev)
        assert_eq!(benchmark.stats.stdev_outlier_rounds, 1);
    }

    #[test]
    fn test_iqr_outlier_detection() {
        // Test IQR outlier detection (1.5*IQR rule)
        // Per-iteration times: 100, 110, 120, 130, 140, 500 (ns)
        let round_times = vec![100, 110, 120, 130, 140, 500];
        let iters_per_round = vec![1; 6];

        let benchmark = WalltimeBenchmark::from_runtime_data(
            NAME.to_string(),
            URI.to_string(),
            iters_per_round,
            round_times,
            None,
        );

        // Using ISO quantile calculation (exclusive method with interpolation)
        // Q1 = 107.5, Q3 = 230.0
        assert_eq!(benchmark.stats.q1_ns, 107.5);
        assert_eq!(benchmark.stats.q3_ns, 230.0);

        // IQR = 230 - 107.5 = 122.5
        // Lower bound: 107.5 - 1.5*122.5 = -76.25
        // Upper bound: 230 + 1.5*122.5 = 413.75
        // Outlier: 500 (> 413.75)
        assert_eq!(benchmark.stats.iqr_outlier_rounds, 1);
    }

    #[test]
    fn test_single_round_edge_case() {
        // Test with a single round
        let benchmark = WalltimeBenchmark::from_runtime_data(
            NAME.to_string(),
            URI.to_string(),
            vec![5],
            vec![500], // 100ns per iter
            None,
        );

        // With a single value, all stats should be the same
        assert_eq!(benchmark.stats.min_ns, 100.0);
        assert_eq!(benchmark.stats.max_ns, 100.0);
        assert_eq!(benchmark.stats.mean_ns, 100.0);
        assert_eq!(benchmark.stats.median_ns, 100.0);
        assert_eq!(benchmark.stats.q1_ns, 100.0);
        assert_eq!(benchmark.stats.q3_ns, 100.0);
        assert_eq!(benchmark.stats.stdev_ns, 0.0);
        assert_eq!(benchmark.stats.rounds, 1);
        assert_eq!(benchmark.stats.iqr_outlier_rounds, 0);
        assert_eq!(benchmark.stats.stdev_outlier_rounds, 0);

        // Total time: 500ns = 0.0000005s
        assert_eq!(benchmark.stats.total_time, 500.0 / 1e9);
    }

    #[test]
    fn test_quantile_computation() {
        // Test quantile computation with a specific dataset
        // Per-iteration times: 10, 20, 30, 40, 50, 60, 70, 80, 90 (ns)
        let round_times = vec![10, 20, 30, 40, 50, 60, 70, 80, 90];
        let iters_per_round = vec![1; 9];

        let benchmark = WalltimeBenchmark::from_runtime_data(
            NAME.to_string(),
            URI.to_string(),
            iters_per_round,
            round_times,
            None,
        );

        // With 9 values, using ISO quantile calculation (exclusive method):
        // Q1 = 25.0, Median = 50.0, Q3 = 75.0
        assert_eq!(benchmark.stats.q1_ns, 25.0);
        assert_eq!(benchmark.stats.median_ns, 50.0);
        assert_eq!(benchmark.stats.q3_ns, 75.0);
    }

    #[test]
    fn test_quantile_interpolation() {
        // Test quantile computation with even number of values
        let round_times = vec![10, 20, 30, 40, 50, 60, 70, 80];
        let iters_per_round = vec![1; 8];

        let benchmark = WalltimeBenchmark::from_runtime_data(
            NAME.to_string(),
            URI.to_string(),
            iters_per_round,
            round_times,
            None,
        );

        // With 8 values, using ISO quantile calculation (exclusive method):
        // Q1 = 22.5, Median = 45.0, Q3 = 67.5
        assert_eq!(benchmark.stats.q1_ns, 22.5);
        assert_eq!(benchmark.stats.median_ns, 45.0);
        assert_eq!(benchmark.stats.q3_ns, 67.5);
    }

    #[test]
    fn test_empty_rounds() {
        // Test with no rounds (edge case)
        let benchmark = WalltimeBenchmark::from_runtime_data(
            NAME.to_string(),
            URI.to_string(),
            vec![],
            vec![],
            None,
        );

        // All stats should be zero or default
        assert_eq!(benchmark.stats.min_ns, 0.0);
        assert_eq!(benchmark.stats.max_ns, 0.0);
        assert_eq!(benchmark.stats.mean_ns, 0.0);
        assert_eq!(benchmark.stats.median_ns, 0.0);
        assert_eq!(benchmark.stats.q1_ns, 0.0);
        assert_eq!(benchmark.stats.q3_ns, 0.0);
        assert_eq!(benchmark.stats.stdev_ns, 0.0);
        assert_eq!(benchmark.stats.rounds, 0);
        assert_eq!(benchmark.stats.total_time, 0.0);
        assert_eq!(benchmark.stats.iqr_outlier_rounds, 0);
        assert_eq!(benchmark.stats.stdev_outlier_rounds, 0);
    }

    #[test]
    fn test_two_rounds() {
        // Test with exactly two rounds (edge case for median calculation)
        let benchmark = WalltimeBenchmark::from_runtime_data(
            NAME.to_string(),
            URI.to_string(),
            vec![1, 1],
            vec![100, 200],
            None,
        );

        assert_eq!(benchmark.stats.min_ns, 100.0);
        assert_eq!(benchmark.stats.max_ns, 200.0);
        assert_eq!(benchmark.stats.mean_ns, 150.0);
        assert_eq!(benchmark.stats.median_ns, 150.0); // Average of the two values
        assert_eq!(benchmark.stats.rounds, 2);

        // Standard deviation for two values (sample stdev, n-1 denominator)
        // variance = [(100-150)^2 + (200-150)^2] / 1 = [2500 + 2500] = 5000
        // stdev = sqrt(5000) ≈ 70.71
        assert_eq!(benchmark.stats.stdev_ns, 70.71067811865476);
    }
}
