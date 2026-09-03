#![allow(dead_code, unused)]

pub use memtrack::OwnershipMaps;
use memtrack::prelude::*;
use memtrack::{BpfVariant, Tracker, TrackerOptions};
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

        // Keep only allocator events. mmap/munmap/brk sizes reflect allocator
        // arena reservations that vary per run, so including them here would
        // make these snapshots nondeterministic.
        let formatted_events: Vec<String> = $events
            .iter()
            .filter(|e| {
                matches!(
                    e.kind,
                    MemtrackEventKind::Malloc { .. }
                        | MemtrackEventKind::Free
                        | MemtrackEventKind::Calloc { .. }
                        | MemtrackEventKind::Realloc { .. }
                        | MemtrackEventKind::AlignedAlloc { .. }
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
        // Drop non-allocator events before slicing: the marker window's skip(2)
        // is positional (it drops [marker-malloc, marker-free]), so a stray
        // event sorting between the pair would displace it and leak the
        // marker free.
        .filter(|e| {
            !matches!(
                e.kind,
                MemtrackEventKind::Rss { .. }
                    | MemtrackEventKind::Rmap { .. }
                    | MemtrackEventKind::Fork { .. }
                    | MemtrackEventKind::Exec
                    | MemtrackEventKind::Exit
            )
        })
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

/// Track a command with the default probes: allocators only, discovered by the
/// exec-mapping watcher as the tracked tree maps executables.
pub fn track_command(command: Command) -> TrackResult {
    track_command_with_opts(command, TrackerOptions::builder().build())
}

/// Track a command under a specific BPF variant rather than the detected one.
pub fn track_command_with_variant(command: Command, variant: BpfVariant) -> TrackResult {
    track_command_with_tracker(command, Tracker::with_variant(variant)?)
}

/// Physical-memory tracking without allocator probes.
fn rmap_only_options() -> TrackerOptions {
    TrackerOptions::builder()
        .allocators(false)
        .physical(true)
        .build()
}

/// Track a command with physical-memory tracking enabled.
pub fn track_command_with_rmap(command: Command) -> TrackResult {
    track_command_with_opts(command, rmap_only_options())
}

/// Track a command with an explicit probe selection rather than the environment's.
pub fn track_command_with_opts(command: Command, options: TrackerOptions) -> TrackResult {
    track_command_with_tracker(command, Tracker::with_options(options)?)
}

/// Track a command with rmap hooks and snapshot its ownership maps after the
/// tracked tree exits but before tracker teardown frees the BPF maps.
pub fn track_command_with_rmap_maps(
    command: Command,
) -> anyhow::Result<(Vec<Event>, OwnershipMaps, std::thread::JoinHandle<()>)> {
    let tracker = Tracker::with_options(rmap_only_options())?;
    let (tracker, events, ()) = run_tracked(command, tracker, |_, _| Ok(()))?;
    let maps = tracker.ownership_maps()?;
    Ok((events, maps, std::thread::spawn(move || drop(tracker))))
}

/// Track a command with rmap hooks and snapshot the ownership maps at a
/// fixture-signalled checkpoint.
///
/// The fixture creates `ready` and waits for `release`. The root pid is returned
/// for correlating snapshot entries with events.
pub fn track_command_with_rmap_checkpoint(
    command: Command,
    ready: &Path,
    release: &Path,
) -> anyhow::Result<(Vec<Event>, OwnershipMaps, i32, std::thread::JoinHandle<()>)> {
    let tracker = Tracker::with_options(rmap_only_options())?;
    let (tracker, events, (maps, root_pid)) = run_tracked(command, tracker, |tracker, pid| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !ready.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let maps = tracker.ownership_maps()?;
        // Release before bailing: nothing else reaps the fixture, which is
        // parked holding its mapping.
        std::fs::write(release, b"")?;
        ensure!(ready.exists(), "fixture never reached the checkpoint");
        Ok((maps, pid))
    })?;
    Ok((
        events,
        maps,
        root_pid,
        std::thread::spawn(move || drop(tracker)),
    ))
}

/// How many events of each kind-and-size a run saw. Addresses, timestamps, pids
/// and event order all differ legitimately between runs of the same workload, so
/// none of them can be compared across variants.
type EventProfile = std::collections::BTreeMap<String, usize>;

fn event_profile(events: &[Event]) -> EventProfile {
    let mut profile = EventProfile::new();
    for event in events {
        // Only allocator events are comparable across variants: RSS and
        // lifecycle values (sizes, pids) are per-run.
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
    let (tracker, events, ()) = run_tracked(command, tracker, |_, _| Ok(()))?;

    // Detaching the probes blocks on RCU grace periods; let the caller decide
    // when to wait for it.
    let thread_handle = std::thread::spawn(move || drop(tracker));

    Ok((events, thread_handle))
}

/// Run `command` to completion under `tracker` and drain its events, handing
/// back the still-live tracker so BPF state can be read before teardown.
/// `checkpoint` runs while the tracked tree is still live.
fn run_tracked<T>(
    command: Command,
    tracker: Tracker,
    checkpoint: impl FnOnce(&Tracker, i32) -> anyhow::Result<T>,
) -> anyhow::Result<(Tracker, Vec<Event>, T)> {
    tracker.enable_tracking()?;

    let mut session = tracker.spawn(&command, None)?;
    let rx = session.take_events()?;
    let checkpoint = checkpoint(&tracker, session.pid())?;

    session.wait()?;
    // Dropping the session does a final ring buffer drain and closes the
    // channel, so collecting terminates without a silence timeout.
    drop(session);
    let events: Vec<Event> = rx.iter().collect();

    tracker.finish()?;

    info!("Tracked {} events", events.len());
    trace!("Events: {events:#?}");

    Ok((tracker, events, checkpoint))
}
