use crate::prelude::*;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const DEFAULT_WARMUP_TIME_NS: u64 = 1_000_000_000; // 1 second
/// Default maximum time if no constraints are provided
const DEFAULT_MAX_TIME_NS: u64 = 3_000_000_000; // 3 seconds

/// Parse a duration string into nanoseconds
/// Supports humantime format: "1s", "500ms", "1.5s", "2m", "1h", etc.
/// Also supports pure numbers interpreted as seconds (e.g., "2" = 2s, "1.5" = 1.5s)
fn parse_duration_to_ns(s: &str) -> Result<u64> {
    let s = s.trim();

    let duration: Duration = match s.parse::<f64>() {
        // Pure numbers are interpreted as seconds. `try_from_secs_f64` rejects negative, NaN and
        // infinite values, which a bare `as u64` cast would silently turn into 0 or u64::MAX.
        Ok(seconds) => Duration::try_from_secs_f64(seconds).with_context(|| {
            format!("Invalid duration: '{s}'. Expected a finite, non-negative number of seconds")
        })?,
        Err(_) => humantime::parse_duration(s).with_context(|| format!("Invalid duration format: '{s}'. Expected format like '1s', '500ms', '2m', '1h' or a number in seconds"))?,
    };

    Ok(duration.as_nanos() as u64)
}

/// Arguments for walltime execution configuration
///
/// ⚠️ Make sure to update WalltimeExecutionArgs::to_cli_args() when fields change, else the runner
/// will not properly forward arguments
#[derive(Debug, Clone, Default, clap::Args, Serialize, Deserialize)]
pub struct WalltimeExecutionArgs {
    /// Duration of the warmup phase before measurement starts.
    /// During warmup, the benchmark runs to stabilize performance (e.g., JIT compilation, cache warming).
    /// Set to "0s" or "0" to disable warmup.
    ///
    /// Format: duration string (e.g., "1s", "500ms", "1.5s", "2m") or number in seconds (e.g., "1", "0.5")
    /// Default: 1s
    #[arg(long, value_name = "DURATION")]
    pub warmup_time: Option<String>,

    /// Maximum total time to spend running benchmarks (includes warmup).
    /// Execution stops when this time is reached, even if min_rounds is not satisfied.
    /// When used with --min-rounds, the max_time constraint takes priority.
    ///
    /// Format: duration string (e.g., "3s", "15s", "1m") or number in seconds (e.g., "3", "15")
    /// Default: 3s if no other constraints are set, 0 (unlimited) if one of min_time, max_rounds,
    /// or min_rounds is set
    #[arg(long, value_name = "DURATION")]
    pub max_time: Option<String>,

    /// Minimum total time to spend running benchmarks (excludes warmup).
    /// Ensures benchmarks run for at least this duration for statistical accuracy.
    /// When used with --max-rounds, we try to satisfy both if possible, else max_rounds takes priority.
    ///
    /// Format: duration string (e.g., "1s", "500ms") or number in seconds (e.g., "1", "0.5")
    /// Default: undefined (no minimum)
    #[arg(long, value_name = "DURATION")]
    pub min_time: Option<String>,

    /// Maximum number of benchmark iterations (rounds) to perform.
    /// Execution stops after this many rounds, even if max_time is not reached.
    /// When used with --min-time, we try to satisfy both if possible, else max_rounds takes priority.
    ///
    /// Format: positive integer
    /// Default: undefined (determined by timing constraints)
    #[arg(long, value_name = "COUNT")]
    pub max_rounds: Option<u64>,

    /// Minimum number of benchmark iterations (rounds) to perform.
    /// Ensures at least this many rounds are executed for statistical accuracy.
    /// When used with --max-time, the max_time constraint takes priority.
    ///
    /// Format: positive integer
    /// Default: undefined (determined by timing constraints)
    #[arg(long, value_name = "COUNT")]
    pub min_rounds: Option<u64>,
}

