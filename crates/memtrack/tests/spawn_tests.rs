#[macro_use]
mod shared;

use std::path::Path;
use std::process::Command;

fn compile_spawn_binary(name: &str) -> anyhow::Result<std::path::PathBuf> {
    shared::compile_rust_binary(Path::new("testdata/spawn_wrapper"), name, &[])
}

/// The exec-mapping watcher discovers the statically-linked jemalloc in the
/// spawned grandchild binary automatically: no pre-attached allocators. This is
/// the COD-1801 core ask, and it exercises the watcher firing on execve.
#[test_with::env(GITHUB_ACTIONS)]
#[test_log::test]
fn test_spawn_static_allocator_discovery() -> Result<(), Box<dyn std::error::Error>> {
    let wrapper = compile_spawn_binary("wrapper")?;
    let child = compile_spawn_binary("alloc_child")?;

    let mut cmd = Command::new(&wrapper);
    cmd.arg(&child);
    let (events, thread_handle) = shared::track_command(cmd)?;

    assert_events_with_marker!("spawn_on_demand_discovery", &events);

    thread_handle.join().unwrap();
    Ok(())
}
