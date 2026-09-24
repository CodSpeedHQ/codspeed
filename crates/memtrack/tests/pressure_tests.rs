//! Ring-pressure pause under a deliberately slow poller: the event ring fills
//! well before the next poll tick, so writing producers must stay paused
//! until the poller has flushed the ring.

mod shared;

use memtrack::{Tracker, TrackerOptions};
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const THREADS: &str = "8";
const PROCESSES: &str = "16";
const ITERATIONS: &str = "400000";
/// Long enough that the ring's 75% watermark is crossed between two polls.
const SLOW_POLL_MS: u64 = 10_000;

struct Run {
    wall: Duration,
    dropped: u64,
}

fn run_storm(
    binary: &std::path::Path,
    args: [&str; 2],
    options: TrackerOptions,
) -> anyhow::Result<Run> {
    let tracker = Tracker::with_options(options)?;
    tracker.enable_tracking()?;

    let mut command = Command::new(binary);
    command.args(args);

    let started = Instant::now();
    let mut session = tracker.spawn(&command, None)?;
    let rx = session.take_events()?;
    let status = session.wait()?;
    let wall = started.elapsed();
    assert!(status.success(), "fixture failed: {status}");

    drop(session);
    let events: usize = rx.into_iter().map(|batch| batch.len()).sum();
    tracker.finish()?;
    let dropped = tracker.dropped_events_count()?;
    drop(tracker);

    eprintln!("wall {wall:?}, events {events}, dropped {dropped}");
    Ok(Run { wall, dropped })
}

#[test_with::env(GITHUB_ACTIONS)]
#[test]
fn slow_poller_pause_recovers_without_loss() -> anyhow::Result<()> {
    let _ = env_logger::builder().is_test(true).try_init();
    let dir = TempDir::new()?;
    let binary = shared::compile_c_source(
        include_str!("../testdata/alloc_storm.c"),
        "alloc_storm",
        dir.path(),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    eprintln!("-- baseline: fast poller");
    let baseline = run_storm(
        &binary,
        [THREADS, ITERATIONS],
        TrackerOptions::builder().build(),
    )?;

    eprintln!("-- slow poller");
    let blocked = run_storm(
        &binary,
        [THREADS, ITERATIONS],
        TrackerOptions::builder()
            .poll_interval_ms(SLOW_POLL_MS)
            .build(),
    )?;

    eprintln!(
        "baseline {:?} | blocked {:?} (dropped {})",
        baseline.wall, blocked.wall, blocked.dropped
    );

    assert_eq!(blocked.dropped, 0, "pressure pause lost events");
    Ok(())
}

/// Every writing process is stopped on its own and must be resumed: a
/// producer left stopped hangs the parent's `wait`, a missed one drops events.
#[test_with::env(GITHUB_ACTIONS)]
#[test]
fn slow_poller_pause_resumes_every_writing_process() -> anyhow::Result<()> {
    let _ = env_logger::builder().is_test(true).try_init();
    let dir = TempDir::new()?;
    let binary = shared::compile_c_source(
        include_str!("../testdata/alloc_storm_procs.c"),
        "alloc_storm_procs",
        dir.path(),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    let blocked = run_storm(
        &binary,
        [PROCESSES, ITERATIONS],
        TrackerOptions::builder()
            .poll_interval_ms(SLOW_POLL_MS)
            .build(),
    )?;

    assert_eq!(blocked.dropped, 0, "pressure pause lost events");
    Ok(())
}
