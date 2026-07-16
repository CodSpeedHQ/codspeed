#![allow(dead_code, unused)]

use anyhow::Context;
use memtrack::prelude::*;
use memtrack::{AllocatorLib, Tracker};
use runner_shared::artifacts::{MemtrackEvent as Event, MemtrackEventKind};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

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
pub fn compile_rust_binary(
    crate_dir: &Path,
    name: &str,
    features: &[&str],
) -> anyhow::Result<std::path::PathBuf> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(crate_dir)
        .args(["build", "--release", "--bin", name]);

    if !features.is_empty() {
        cmd.arg("--features").arg(features.join(","));
    }

    let output = cmd.output()?;
    if !output.status.success() {
        eprintln!("cargo stderr: {}", String::from_utf8_lossy(&output.stderr));
        eprintln!("cargo stdout: {}", String::from_utf8_lossy(&output.stdout));
        return Err(anyhow::anyhow!("Failed to compile Rust crate"));
    }

    Ok(crate_dir.join(format!("target/release/{name}")))
}

/// Track a spawned command, collecting all memory events.
///
/// When `discover_system_allocators` is true, the tracker will scan for all
/// allocators on the system (slower). When false, only `extra_allocators` are used.
pub fn track_command(
    mut command: Command,
    extra_allocators: &[AllocatorLib],
    discover_system_allocators: bool,
) -> TrackResult {
    // IMPORTANT: Always initialize the tracker BEFORE spawning the binary, as it can take some time to
    // attach to all the allocator libraries (especially when using NixOS).
    let mut tracker = if discover_system_allocators {
        memtrack::Tracker::new()?
    } else {
        memtrack::Tracker::new_without_allocators()?
    };
    tracker.attach_allocators(extra_allocators)?;

    let child = command.spawn().context("Failed to spawn command")?;
    let root_pid = child.id() as i32;

    tracker.enable()?;
    let rx = tracker.track(root_pid)?;

    let mut events = Vec::new();
    while let Ok(event) = rx.recv_timeout(Duration::from_secs(10)) {
        events.push(event);
    }

    // Drop the tracker in a new thread to not block the test
    let thread_handle = std::thread::spawn(move || core::mem::drop(tracker));

    info!("Tracked {} events", events.len());
    trace!("Events: {events:#?}");

    Ok((events, thread_handle))
}

pub fn track_binary_with_opts(binary: &Path, extra_allocators: &[AllocatorLib]) -> TrackResult {
    track_command(Command::new(binary), extra_allocators, true)
}

pub fn track_binary(binary: &Path) -> TrackResult {
    track_binary_with_opts(binary, &[])
}
