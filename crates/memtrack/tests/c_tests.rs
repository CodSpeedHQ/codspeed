#[macro_use]
mod shared;

use rstest::rstest;
use std::process::Command;
use tempfile::TempDir;

struct AllocationTestCase {
    name: &'static str,
    source: &'static str,
}

const ALLOCATION_TEST_CASES: &[AllocationTestCase] = &[
    AllocationTestCase {
        name: "double_malloc",
        source: include_str!("../testdata/double_malloc.c"),
    },
    AllocationTestCase {
        name: "malloc_free",
        source: include_str!("../testdata/malloc_free.c"),
    },
    AllocationTestCase {
        name: "calloc_test",
        source: include_str!("../testdata/calloc_test.c"),
    },
    AllocationTestCase {
        name: "realloc_test",
        source: include_str!("../testdata/realloc_test.c"),
    },
    AllocationTestCase {
        name: "aligned_alloc_test",
        source: include_str!("../testdata/aligned_alloc_test.c"),
    },
    AllocationTestCase {
        name: "many_allocs",
        source: include_str!("../testdata/many_allocs.c"),
    },
    AllocationTestCase {
        name: "fork_test",
        source: include_str!("../testdata/fork_test.c"),
    },
    AllocationTestCase {
        name: "alloc_size",
        source: include_str!("../testdata/alloc_size.c"),
    },
    AllocationTestCase {
        name: "posix_memalign_test",
        source: include_str!("../testdata/posix_memalign_test.c"),
    },
    AllocationTestCase {
        name: "posix_memalign_einval",
        source: include_str!("../testdata/posix_memalign_einval.c"),
    },
];

#[test_with::env(GITHUB_ACTIONS)]
#[rstest]
#[case(&ALLOCATION_TEST_CASES[0])]
#[case(&ALLOCATION_TEST_CASES[1])]
#[case(&ALLOCATION_TEST_CASES[2])]
#[case(&ALLOCATION_TEST_CASES[3])]
#[case(&ALLOCATION_TEST_CASES[4])]
#[case(&ALLOCATION_TEST_CASES[5])]
#[case(&ALLOCATION_TEST_CASES[6])]
#[case(&ALLOCATION_TEST_CASES[7])]
#[case(&ALLOCATION_TEST_CASES[8])]
#[case(&ALLOCATION_TEST_CASES[9])]
#[test_log::test]
fn test_allocation_tracking(
    #[case] test_case: &AllocationTestCase,
) -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let binary = shared::compile_c_source(test_case.source, test_case.name, temp_dir.path())?;

    assert_events_snapshot_for_each_variant!(test_case.name, || Command::new(&binary))?;

    Ok(())
}

#[test_with::env(GITHUB_ACTIONS)]
#[test_log::test]
fn test_track_allocators_disabled_skips_allocations() -> Result<(), Box<dyn std::error::Error>> {
    use runner_shared::artifacts::MemtrackEventKind;

    let temp_dir = TempDir::new()?;
    let binary = shared::compile_c_source(
        include_str!("../testdata/malloc_mmap.c"),
        "malloc_mmap",
        temp_dir.path(),
    )?;

    let (events, thread_handle) = shared::track_command_with_opts(
        std::process::Command::new(&binary),
        memtrack::TrackerOptions::builder()
            .allocators(false)
            .build(),
    )?;

    let has_mmap = events
        .iter()
        .any(|e| matches!(e.kind, MemtrackEventKind::Mmap { .. }));
    assert!(
        has_mmap,
        "expected at least one Mmap event with allocators disabled"
    );

    let alloc_events: Vec<_> = events
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
        .collect();
    assert!(
        alloc_events.is_empty(),
        "expected no allocation events with allocators disabled, got {alloc_events:?}"
    );

    thread_handle.join().unwrap();
    Ok(())
}