impl WalltimeExecutionArgs {
    /// Convert WalltimeExecutionArgs back to CLI argument strings
    ///
    /// Unfortunately, clap does not provide a built-in way to serialize args back to CLI format,
    // Clippy is very confused since this is used in the runner, but not in the binary of exec-harness
    pub fn to_cli_args(&self) -> Vec<String> {
        let mut args = Vec::new();

        if let Some(warmup) = &self.warmup_time {
            args.push("--warmup-time".to_string());
            args.push(warmup.clone());
        }

        if let Some(max_time) = &self.max_time {
            args.push("--max-time".to_string());
            args.push(max_time.clone());
        }

        if let Some(min_time) = &self.min_time {
            args.push("--min-time".to_string());
            args.push(min_time.clone());
        }

        if let Some(max_rounds) = &self.max_rounds {
            args.push("--max-rounds".to_string());
            args.push(max_rounds.to_string());
        }

        if let Some(min_rounds) = &self.min_rounds {
            args.push("--min-rounds".to_string());
            args.push(min_rounds.to_string());
        }

        args
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RoundOrTime {
    /// Explicit number of rounds
    Rounds(u64),
    /// Explicit time in nanoseconds
    TimeNs(u64),
    /// Both rounds and time specified. The most restrictive will be used, e.g the smaller number
    /// of rounds or shorter time for min boundaries.
    Both { rounds: u64, time_ns: u64 },
}

#[derive(Debug)]
pub struct ExecutionOptions {
    pub(crate) warmup_time_ns: u64,
    pub(crate) min: Option<RoundOrTime>,
    pub(crate) max: Option<RoundOrTime>,
}

impl TryFrom<WalltimeExecutionArgs> for ExecutionOptions {
    type Error = anyhow::Error;

    /// Convert WalltimeExecutionArgs to ExecutionOptions with validation
    ///
    /// Check that the input is coherent with rules:
    /// - min_xxx cannot be greater than max_xxx (for same dimension)
    ///
    /// When constraints of different dimensions are mixed (e.g., min_time + max_rounds):
    /// - We try to satisfy both if possible
    /// - Otherwise, the MAX constraint takes priority
    ///
    /// When warmup is disabled and only time constraints are provided:
    /// - We run in "degraded mode" where we try to satisfy the constraint best-effort
    /// - For max_time: actual_benched_time < max_time + one_iteration_time
    fn try_from(args: WalltimeExecutionArgs) -> Result<Self> {
        // Parse duration strings
        let warmup_time_ns = args
            .warmup_time
            .as_ref()
            .map(|s| parse_duration_to_ns(s))
            .transpose()
            .context("Invalid warmup_time")?;

        let max_time_ns = args
            .max_time
            .as_ref()
            .map(|s| parse_duration_to_ns(s))
            .transpose()
            .context("Invalid max_time")?
            .unwrap_or_else(|| {
                // No max_time provided, use default only if no round-based constraints are set
                if args.max_rounds.is_some() || args.min_rounds.is_some() || args.min_time.is_some()
                {
                    0
                } else {
                    DEFAULT_MAX_TIME_NS
                }
            });

        let min_time_ns = args
            .min_time
            .as_ref()
            .map(|s| parse_duration_to_ns(s))
            .transpose()
            .context("Invalid min_time")?;

        // Validation: min_xxx cannot be greater than max_xxx (for same dimension)
        if max_time_ns > 0 {
            if let Some(min) = min_time_ns {
                if min > max_time_ns {
                    bail!(
                        "min_time ({:.2}s) cannot be greater than max_time ({:.2}s)",
                        min as f64 / 1_000_000_000.0,
                        max_time_ns as f64 / 1_000_000_000.0
                    );
                }
            }
        }

        if let (Some(min), Some(max)) = (args.min_rounds, args.max_rounds) {
            if min > max {
                bail!("min_rounds ({min}) cannot be greater than max_rounds ({max})");
            }
        }

        // Build min/max using RoundOrTime enum
        // Now we allow mixing time and rounds constraints across min/max bounds
        let min = match (args.min_rounds, min_time_ns) {
            (Some(rounds), None) => Some(RoundOrTime::Rounds(rounds)),
            (None, Some(time_ns)) => Some(RoundOrTime::TimeNs(time_ns)),
            (Some(rounds), Some(time_ns)) => Some(RoundOrTime::Both { rounds, time_ns }),
            (None, None) => None,
        };

        let max = match (args.max_rounds, max_time_ns) {
            (Some(rounds), 0) => Some(RoundOrTime::Rounds(rounds)),
            (Some(rounds), time_ns) => Some(RoundOrTime::Both { rounds, time_ns }),
            (None, 0) => None,
            (None, time_ns) => Some(RoundOrTime::TimeNs(time_ns)),
        };

        Ok(Self {
            warmup_time_ns: warmup_time_ns.unwrap_or(DEFAULT_WARMUP_TIME_NS),
            min,
            max,
        })
    }
}

impl Default for ExecutionOptions {
    fn default() -> Self {
        ExecutionOptions {
            warmup_time_ns: DEFAULT_WARMUP_TIME_NS,
            min: None,
            max: Some(RoundOrTime::TimeNs(DEFAULT_MAX_TIME_NS)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_to_ns_pure_numbers() {
        // Pure numbers should be interpreted as seconds
        assert_eq!(parse_duration_to_ns("1").unwrap(), 1_000_000_000); // 1 second
        assert_eq!(parse_duration_to_ns("2").unwrap(), 2_000_000_000); // 2 seconds
        assert_eq!(parse_duration_to_ns("0.5").unwrap(), 500_000_000); // 0.5 seconds
        assert_eq!(parse_duration_to_ns("0").unwrap(), 0);
        // `-0` parses as `-0.0`, which is still a valid way to disable warmup
        assert_eq!(parse_duration_to_ns("-0").unwrap(), 0);
        assert_eq!(parse_duration_to_ns("1.5").unwrap(), 1_500_000_000); // 1.5 seconds
    }

    #[test]
    fn test_parse_duration_to_ns_humantime_formats() {
        // Humantime format durations
        assert_eq!(parse_duration_to_ns("1s").unwrap(), 1_000_000_000);
        assert_eq!(parse_duration_to_ns("500ms").unwrap(), 500_000_000);
        assert_eq!(parse_duration_to_ns("2m").unwrap(), 120_000_000_000);
        assert_eq!(parse_duration_to_ns("1h").unwrap(), 3_600_000_000_000);

        // Fractional values
        assert_eq!(parse_duration_to_ns("1.5s").unwrap(), 1_500_000_000);
        assert_eq!(parse_duration_to_ns("0.5s").unwrap(), 500_000_000);
    }

    #[test]
    fn test_parse_duration_to_ns_whitespace() {
        // Should handle whitespace
        assert_eq!(parse_duration_to_ns("  1s  ").unwrap(), 1_000_000_000);
        assert_eq!(parse_duration_to_ns(" 500ms ").unwrap(), 500_000_000);
    }

    #[test]
    fn test_parse_duration_to_ns_invalid() {
        // Invalid formats should error
        assert!(parse_duration_to_ns("invalid").is_err());
        assert!(parse_duration_to_ns("1x").is_err());
        assert!(parse_duration_to_ns("").is_err());
    }

    #[test]
    fn test_parse_duration_to_ns_rejects_negative_and_non_finite_numbers() {
        for input in ["-1", "-0.5", "NaN", "inf", "-inf"] {
            assert!(
                parse_duration_to_ns(input).is_err(),
                "expected '{input}' to be rejected"
            );
        }
    }

    #[test]
    fn test_execution_options_from_args() {
        // Test creating ExecutionOptions from CLI args
        let opts: ExecutionOptions = WalltimeExecutionArgs {
            warmup_time: Some("2s".to_string()),
            max_time: Some("10s".to_string()),
            min_time: None,
            max_rounds: Some(10),
            min_rounds: None,
        }
        .try_into()
        .unwrap();

        assert_eq!(opts.warmup_time_ns, 2_000_000_000);
        assert!(matches!(
            opts.max,
            Some(RoundOrTime::Both {
                rounds: 10,
                time_ns: 10_000_000_000
            })
        ));
        assert!(opts.min.is_none());
    }

    #[test]
    fn test_execution_options_from_args_none() {
        // Test with all None values (should use defaults)
        let opts: ExecutionOptions = WalltimeExecutionArgs::default().try_into().unwrap();

        assert_eq!(opts.warmup_time_ns, DEFAULT_WARMUP_TIME_NS);
        assert_eq!(opts.max, Some(RoundOrTime::TimeNs(DEFAULT_MAX_TIME_NS)));
        assert!(opts.min.is_none());
    }

    #[test]
    fn test_execution_options_from_args_invalid_duration() {
        // Test with invalid duration string
        let result: Result<ExecutionOptions> = WalltimeExecutionArgs {
            warmup_time: Some("invalid".to_string()),
            max_time: None,
            min_time: None,
            max_rounds: None,
            min_rounds: None,
        }
        .try_into();

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid warmup_time")
        );
    }

    #[test]
    fn test_execution_options_from_args_negative_max_time() {
        let result: Result<ExecutionOptions> = WalltimeExecutionArgs {
            warmup_time: None,
            max_time: Some("-1".to_string()),
            min_time: None,
            max_rounds: None,
            min_rounds: None,
        }
        .try_into();

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Invalid max_time"),
            "Expected error about max_time, got: {err}"
        );
    }

    // Business rule validation tests

    #[test]
    fn test_mixing_min_time_and_max_rounds_is_allowed() {
        // min_time + max_rounds (different dimensions)
        // The constraint will be: try to satisfy both if possible, else max_rounds takes priority
        let result: Result<ExecutionOptions> = WalltimeExecutionArgs {
            warmup_time: Some("1s".to_string()),
            max_time: None,
            min_time: Some("2s".to_string()),
            max_rounds: Some(10),
            min_rounds: None,
        }
        .try_into();

        assert!(result.is_ok());
        let opts = result.unwrap();
        assert!(matches!(opts.min, Some(RoundOrTime::TimeNs(_))));
        assert!(matches!(opts.max, Some(RoundOrTime::Rounds(10))));
    }

    #[test]
    fn test_mixing_max_time_and_min_rounds_is_allowed() {
        // Now allowed: max_time + min_rounds (different dimensions)
        // The constraint will be: max_time constraint takes priority
        let result: Result<ExecutionOptions> = WalltimeExecutionArgs {
            warmup_time: Some("1s".to_string()),
            max_time: Some("10s".to_string()),
            min_time: None,
            max_rounds: None,
            min_rounds: Some(5),
        }
        .try_into();

        assert!(result.is_ok());
        let opts = result.unwrap();
        assert!(matches!(opts.min, Some(RoundOrTime::Rounds(5))));
        assert!(matches!(opts.max, Some(RoundOrTime::TimeNs(_))));
    }

    #[test]
    fn test_validation_min_time_greater_than_max_time() {
        let result: Result<ExecutionOptions> = WalltimeExecutionArgs {
            warmup_time: Some("1s".to_string()),
            max_time: Some("5s".to_string()),
            min_time: Some("10s".to_string()), // min > max!
            max_rounds: None,
            min_rounds: None,
        }
        .try_into();

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("min_time") && err.contains("cannot be greater than max_time"),
            "Expected error about min_time > max_time, got: {err}"
        );
    }

    #[test]
    fn test_validation_min_rounds_greater_than_max_rounds() {
        let result: Result<ExecutionOptions> = WalltimeExecutionArgs {
            warmup_time: Some("1s".to_string()),
            max_time: None,
            min_time: None,
            max_rounds: Some(10),
            min_rounds: Some(50), // min > max!
        }
        .try_into();

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("min_rounds") && err.contains("cannot be greater than max_rounds"),
            "Expected error about min_rounds > max_rounds, got: {err}"
        );
    }

    #[test]
    fn test_no_warmup_with_time_only_is_allowed() {
        // No warmup + time constraints only is now allowed (degraded mode)
        // Validation happens at runtime in run_rounds
        let result: Result<ExecutionOptions> = WalltimeExecutionArgs {
            warmup_time: Some("0".to_string()), // No warmup
            max_time: Some("10s".to_string()),
            min_time: None,
            max_rounds: None, // No rounds specified
            min_rounds: None,
        }
        .try_into();

        assert!(result.is_ok());
        let opts = result.unwrap();
        assert_eq!(opts.warmup_time_ns, 0);
        assert!(matches!(opts.max, Some(RoundOrTime::TimeNs(_))));
    }

    #[test]
    fn test_validation_valid_combinations() {
        // Valid: max_time + max_rounds
        let result: Result<ExecutionOptions> = WalltimeExecutionArgs {
            warmup_time: Some("1s".to_string()),
            max_time: Some("10s".to_string()),
            min_time: None,
            max_rounds: Some(5),
            min_rounds: None,
        }
        .try_into();
        assert!(result.is_ok());

        // Valid: min_time + min_rounds
        let result: Result<ExecutionOptions> = WalltimeExecutionArgs {
            warmup_time: Some("1s".to_string()),
            max_time: None,
            min_time: Some("2s".to_string()),
            max_rounds: None,
            min_rounds: Some(100),
        }
        .try_into();
        assert!(result.is_ok());

        // Valid: max_time + min_time (with min < max)
        let result: Result<ExecutionOptions> = WalltimeExecutionArgs {
            warmup_time: Some("1s".to_string()),
            max_time: Some("10s".to_string()),
            min_time: Some("2s".to_string()),
            max_rounds: None,
            min_rounds: None,
        }
        .try_into();
        assert!(result.is_ok());

        // Valid: max_rounds + min_rounds (with min < max)
        let result: Result<ExecutionOptions> = WalltimeExecutionArgs {
            warmup_time: Some("1s".to_string()),
            max_time: None,
            min_time: None,
            max_rounds: Some(100),
            min_rounds: Some(10),
        }
        .try_into();
        assert!(result.is_ok());

        // Valid: no warmup with rounds specified
        let result: Result<ExecutionOptions> = WalltimeExecutionArgs {
            warmup_time: Some("0".to_string()),
            max_time: None,
            min_time: None,
            max_rounds: Some(50),
            min_rounds: None,
        }
        .try_into();
        assert!(result.is_ok());
    }
}
