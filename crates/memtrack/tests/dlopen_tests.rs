#[macro_use]
mod shared;

use runner_shared::artifacts::MemtrackEventKind;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn compile_shared(source: &str, name: &str, dir: &Path) -> PathBuf {
    let src = dir.join(format!("{name}.c"));
    std::fs::write(&src, source).expect("write source");
    let out = dir.join(format!("{name}.so"));
    let ok = Command::new("gcc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(&out)
        .arg(&src)
        .status()
        .expect("run gcc")
        .success();
    assert!(ok, "failed to compile {name}.so");
    out
}

fn compile_exe(source: &str, name: &str, dir: &Path, libs: &[&str]) -> PathBuf {
    let src = dir.join(format!("{name}.c"));
    std::fs::write(&src, source).expect("write source");
    let out = dir.join(name);
    let ok = Command::new("gcc")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .args(libs)
        .status()
        .expect("run gcc")
        .success();
    assert!(ok, "failed to compile {name}");
    out
}

/// dlopen -> fentry -> SIGSTOP -> resolve -> classify -> attach -> SIGCONT ->
/// first allocation captured. The dlopen'd allocator's 100 allocations must all
/// be captured, proving the attach completes before the child's first alloc.
#[test_with::env(GITHUB_ACTIONS)]
#[test_log::test]
fn test_dlopen_allocator() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let lib = compile_shared(
        include_str!("../testdata/dlopen/fake_mimalloc.c"),
        "libfake_mimalloc",
        dir.path(),
    );
    let exe = compile_exe(
        include_str!("../testdata/dlopen/dlopen_alloc.c"),
        "dlopen_alloc",
        dir.path(),
        &["-ldl"],
    );

    let mut cmd = Command::new(&exe);
    cmd.arg(&lib);
    let (events, thread_handle) = shared::track_command(cmd)?;

    let malloc_addrs: HashSet<u64> = events
        .iter()
        .filter_map(|e| match e.kind {
            MemtrackEventKind::Malloc { size: 4242 } => Some(e.addr),
            _ => None,
        })
        .collect();
    let malloc_count = events
        .iter()
        .filter(|e| matches!(e.kind, MemtrackEventKind::Malloc { size: 4242 }))
        .count();
    let free_count = events
        .iter()
        .filter(|e| matches!(e.kind, MemtrackEventKind::Free) && malloc_addrs.contains(&e.addr))
        .count();

    assert_eq!(malloc_count, 100, "expected 100 mi_malloc(4242) events");
    assert_eq!(
        free_count, 100,
        "expected 100 mi_free events for tracked addrs"
    );

    thread_handle.join().unwrap();
    Ok(())
}

/// Two threads dlopen distinct allocator libs concurrently. Both libs' 100
/// allocations each must be captured, exercising concurrent stop-the-world.
#[test_with::env(GITHUB_ACTIONS)]
#[test_log::test]
fn test_thread_dlopen() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let mimalloc = compile_shared(
        include_str!("../testdata/dlopen/fake_mimalloc.c"),
        "libfake_mimalloc",
        dir.path(),
    );
    let jemalloc = compile_shared(
        include_str!("../testdata/dlopen/fake_jemalloc.c"),
        "libfake_jemalloc",
        dir.path(),
    );
    let exe = compile_exe(
        include_str!("../testdata/dlopen/thread_dlopen.c"),
        "thread_dlopen",
        dir.path(),
        &["-ldl", "-lpthread"],
    );

    let mut cmd = Command::new(&exe);
    cmd.arg(&mimalloc).arg(&jemalloc);
    let (events, thread_handle) = shared::track_command(cmd)?;

    let m4242 = events
        .iter()
        .filter(|e| matches!(e.kind, MemtrackEventKind::Malloc { size: 4242 }))
        .count();
    let m4243 = events
        .iter()
        .filter(|e| matches!(e.kind, MemtrackEventKind::Malloc { size: 4243 }))
        .count();

    assert_eq!(m4242, 100, "expected 100 mi_malloc(4242) events");
    assert_eq!(m4243, 100, "expected 100 je_malloc(4243) events");

    thread_handle.join().unwrap();
    Ok(())
}
