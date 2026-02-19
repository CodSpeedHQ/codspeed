use super::ExecutionOptions;
use super::config::RoundOrTime;
use crate::constants::{INTEGRATION_NAME, INTEGRATION_VERSION};
use crate::prelude::*;
use instrument_hooks_bindings::InstrumentHooks;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;

pub fn run_rounds(
    bench_uri: String,
    command: Vec<String>,
    config: &ExecutionOptions,
) -> Result<Vec<u128>> {
    let warmup_time_ns = config.warmup_time_ns;
    let hooks = InstrumentHooks::instance(INTEGRATION_NAME, INTEGRATION_VERSION);

    let do_one_round = || -> Result<(u64, u64)> {
        let bench_round_start_ts_ns = InstrumentHooks::current_timestamp();
        let status = Command::new("nice")
            .args(["-n", "-20"])
            .arg(&command[0])
            .args(&command[1..])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("Failed to execute command")?;

        let bench_round_end_ts_ns = InstrumentHooks::current_timestamp();

        if !status.success() {
            bail!("Command exited with non-zero status: {status}");
        }

        Ok((bench_round_start_ts_ns, bench_round_end_ts_ns))
    };

    // Compute the number of rounds to perform (potentially undefined if no warmup and only time constraints)
    hooks.start_benchmark().unwrap();
    let rounds_to_perform: Option<u64> = if warmup_time_ns > 0 {
        match compute_rounds_from_warmup(config, do_one_round)? {
            WarmupResult::EarlyReturn { start, end } => {
                // Add marker for the single warmup round so the run still gets profiling data
                hooks.add_benchmark_timestamps(start, end);
                hooks.stop_benchmark().unwrap();
                hooks.set_executed_benchmark(&bench_uri).unwrap();
                return Ok(vec![(end - start) as u128]);
            }
            WarmupResult::Rounds(rounds) => Some(rounds),
        }
    } else {
        extract_rounds_from_config(config)
    };

    let (min_time_ns, max_time_ns) = extract_time_constraints(config);

    // Validate that we have at least one constraint when warmup is disabled
    if warmup_time_ns == 0
        && rounds_to_perform.is_none()
        && min_time_ns.is_none()
        && max_time_ns.is_none()
    {
        bail!(
            "When warmup is disabled, at least one constraint (min_rounds, max_rounds, min_time, or max_time) must be specified"
        );
    }

    if let Some(rounds) = rounds_to_perform {
        info!("Warmup done, now performing {rounds} rounds");
    } else {
        debug!(
            "Running in degraded mode (no warmup, time-based constraints only): min_time={}, max_time={}",
            min_time_ns
                .map(format_ns)
                .unwrap_or_else(|| "none".to_string()),
            max_time_ns
                .map(format_ns)
                .unwrap_or_else(|| "none".to_string())
        );
    }

    let mut times_per_round_ns = rounds_to_perform
        .map(|r| Vec::with_capacity(r as usize))
        .unwrap_or_default();
    let mut current_round: u64 = 0;

    debug!(
        "Starting loop with ending conditions: \
        rounds {rounds_to_perform:?}, \
        min_time_ns {min_time_ns:?}, \
        max_time_ns {max_time_ns:?}"
    );

    let round_start_ts_ns = InstrumentHooks::current_timestamp();

    let mut round_timestamps: Vec<(u64, u64)> = if let Some(rounds) = rounds_to_perform {
        Vec::with_capacity(rounds as usize)
    } else {
        Vec::new()
    };

    loop {
        let current_round_timestamps = do_one_round()?;
        // Only store timestamps for later processing in order to avoid overhead during the loop
        round_timestamps.push(current_round_timestamps);
        current_round += 1;

        let elapsed_ns = InstrumentHooks::current_timestamp() - round_start_ts_ns;

        // Check stop conditions
        let reached_max_rounds = rounds_to_perform.is_some_and(|r| current_round >= r);
        let reached_max_time = max_time_ns.is_some_and(|t| elapsed_ns >= t);
        let reached_min_time = min_time_ns.is_some_and(|t| elapsed_ns >= t);

        // Stop if we hit max_time
        if reached_max_time {
            debug!(
                "Reached maximum time limit after {current_round} rounds (elapsed: {}, max: {})",
                format_ns(elapsed_ns),
                format_ns(max_time_ns.unwrap())
            );
            break;
        }

        // Stop if we hit max_rounds
        if reached_max_rounds {
            break;
        }

        // If no rounds constraint, stop when min_time is reached
        if rounds_to_perform.is_none() && reached_min_time {
            debug!(
                "Reached minimum time after {current_round} rounds (elapsed: {}, min: {})",
                format_ns(elapsed_ns),
                format_ns(min_time_ns.unwrap())
            );
            break;
        }
    }

    // Record timestamps
    for (start, end) in round_timestamps {
        hooks.add_benchmark_timestamps(start, end);
        times_per_round_ns.push((end - start) as u128);
    }

    hooks.stop_benchmark().unwrap();
    hooks.set_executed_benchmark(&bench_uri).unwrap();

    Ok(times_per_round_ns)
}

enum WarmupResult {
    /// Warmup exceeded max_time constraint with a single run, return early with this single timestamp pair
    EarlyReturn { start: u64, end: u64 },
    /// Continue with this many rounds
    Rounds(u64),
}

