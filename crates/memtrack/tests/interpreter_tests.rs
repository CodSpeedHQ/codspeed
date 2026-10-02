#[macro_use]
mod shared;

use memtrack::TrackerOptions;
use runner_shared::artifacts::{MemtrackEvent, MemtrackEventKind};
use std::path::Path;
use std::process::Command;

const ALLOCATION_SIZE: u64 = 2_000_001;

/// Find the fixture's allocation and return the pid that made it. The stack
/// must be captured, since offline attribution walks it through the JIT frame.
fn allocation_pid(events: &[MemtrackEvent]) -> libc::pid_t {
    let (pid, stack_hash) = events
        .iter()
        .find_map(|event| match event.kind {
            MemtrackEventKind::Malloc { size, stack_hash }
            | MemtrackEventKind::Calloc { size, stack_hash }
            | MemtrackEventKind::AlignedAlloc { size, stack_hash }
                if size == ALLOCATION_SIZE =>
            {
                Some((event.pid, stack_hash))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no {ALLOCATION_SIZE}-byte allocation was tracked"));
    assert_ne!(stack_hash, 0, "allocation has no captured stack");

    pid
}

/// The runtime wrote a perf map for the allocating process naming the
/// fixture's `allocation` function, which the runner harvests from /tmp.
fn assert_perf_map_symbol(pid: libc::pid_t, symbol: &str) {
    let path = format!("/tmp/perf-{pid}.map");
    let perf_map =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    let _ = std::fs::remove_file(&path);

    assert!(
        perf_map.lines().any(|line| line.contains(symbol)),
        "{path} has no `{symbol}` entry"
    );
}

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn stack_capture() -> TrackerOptions {
    TrackerOptions::builder().stack_capture(true).build()
}

#[test_with::env(GITHUB_ACTIONS)]
#[test_log::test]
fn test_python_allocation_has_jit_symbol() -> anyhow::Result<()> {
    let mut command = Command::new("python3");
    command.args(["-X", "perf", &fixture("python_alloc.py")]);
    let (events, thread_handle) = shared::track_command(command, stack_capture())?;

    assert_perf_map_symbol(allocation_pid(&events), "py::allocation:");

    thread_handle.join().unwrap();
    Ok(())
}

#[test_with::env(GITHUB_ACTIONS)]
#[test_log::test]
fn test_node_allocation_has_jit_symbol() -> anyhow::Result<()> {
    let mut command = Command::new("node");
    command.args([
        "--perf-basic-prof",
        "--interpreted-frames-native-stack",
        &fixture("node_alloc.js"),
    ]);
    let (events, thread_handle) = shared::track_command(command, stack_capture())?;

    assert_perf_map_symbol(allocation_pid(&events), "JS:~allocation ");

    thread_handle.join().unwrap();
    Ok(())
}
