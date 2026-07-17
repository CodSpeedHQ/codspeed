#![allow(dead_code, unused)]

use memtrack::prelude::*;
use memtrack::{BpfVariant, Tracker};
use runner_shared::artifacts::{MemtrackEvent as Event, MemtrackEventKind};
use std::path::Path;
use std::process::Command;

pub type TrackResult = anyhow::Result<(Vec<Event>, std::thread::JoinHandle<()>)>;

/// Snapshot every tracked event, ordered by timestamp and deduplicated by
/// `(addr, kind)` so repeated tracking of one allocation counts once.
///
/// ```no_run
/// let (events, _handle) = track_binary(&binary)?;
/// assert_events_snapshot!("test_name", events);
/// ```
macro_rules! assert_events_snapshot {
    ($name:expr, $events:expr) => {{
        use itertools::Itertools;
        use runner_shared::artifacts::MemtrackEventKind;
        use std::mem::discriminant;

        let formatted_events: Vec<String> = $events
            .iter()
            .filter(|e| {
                // Allocation snapshots track only allocator events; RSS and
                // process-lifecycle events are asserted by dedicated tests.
                !matches!(
                    e.kind,
                    MemtrackEventKind::Rss { .. }
                        | MemtrackEventKind::Fork { .. }
                        | MemtrackEventKind::Exec
                        | MemtrackEventKind::Exit
                )
            })
            .sorted_by_key(|e| e.timestamp)
            .dedup_by(|a, b| a.addr == b.addr && discriminant(&a.kind) == discriminant(&b.kind))
            .map(|e| shared::describe_kind(&e.kind))
            .collect();
        insta::assert_debug_snapshot!($name, formatted_events);
    }};
}

/// [`assert_events_snapshot`] over only the events the workload bracketed with
/// `malloc(0xC0D59EED)`, which keeps libc and runtime noise out of the snapshot.
/// The workload must allocate the marker before and after the region of interest:
///
/// ```no_run
/// malloc(0xC0D59EED);
/// // allocations under test
/// malloc(0xC0D59EED);
/// ```
///
/// ```no_run
/// let (events, _handle) = track_binary(&binary)?;
/// assert_events_with_marker!("test_name", events);
/// ```
macro_rules! assert_events_with_marker {
    ($name:expr, $events:expr) => {{
        let filtered_events = shared::between_markers($events);
        assert_events_snapshot!($name, &filtered_events);
    }};
}

/// [`assert_events_snapshot`] run under each BPF variant. `$workload` is called
/// once per variant, so it must return a fresh [`Command`] every time.
///
/// ```no_run
/// assert_events_snapshot_for_each_variant!("test_name", || Command::new(&binary));
/// ```
macro_rules! assert_events_snapshot_for_each_variant {
    ($name:expr, $workload:expr) => {
        shared::for_each_variant($workload, |events| {
            assert_events_snapshot!($name, events);
        })
    };
}

/// [`assert_events_with_marker`] run under each BPF variant. `$workload` is
/// called once per variant, so it must return a fresh [`Command`] every time.
macro_rules! assert_events_with_marker_for_each_variant {
    ($name:expr, $workload:expr) => {
        shared::for_each_variant($workload, |events| {
            assert_events_with_marker!($name, events);
        })
    };
}

/// An event's kind and size, without the addresses that differ between runs of
/// the same workload. `Realloc` needs spelling out since its `Debug` includes the
/// old address.
pub fn describe_kind(kind: &MemtrackEventKind) -> String {
    match kind {
        MemtrackEventKind::Realloc { size, .. } => format!("Realloc {{ size: {size} }}"),
        other => format!("{other:?}"),
    }
}

/// The events between the workload's `malloc(0xC0D59EED)` markers, ordered by
/// timestamp and deduplicated by `(addr, kind)`.
pub fn between_markers(events: &[Event]) -> Vec<Event> {
    use itertools::Itertools;
    use std::mem::discriminant;

    const MARKER: u64 = 0xC0D5_9EED;
    let is_marker =
        |e: &&Event| matches!(e.kind, MemtrackEventKind::Malloc { size } if size == MARKER);

    events
        .iter()
        // Drop Rss before slicing: the marker window's skip(2) is positional
        // (it drops [marker-malloc, marker-free]), so a stray rss_stat event
        // sorting between the pair would displace it and leak the marker free.
        .filter(|e| !matches!(e.kind, MemtrackEventKind::Rss { .. }))
        .sorted_by_key(|e| e.timestamp)
        .dedup_by(|a, b| a.addr == b.addr && discriminant(&a.kind) == discriminant(&b.kind))
        .skip_while(|e| !is_marker(e))
        .skip(2) // the opening marker malloc and its free
        .take_while(|e| !is_marker(e))
        .cloned()
        .collect()
}

/// Compile a Rust binary from a test crate directory. Each feature set gets its
/// own target dir, otherwise parallel test cases race to overwrite one binary.
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

pub fn compile_c_source(
    source_code: &str,
    name: &str,
    output_dir: &Path,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let source_path = output_dir.join(format!("{name}.c"));
    let binary_path = output_dir.join(name);
    std::fs::write(&source_path, source_code)?;

    let output = Command::new("gcc")
        .args(["-O0", "-o", binary_path.to_str().unwrap()])
        .arg(&source_path)
        .output()?;
    if !output.status.success() {
        error!("gcc stderr: {}", String::from_utf8_lossy(&output.stderr));
        return Err("Failed to compile C fixture".into());
    }

    Ok(binary_path)
}