/// Run warmup rounds and compute the number of benchmark rounds to perform
fn compute_rounds_from_warmup<F>(config: &ExecutionOptions, do_one_round: F) -> Result<WarmupResult>
where
    F: Fn() -> Result<(u64, u64)>,
{
    let mut warmup_timestamps: Vec<(u64, u64)> = Vec::new();
    let warmup_start_ts_ns = InstrumentHooks::current_timestamp();

    while InstrumentHooks::current_timestamp() < warmup_start_ts_ns + config.warmup_time_ns {
        let (start, end) = do_one_round()?;
        warmup_timestamps.push((start, end));
    }
    let warmup_end_ts_ns = InstrumentHooks::current_timestamp();

    // Check if single warmup round already exceeded max_time
    if let [(start, end)] = warmup_timestamps.as_slice() {
        let single_warmup_round_duration_ns = end - start;
        match config.max {
            Some(RoundOrTime::TimeNs(time_ns)) | Some(RoundOrTime::Both { time_ns, .. }) => {
                if time_ns <= single_warmup_round_duration_ns {
                    info!(
                        "A single warmup execution ({}) exceeded or met max_time ({}). No more rounds will be performed.",
                        format_ns(single_warmup_round_duration_ns),
                        format_ns(time_ns)
                    );
                    return Ok(WarmupResult::EarlyReturn {
                        start: *start,
                        end: *end,
                    });
                }
            }
            _ => { /* No max time constraint */ }
        }
    }

    info!("Completed {} warmup rounds", warmup_timestamps.len());

    let average_time_per_round_ns =
        (warmup_end_ts_ns - warmup_start_ts_ns) / warmup_timestamps.len() as u64;

    let actual_min_rounds = compute_min_rounds(config, average_time_per_round_ns);
    let actual_max_rounds = compute_max_rounds(config, average_time_per_round_ns);

    let rounds = match (actual_min_rounds, actual_max_rounds) {
        (Some(min), Some(max)) if min > max => {
            warn!(
                "Computed min rounds ({min}) is greater than max rounds ({max}). Using max rounds.",
            );
            max
        }
        (Some(min), Some(max)) => (min + max) / 2,
        (None, Some(max)) => max,
        (Some(min), None) => min,
        (None, None) => {
            bail!("Unable to determine number of rounds to perform");
        }
    };

    Ok(WarmupResult::Rounds(rounds))
}

/// Compute the minimum number of rounds based on config and average round time
fn compute_min_rounds(config: &ExecutionOptions, avg_time_per_round_ns: u64) -> Option<u64> {
    match &config.min {
        Some(RoundOrTime::Rounds(rounds)) => Some(*rounds),
        Some(RoundOrTime::TimeNs(time_ns)) => {
            Some(((time_ns + avg_time_per_round_ns) / avg_time_per_round_ns) + 1)
        }
        Some(RoundOrTime::Both { rounds, time_ns }) => {
            let rounds_from_time = ((time_ns + avg_time_per_round_ns) / avg_time_per_round_ns) + 1;
            Some((*rounds).max(rounds_from_time))
        }
        None => None,
    }
}

/// Compute the maximum number of rounds based on config and average round time
fn compute_max_rounds(config: &ExecutionOptions, avg_time_per_round_ns: u64) -> Option<u64> {
    match &config.max {
        Some(RoundOrTime::Rounds(rounds)) => Some(*rounds),
        Some(RoundOrTime::TimeNs(time_ns)) => {
            Some((time_ns + avg_time_per_round_ns) / avg_time_per_round_ns)
        }
        Some(RoundOrTime::Both { rounds, time_ns }) => {
            let rounds_from_time = (time_ns + avg_time_per_round_ns) / avg_time_per_round_ns;
            Some((*rounds).min(rounds_from_time))
        }
        None => None,
    }
}

/// Extract rounds directly from config (used when warmup is disabled)
fn extract_rounds_from_config(config: &ExecutionOptions) -> Option<u64> {
    match (&config.max, &config.min) {
        (Some(RoundOrTime::Rounds(rounds)), _) | (_, Some(RoundOrTime::Rounds(rounds))) => {
            Some(*rounds)
        }
        (Some(RoundOrTime::Both { rounds, .. }), _)
        | (_, Some(RoundOrTime::Both { rounds, .. })) => Some(*rounds),
        _ => None,
    }
}

/// Extract time constraints from config for stop conditions
fn extract_time_constraints(config: &ExecutionOptions) -> (Option<u64>, Option<u64>) {
    let min_time_ns = match &config.min {
        Some(RoundOrTime::TimeNs(time_ns)) | Some(RoundOrTime::Both { time_ns, .. }) => {
            Some(*time_ns)
        }
        _ => None,
    };
    let max_time_ns = match &config.max {
        Some(RoundOrTime::TimeNs(time_ns)) | Some(RoundOrTime::Both { time_ns, .. }) => {
            Some(*time_ns)
        }
        _ => None,
    };
    (min_time_ns, max_time_ns)
}

fn format_ns(ns: u64) -> String {
    format!("{:?}", Duration::from_nanos(ns))
}
