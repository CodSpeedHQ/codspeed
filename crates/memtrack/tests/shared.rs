#![allow(dead_code, unused)]

use memtrack::Tracker;
use memtrack::prelude::*;
use runner_shared::artifacts::{MemtrackEvent as Event, MemtrackEventKind};
use std::path::Path;
use std::process::Command;

type TrackResult = anyhow::Result<(Vec<Event>, std::thread::JoinHandle<()>)>;

/// Asserts memory events using snapshot testing without marker filtering.
///
/// Formats and snapshots all memory events for regression testing.
/// Deduplicates by address and type to remove duplicate tracking.
///
/// # Example
/// ```no_run
/// let (events, _handle) = track_binary(&binary)?;
/// assert_events_snapshot!("test_name", events);
/// ```
macro_rules! assert_events_snapshot {
    ($name:expr, $events:expr) => {{
        use itertools::Itertools;
        use runner_shared::artifacts::MemtrackEventKind;
        use std::mem::discriminant;

        // Dedup events by address and type to remove duplicates
        let events = $events
            .iter()
            .sorted_by_key(|e| e.timestamp)
            .dedup_by(|a, b| a.addr == b.addr && discriminant(&a.kind) == discriminant(&b.kind))
            .collect::<Vec<_>>();

        let formatted_events: Vec<String> = events
            .iter()
            .map(|e| match e.kind {
                // Exclude address in snapshots:
                MemtrackEventKind::Realloc { size, .. } => format!("Realloc {{ size: {} }}", size),
                _ => format!("{:?}", e.kind),
            })
            .collect();
        insta::assert_debug_snapshot!($name, formatted_events);
    }};
}

/// Asserts events, filtered by a 0xC0D59EED allocation marker to
/// exclude noise.
///
/// Processes events by:
/// 1. Deduplicating by address and event type
/// 2. Filtering to events between 0xC0D59EED marker allocations
/// 3. Creating an insta snapshot for regression testing
///
/// Binary must alloc `0xC0D59EED` memory before and after.
///
/// # Example
///
/// Do the following in the test:
/// ```no_run
/// malloc(0xC0D59EED)
/// // do you allocations/frees here
/// malloc(0xC0D59EED)
/// ```
///
/// Then in Rust:
/// ```no_run
/// let (events, _handle) = track_binary(&binary)?;
/// assert_events_with_marker!("test_name", events);
/// ```
macro_rules! assert_events_with_marker {
    ($name:expr, $events:expr) => {{
        use itertools::Itertools;
        use runner_shared::artifacts::MemtrackEventKind;
        use std::mem::discriminant;

        // Remove events outside our 0xC0D59EED marker allocations
        let filtered_events = $events
            .iter()
            .sorted_by_key(|e| e.timestamp)
            .dedup_by(|a, b| a.addr == b.addr && discriminant(&a.kind) == discriminant(&b.kind))
            .skip_while(|e| {
                let MemtrackEventKind::Malloc { size } = e.kind else {
                    return true;
                };
                size != 0xC0D59EED
            })
            .skip(2) // Skip the marker allocation and free
            .take_while(|e| {
                let MemtrackEventKind::Malloc { size } = e.kind else {
                    return true;
                };
                size != 0xC0D59EED
            })
            .cloned()
            .collect::<Vec<_>>();

        assert_events_snapshot!($name, filtered_events);
    }};
}

/// Compile a Rust binary from a test crate directory.
///
/// Each feature set builds into its own target dir: parallel test cases would
/// otherwise overwrite the same output binary and race against each other.
pub fn compile_rust_binary(
    crate_dir: &Path,
    name: &str,
    features: &[&str],
) -> anyhow::Result<std::path::PathBuf> {
    let target_dir = match features {
        [] => "target/default".to_string(),
        _ => format!("target/{}", features.join("-")),
    };

    let mut cmd = Command::new("cargo");
    cmd.current_dir(crate_dir).args([
        "build",
        "--release",
        "--bin",
        name,
        "--target-dir",
        &target_dir,
    ]);

    if !features.is_empty() {
        cmd.arg("--features").arg(features.join(","));
    }

    let output = cmd.output()?;
    if !output.status.success() {
        eprintln!("cargo stderr: {}", String::from_utf8_lossy(&output.stderr));
        eprintln!("cargo stdout: {}", String::from_utf8_lossy(&output.stdout));
        return Err(anyhow::anyhow!("Failed to compile Rust crate"));
    }

    Ok(crate_dir.join(format!("{target_dir}/release/{name}")))
}

/// Track a binary, collecting all memory events.
pub fn track_binary(binary: &Path) -> TrackResult {
    track_command(Command::new(binary))
}

/// Track a command, collecting all memory events.
///
/// No allocators are pre-attached: the exec-mapping watcher discovers and
/// attaches them as the tracked tree maps executable files.
pub fn track_command(command: Command) -> TrackResult {
    let tracker = Tracker::new()?;
    tracker.enable_tracking()?;

    let mut session = tracker.spawn(&command, None)?;
    let rx = session.take_events()?;

    session.wait()?;
    // Dropping the session does a final ring buffer drain and closes the
    // channel, so collecting terminates without a silence timeout.
    drop(session);
    let events: Vec<Event> = rx.iter().collect();

    tracker.finish()?;

    // Drop the tracker in a new thread to not block the test.
    let thread_handle = std::thread::spawn(move || drop(tracker));

    info!("Tracked {} events", events.len());
    trace!("Events: {events:#?}");

    Ok((events, thread_handle))
}