/// Track a command, collecting all memory events. No allocators are pre-attached:
/// the exec-mapping watcher discovers them as the tracked tree maps executables.
pub fn track_command(command: Command) -> TrackResult {
    track_command_with_tracker(command, Tracker::new()?)
}

/// Track a command under a specific BPF variant rather than the detected one.
pub fn track_command_with_variant(command: Command, variant: BpfVariant) -> TrackResult {
    track_command_with_tracker(command, Tracker::with_variant(variant)?)
}

/// Track a command with folio rmap hooks enabled, reconstructing per-process RSS.
pub fn track_command_with_rmap(command: Command) -> TrackResult {
    track_command_with_tracker(command, Tracker::new_without_allocators_with_rmap(true)?)
}

/// How many events of each kind-and-size a run saw. Addresses, timestamps, pids
/// and event order all differ legitimately between runs of the same workload, so
/// none of them can be compared across variants.
type EventProfile = std::collections::BTreeMap<String, usize>;

fn event_profile(events: &[Event]) -> EventProfile {
    let mut profile = EventProfile::new();
    for event in events {
        // Only allocator events are comparable: the variants differ solely in
        // how uprobes attach, while RSS and lifecycle events carry per-run
        // values (resident sizes, pids) that legitimately differ.
        if !matches!(
            event.kind,
            MemtrackEventKind::Malloc { .. }
                | MemtrackEventKind::Free
                | MemtrackEventKind::Calloc { .. }
                | MemtrackEventKind::Realloc { .. }
                | MemtrackEventKind::AlignedAlloc { .. }
        ) {
            continue;
        }
        *profile.entry(describe_kind(&event.kind)).or_default() += 1;
    }
    profile
}

/// Run `workload` under each BPF variant, pass every run's events to
/// `assert_events`, and require the variants to have observed the same
/// allocations: they differ only in how probes attach, so what one sees the other
/// must see too.
///
/// Variants that cannot attach on this host are skipped, since the token variant
/// needs `uprobe_multi` (kernel >= 6.6). Panics if none attach.
pub fn for_each_variant(
    mut workload: impl FnMut() -> Command,
    mut assert_events: impl FnMut(&[Event]),
) -> anyhow::Result<()> {
    let mut profiles: Vec<(BpfVariant, EventProfile)> = Vec::new();

    for variant in [BpfVariant::Legacy, BpfVariant::Token] {
        let tracker = match Tracker::with_variant(variant) {
            Ok(tracker) => tracker,
            Err(err) => {
                eprintln!("skipping {variant:?} variant, cannot attach here: {err:#}");
                continue;
            }
        };

        let (events, thread_handle) = track_command_with_tracker(workload(), tracker)?;
        assert_events(&events);
        profiles.push((variant, event_profile(&events)));
        thread_handle.join().unwrap();
    }

    let Some(((first_variant, first), rest)) = profiles.split_first() else {
        panic!("no BPF variant could attach");
    };
    for (variant, profile) in rest {
        assert_eq!(
            first, profile,
            "{first_variant:?} and {variant:?} variants disagree on tracked allocations"
        );
    }

    Ok(())
}

fn track_command_with_tracker(command: Command, tracker: Tracker) -> TrackResult {
    tracker.enable_tracking()?;

    let mut session = tracker.spawn(&command, None)?;
    let rx = session.take_events()?;

    session.wait()?;
    // Dropping the session does a final ring buffer drain and closes the
    // channel, so collecting terminates without a silence timeout.
    drop(session);
    let events: Vec<Event> = rx.iter().collect();

    tracker.finish()?;

    // Detaching the probes blocks on RCU grace periods; let the caller decide
    // when to wait for it.
    let thread_handle = std::thread::spawn(move || drop(tracker));

    info!("Tracked {} events", events.len());
    trace!("Events: {events:#?}");

    Ok((events, thread_handle))
}

/// Track a command with rmap, enabling tracking only after the target creates
/// `ready_path`. The target is spawned and resumed first, so any memory it
/// faults before signalling `ready` is already resident when tracking turns on.
/// The caller enables tracking, then creates `go_path` to release the target.
pub fn track_command_with_rmap_late_enable(
    command: Command,
    ready_path: &Path,
    go_path: &Path,
) -> TrackResult {
    let tracker = Tracker::new_without_allocators_with_rmap(true)?;

    let mut session = tracker.spawn(&command, None)?;
    let rx = session.take_events()?;

    let handshake = (|| -> anyhow::Result<()> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !ready_path.exists() {
            if std::time::Instant::now() > deadline {
                anyhow::bail!("target never signalled baseline-ready");
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        tracker.enable_tracking()?;
        std::fs::write(go_path, b"go")?;
        Ok(())
    })();

    // A failed handshake leaves the target blocked on `go_path`; Session has no
    // Drop kill, so reap it explicitly before propagating or the test hangs.
    if let Err(e) = handshake {
        unsafe { libc::kill(session.pid(), libc::SIGKILL) };
        let _ = session.wait();
        let _ = tracker.finish();
        return Err(e);
    }

    session.wait()?;
    drop(session);
    let events: Vec<Event> = rx.iter().collect();

    tracker.finish()?;
    let thread_handle = std::thread::spawn(move || drop(tracker));

    info!("Tracked {} events (late enable)", events.len());
    Ok((events, thread_handle))
}
