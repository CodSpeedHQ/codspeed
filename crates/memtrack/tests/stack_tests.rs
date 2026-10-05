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
        MemtrackEventKind::Free => "Free".to_string(),
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
                        | MemtrackEventKind::Free
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

/// A hooked allocator calling another hooked allocator (`operator new` →
/// `malloc`) must restore the return address that the uretprobe trampoline
/// replaced in the captured stack bytes: offline DWARF unwinding reads the
/// trampoline as a return address and stops.
///
/// Only the fixture's own allocations are checked: stale trampoline words can
/// linger in reused stack memory of unrelated libc calls, where they are not
/// return addresses and unwinding never reads them.
///
/// x86_64 only: memtrack cannot restore the hijacked link register on arm64.
#[cfg(target_arch = "x86_64")]
#[test_with::env(GITHUB_ACTIONS)]
#[test_log::test]
fn test_nested_allocator_stack_has_no_uretprobe_trampoline()
-> Result<(), Box<dyn std::error::Error>> {
    use object::{Object, ObjectSymbol};

    const NAME: &str = "nested_operator_new";
    /// Fixture function that calls `operator new`.
    const CALLER: &str = "allocate";
    const SOURCE: &str = include_str!("../testdata/nested_operator_new.cpp");
    /// `sizeof(Payload)` in the fixture.
    const PAYLOAD_SIZE: u64 = 0x2A5;

    let temp_dir = TempDir::new()?;
    let source_path = temp_dir.path().join(format!("{NAME}.cpp"));
    let binary = temp_dir.path().join(NAME);
    let trampoline_path = temp_dir.path().join("uprobes_start");
    std::fs::write(&source_path, SOURCE)?;
    // No PIE, so the symbol table gives runtime addresses.
    let compile = Command::new("g++")
        .args(["-O0", "-no-pie", "-o"])
        .arg(&binary)
        .arg(&source_path)
        .output()?;
    assert!(
        compile.status.success(),
        "g++ failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let elf = std::fs::read(&binary)?;
    let caller = object::File::parse(&*elf)?
        .symbols()
        .find(|symbol| symbol.name() == Ok(CALLER))
        .map(|symbol| symbol.address()..symbol.address() + symbol.size())
        .ok_or_else(|| format!("fixture has no {CALLER} symbol"))?;

    let options = TrackerOptions::builder().stack_capture(true).build();
    let mut command = Command::new(&binary);
    command.arg(&trampoline_path);
    let (events, thread_handle) = shared::track_command(command, options)?;

    let trampoline = u64::from_str_radix(std::fs::read_to_string(&trampoline_path)?.trim(), 16)?;
    assert_ne!(trampoline, 0, "fixture found no [uprobes] mapping");

    let payload_hashes: std::collections::HashSet<u64> = events
        .iter()
        .filter_map(|event| match event.kind {
            MemtrackEventKind::Malloc { size, stack_hash } if size == PAYLOAD_SIZE => {
                Some(stack_hash)
            }
            _ => None,
        })
        .collect();
    assert!(
        !payload_hashes.is_empty() && !payload_hashes.contains(&0),
        "every payload allocation needs a stack: {payload_hashes:x?}"
    );

    let payload_stacks = events
        .iter()
        .filter_map(|event| match &event.kind {
            MemtrackEventKind::Stack { record } if payload_hashes.contains(&record.hash) => {
                Some(record)
            }
            _ => None,
        })
        .collect_vec();
    assert!(!payload_stacks.is_empty(), "no payload stacks captured");

    let payload_words = payload_stacks
        .iter()
        .map(|record| {
            record
                .bytes
                .chunks_exact(size_of::<u64>())
                .map(|word| u64::from_le_bytes(word.try_into().unwrap()))
                .collect_vec()
        })
        .collect_vec();
    let poisoned = payload_words
        .iter()
        .filter(|words| words.contains(&trampoline))
        .count();
    assert_eq!(
        poisoned,
        0,
        "{poisoned}/{} payload stacks contain the uretprobe trampoline {trampoline:#x}",
        payload_stacks.len()
    );

    // The restored word must be the real return address, not just any value.
    let missing_caller = payload_words
        .iter()
        .filter(|words| !words.iter().any(|word| caller.contains(word)))
        .count();
    assert_eq!(
        missing_caller,
        0,
        "{missing_caller}/{} payload stacks lack a return address into {CALLER} {caller:#x?}",
        payload_stacks.len()
    );

    thread_handle
        .join()
        .expect("tracker teardown thread panicked");
    Ok(())
}
