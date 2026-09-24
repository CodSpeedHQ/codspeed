//! Delta-compressed stack records must decode to exactly the bytes the kernel
//! hashed. The decoder drops any record whose reconstruction fails the hash
//! check, so a run with many records and every allocation's stack hash
//! resolvable proves the kernel encoder and userspace decoder agree.
#[macro_use]
mod shared;

use memtrack::stack_codec::fnv_stack_hash;
use memtrack::{Tracker, TrackerOptions};
use rstest::rstest;
use runner_shared::artifacts::MemtrackEventKind;
use std::collections::HashSet;
use std::process::Command;
use tempfile::TempDir;

fn alloc_stack_hash(kind: &MemtrackEventKind) -> Option<u64> {
    match kind {
        MemtrackEventKind::Malloc { stack_hash, .. }
        | MemtrackEventKind::Calloc { stack_hash, .. }
        | MemtrackEventKind::AlignedAlloc { stack_hash, .. }
        | MemtrackEventKind::Realloc { stack_hash, .. }
        | MemtrackEventKind::Free { stack_hash } => Some(*stack_hash),
        _ => None,
    }
}

#[test_with::env(GITHUB_ACTIONS)]
#[rstest]
#[case(8192)]
#[case(32768)]
#[test_log::test]
fn delta_records_decode_to_hashed_bytes(
    #[case] budget: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let binary = shared::compile_c_source(
        include_str!("../testdata/stack_churn.c"),
        "stack_churn",
        temp_dir.path(),
    )?;
    let options = TrackerOptions::builder()
        .stack_capture(true)
        .stack_budget(budget)
        .build();
    let tracker = Tracker::with_options(options)?;
    let (tracker, events, ()) = shared::run_tracked(Command::new(binary), tracker, |_, _| Ok(()))?;

    let capture_stats = tracker.stack_capture_stats()?;
    let teardown = std::thread::spawn(move || drop(tracker));

    let mut emitted = HashSet::new();
    let mut stack_records = 0usize;
    for event in &events {
        let MemtrackEventKind::Stack { record } = &event.kind else {
            continue;
        };
        stack_records += 1;
        assert_eq!(record.bytes.len() % 512, 0, "copy length is chunked");
        assert!(record.bytes.len() as u32 <= budget);
        assert_eq!(fnv_stack_hash(&record.bytes), record.hash);
        emitted.insert(record.hash);
    }

    let referenced: Vec<u64> = events
        .iter()
        .filter_map(|e| alloc_stack_hash(&e.kind))
        .filter(|&hash| hash != 0)
        .collect();
    let unresolved = referenced
        .iter()
        .filter(|hash| !emitted.contains(hash))
        .count();

    eprintln!(
        "budget {budget}: {stack_records} stack records, {} referencing allocations, {capture_stats:?}",
        referenced.len()
    );
    assert!(stack_records >= 1000, "expected many distinct stacks");
    assert_eq!(capture_stats.ring_full, 0);
    assert_eq!(capture_stats.delta_fallback, 0);
    assert_eq!(
        unresolved, 0,
        "every allocation stack hash has a decoded record"
    );

    teardown.join().expect("tracker teardown thread panicked");
    Ok(())
}
