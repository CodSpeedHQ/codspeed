#[macro_use]
mod shared;

use runner_shared::artifacts::{MemtrackEvent, MemtrackEventKind};
use std::collections::HashSet;
use std::process::Command;
use tempfile::TempDir;

const COPY_SIZE: u32 = memtrack::DEFAULT_STACK_COPY_SIZE;

fn compile_fixture(
    name: &str,
    temp_dir: &TempDir,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    shared::compile_c_source(
        include_str!("../testdata/stack_paths.c"),
        name,
        temp_dir.path(),
    )
}
fn require_mapping_support() -> bool {
    if memtrack::MappingSupport::detect() == memtrack::MappingSupport::Unsupported {
        eprintln!("skipping stack capture test: mapping support is unavailable");
        return false;
    }
    true
}

/// The stack identity carried by each allocation and deallocation event that has one.
fn event_hashes(events: &[MemtrackEvent]) -> Vec<u64> {
    events
        .iter()
        .filter_map(|e| match e.kind {
            MemtrackEventKind::Malloc { stack_hash, .. }
            | MemtrackEventKind::Calloc { stack_hash, .. }
            | MemtrackEventKind::AlignedAlloc { stack_hash, .. }
            | MemtrackEventKind::Realloc { stack_hash, .. }
            | MemtrackEventKind::Free { stack_hash } => (stack_hash != 0).then_some(stack_hash),
            _ => None,
        })
        .collect()
}

fn record_hashes(events: &[MemtrackEvent]) -> HashSet<u64> {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            MemtrackEventKind::Stack { record } => Some(record.hash),
            _ => None,
        })
        .collect()
}

#[test_with::env(GITHUB_ACTIONS)]
#[test_log::test]
fn distinct_call_paths_get_distinct_stacks() -> Result<(), Box<dyn std::error::Error>> {
    if !require_mapping_support() {
        return Ok(());
    }
    let temp_dir = TempDir::new()?;
    let binary = compile_fixture("stack_paths", &temp_dir)?;
    let (events, thread_handle) =
        shared::track_command_with_stacks(Command::new(&binary), COPY_SIZE)?;

    let records: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.kind {
            MemtrackEventKind::Stack { record: r } => {
                Some((r.hash, r.sp, &r.regs, &r.bytes, r.truncated))
            }
            _ => None,
        })
        .collect();

    assert!(
        records.len() >= 2,
        "expected at least two stack records, got {} ({} events)",
        records.len(),
        events.len()
    );

    let hashes = record_hashes(&events);
    assert_eq!(
        hashes.len(),
        records.len(),
        "stack records must be deduplicated by unique hash"
    );

    for (hash, sp, regs, bytes, truncated) in &records {
        assert_ne!(*sp, 0, "record {hash:#x} has no stack pointer");
        assert_eq!(regs.len(), 33, "record {hash:#x} must carry 33 registers");
        assert!(
            !bytes.is_empty() && bytes.len() % 512 == 0 && bytes.len() <= COPY_SIZE as usize,
            "record {hash:#x} must hold whole 512-byte chunks within the budget, got {}",
            bytes.len()
        );
        assert_eq!(
            *truncated,
            bytes.len() == COPY_SIZE as usize,
            "record {hash:#x} may only be flagged truncated when it filled the budget"
        );
    }

    let carried = event_hashes(&events);
    assert!(
        !carried.is_empty(),
        "expected events carrying a captured stack hash"
    );
    assert!(
        carried.iter().all(|hash| hashes.contains(hash)),
        "every non-zero stack_hash must have a matching stack record"
    );

    // The fixture frees every allocation, so both sides must report identities.
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, MemtrackEventKind::Free { stack_hash } if stack_hash != 0)),
        "free events must carry their own stack identity"
    );

    thread_handle
        .join()
        .expect("tracker teardown thread panicked");
    Ok(())
}

#[test_with::env(GITHUB_ACTIONS)]
#[test_log::test]
fn dedup_collapses_repeated_call_paths() -> Result<(), Box<dyn std::error::Error>> {
    if !require_mapping_support() {
        return Ok(());
    }
    let temp_dir = TempDir::new()?;
    let binary = compile_fixture("stack_paths_dedup", &temp_dir)?;
    let (events, thread_handle) =
        shared::track_command_with_stacks(Command::new(&binary), COPY_SIZE)?;

    let carried = event_hashes(&events);
    let records = record_hashes(&events);
    assert!(
        carried.len() > records.len(),
        "expected repeated call paths to deduplicate raw stacks: {} stack-bearing events across {} unique stacks ({} total events)",
        carried.len(),
        records.len(),
        events.len()
    );

    thread_handle
        .join()
        .expect("tracker teardown thread panicked");
    Ok(())
}

/// The largest budget stresses the verifier hardest: the copy loop and its
/// unrolled per-chunk hash both scale with the configured size, so a program
/// that loads at the default can still exceed the instruction limit here.
/// It is also the only budget at which nothing can be budget-limited, because
/// the stack mapping always ends first.
#[test_with::env(GITHUB_ACTIONS)]
#[test_log::test]
fn max_copy_budget_loads_and_captures_whole_stacks() -> Result<(), Box<dyn std::error::Error>> {
    if !require_mapping_support() {
        return Ok(());
    }
    let temp_dir = TempDir::new()?;
    let binary = compile_fixture("stack_paths_max", &temp_dir)?;
    let (events, thread_handle) =
        shared::track_command_with_stacks(Command::new(&binary), u32::MAX)?;

    let truncated: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.kind {
            MemtrackEventKind::Stack { record: r } if r.truncated => Some(r.hash),
            _ => None,
        })
        .collect();

    assert!(
        !record_hashes(&events).is_empty(),
        "expected stack records at the maximum copy budget"
    );
    assert!(
        truncated.is_empty(),
        "no capture can be budget-limited at the maximum budget: {truncated:#x?}"
    );

    thread_handle
        .join()
        .expect("tracker teardown thread panicked");
    Ok(())
}

/// Restores the capture toggle on drop so a failing assertion cannot leak the
/// override into later tests (the suite runs single-threaded).
struct DisableCaptureGuard;

impl DisableCaptureGuard {
    fn set() -> Self {
        // SAFETY: tests run with --test-threads 1, so no concurrent env access.
        unsafe { std::env::set_var("CODSPEED_MEMTRACK_CAPTURE_STACKS", "0") };
        Self
    }
}

impl Drop for DisableCaptureGuard {
    fn drop(&mut self) {
        // SAFETY: see `set`.
        unsafe { std::env::remove_var("CODSPEED_MEMTRACK_CAPTURE_STACKS") };
    }
}

#[test_with::env(GITHUB_ACTIONS)]
#[test_log::test]
fn explicit_disable_suppresses_stack_capture() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let binary = compile_fixture("stack_paths_disabled", &temp_dir)?;
    let _guard = DisableCaptureGuard::set();
    let (events, thread_handle) = shared::track_binary(&binary)?;

    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, MemtrackEventKind::Malloc { .. })),
        "disabled capture must still report allocation events"
    );
    assert!(
        record_hashes(&events).is_empty(),
        "disabled capture must emit zero stack records"
    );
    assert!(
        event_hashes(&events).is_empty(),
        "disabled capture must leave stack_hash zero on every event"
    );

    thread_handle
        .join()
        .expect("tracker teardown thread panicked");
    Ok(())
}
