#[macro_use]
mod shared;

use itertools::Itertools;
use memtrack::TrackerOptions;
use rstest::rstest;
use runner_shared::artifacts::{MemtrackEvent, MemtrackEventKind};
use shared::AllocationTestCase;
use std::mem::discriminant;
use std::process::Command;
use tempfile::TempDir;

fn describe_allocator_event(kind: &MemtrackEventKind) -> Option<String> {
    let description = match kind {
        MemtrackEventKind::Malloc { size, stack_hash } => {
            format!("Malloc {{ size: {size}, has_stack: {} }}", *stack_hash != 0)
        }
        MemtrackEventKind::Calloc { size, stack_hash } => {
            format!("Calloc {{ size: {size}, has_stack: {} }}", *stack_hash != 0)
        }
        MemtrackEventKind::AlignedAlloc { size, stack_hash } => format!(
            "AlignedAlloc {{ size: {size}, has_stack: {} }}",
            *stack_hash != 0
        ),
        MemtrackEventKind::Realloc {
            size, stack_hash, ..
        } => {
            format!(
                "Realloc {{ size: {size}, has_stack: {} }}",
                *stack_hash != 0
            )
        }
        MemtrackEventKind::Free { stack_hash } => {
            format!("Free {{ has_stack: {} }}", *stack_hash != 0)
        }
        _ => return None,
    };

    Some(description)
}

fn format_events(events: &[MemtrackEvent]) -> Vec<String> {
    const MARKER: u64 = 0xC0D5_9EED;
    let has_markers = events.iter().any(|e| {
        matches!(
            e.kind,
            MemtrackEventKind::Malloc { size, .. } if size == MARKER
        )
    });

    let filtered_events = if has_markers {
        shared::between_markers(events)
    } else {
        events
            .iter()
            .filter(|e| {
                matches!(
                    e.kind,
                    MemtrackEventKind::Malloc { .. }
                        | MemtrackEventKind::Free { .. }
                        | MemtrackEventKind::Calloc { .. }
                        | MemtrackEventKind::Realloc { .. }
                        | MemtrackEventKind::AlignedAlloc { .. }
                )
            })
            .sorted_by_key(|e| e.timestamp)
            .dedup_by(|a, b| a.addr == b.addr && discriminant(&a.kind) == discriminant(&b.kind))
            .cloned()
            .collect()
    };

    filtered_events
        .iter()
        .filter_map(|e| describe_allocator_event(&e.kind))
        .collect()
}

const STACK_TEST_CASES: &[AllocationTestCase] = &[
    AllocationTestCase {
        name: "stack_paths",
        source: include_str!("../testdata/stack_paths.c"),
    },
    AllocationTestCase {
        name: "nested_doubling",
        source: include_str!("../testdata/nested_doubling.c"),
    },
    AllocationTestCase {
        name: "nested_doubling_shared_free",
        source: include_str!("../testdata/nested_doubling_shared_free.c"),
    },
];

fn assert_stack_snapshot(
    test_case: &AllocationTestCase,
    stack_capture: bool,
    snapshot_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let binary = shared::compile_c_source(test_case.source, test_case.name, temp_dir.path())?;
    let options = TrackerOptions::builder()
        .stack_capture(stack_capture)
        .build();
    let (events, thread_handle) = shared::track_command(Command::new(binary), options)?;

    insta::assert_debug_snapshot!(snapshot_name, format_events(&events));

    thread_handle
        .join()
        .expect("tracker teardown thread panicked");
    Ok(())
}

#[test_with::env(GITHUB_ACTIONS)]
#[rstest]
#[case(&STACK_TEST_CASES[0])]
#[case(&STACK_TEST_CASES[1])]
#[case(&STACK_TEST_CASES[2])]
#[test_log::test]
fn test_stack_capture(
    #[case] test_case: &AllocationTestCase,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_stack_snapshot(test_case, true, test_case.name)
}

#[test_with::env(GITHUB_ACTIONS)]
#[rstest]
#[case(&STACK_TEST_CASES[0])]
#[case(&STACK_TEST_CASES[1])]
#[case(&STACK_TEST_CASES[2])]
#[test_log::test]
fn test_stack_capture_disabled(
    #[case] test_case: &AllocationTestCase,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_stack_snapshot(
        test_case,
        false,
        &format!("{}_stack_capture_disabled", test_case.name),
    )
}
