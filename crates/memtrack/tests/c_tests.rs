#[macro_use]
mod shared;

use rstest::rstest;
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// Compiles C source code and returns the binary path
fn compile_c_source(
    source_code: &str,
    name: &str,
    output_dir: &Path,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let source_path = output_dir.join(format!("{name}.c"));
    let binary_path = output_dir.join(name);

    fs::write(&source_path, source_code)?;

    let output = Command::new("gcc")
        .args(["-o", binary_path.to_str().unwrap()])
        .arg(&source_path)
        .output()?;

    if !output.status.success() {
        eprintln!("gcc stderr: {}", String::from_utf8_lossy(&output.stderr));
        return Err("Failed to compile C fixture".into());
    }

    Ok(binary_path)
}

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
#[test_log::test]
fn test_allocation_tracking(
    #[case] test_case: &AllocationTestCase,
) -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let binary = compile_c_source(test_case.source, test_case.name, temp_dir.path())?;

    let (events, thread_handle) = shared::track_binary(&binary)?;

    assert_events_snapshot!(test_case.name, events);

    thread_handle.join().unwrap();

    Ok(())
}
