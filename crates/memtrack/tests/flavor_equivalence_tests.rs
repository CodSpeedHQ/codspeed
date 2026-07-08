//! Verifies the two eBPF attach flavors produce equivalent tracking results.
//!
//! memtrack attaches its probes one of two ways: bpf()-based links
//! (`uprobe_multi` + `tp_btf`, [`Flavor::Token`]) that a delegated BPF token can
//! authorize inside the macro-agent sandbox, or classic perf-based
//! uprobe/tracepoint attach ([`Flavor::Perf`]) that works on kernels predating
//! `uprobe_multi` but cannot be delegated. Both are generated from the same BPF
//! source; only the attach mechanism differs. This test runs the same
//! deterministic workload through each and asserts the tracked allocations
//! match, so the fallback path can't silently diverge from the sandbox path.
//!
//! Attaching either flavor requires privilege (root or CAP_BPF/CAP_PERFMON);
//! the token flavor additionally needs `uprobe_multi` (kernel >= 6.6). The test
//! skips any path it cannot exercise on the current host rather than failing.

#[macro_use]
mod shared;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use memtrack::Flavor;
use runner_shared::artifacts::{MemtrackEvent as Event, MemtrackEventKind};
use tempfile::TempDir;

fn compile_c_source(source: &str, name: &str, out_dir: &Path) -> anyhow::Result<PathBuf> {
    let source_path = out_dir.join(format!("{name}.c"));
    let binary_path = out_dir.join(name);
    fs::write(&source_path, source)?;

    let output = Command::new("gcc")
        .args(["-O0", "-o", binary_path.to_str().unwrap()])
        .arg(&source_path)
        .output()?;
    if !output.status.success() {
        anyhow::bail!("gcc failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(binary_path)
}

/// A stable summary of the tracked allocations: how many events of each kind,
/// and the total bytes they account for. Independent of addresses, timestamps,
/// and event ordering, all of which legitimately differ run to run, so it is
/// safe to compare across two separate tracking runs.
type Summary = BTreeMap<String, (usize, u64)>;

fn summarize(events: &[Event]) -> Summary {
    let mut summary: Summary = BTreeMap::new();
    for event in events {
        let (kind, size) = match event.kind {
            MemtrackEventKind::Malloc { size } => ("Malloc", size),
            MemtrackEventKind::Free => ("Free", 0),
            MemtrackEventKind::Realloc { size, .. } => ("Realloc", size),
            MemtrackEventKind::Calloc { size } => ("Calloc", size),
            MemtrackEventKind::AlignedAlloc { size } => ("AlignedAlloc", size),
            MemtrackEventKind::Mmap { size } => ("Mmap", size),
            MemtrackEventKind::Munmap { size } => ("Munmap", size),
            MemtrackEventKind::Brk { size } => ("Brk", size),
        };
        let entry = summary.entry(kind.to_string()).or_insert((0, 0));
        entry.0 += 1;
        entry.1 += size;
    }
    summary
}

/// Track the workload under one flavor, returning the marker-isolated summary,
/// or `None` if this flavor cannot be attached on the current host (e.g. the
/// token flavor on a kernel without `uprobe_multi`).
fn summary_for_flavor(binary: &Path, flavor: Flavor) -> Option<Summary> {
    let command = Command::new(binary);
    match shared::track_command_with_flavor(command, &[], flavor) {
        Ok((events, handle)) => {
            handle.join().ok();
            let filtered = shared::between_markers(&events);
            assert!(
                !filtered.is_empty(),
                "{flavor:?}: no events captured between markers"
            );
            Some(summarize(&filtered))
        }
        Err(err) => {
            eprintln!("skipping {flavor:?} flavor: cannot attach on this host: {err:#}");
            None
        }
    }
}

#[test_with::env(GITHUB_ACTIONS)]
#[test_log::test]
fn both_flavors_track_equivalently() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    let binary = compile_c_source(
        include_str!("../testdata/flavor_equivalence.c"),
        "flavor_equivalence",
        temp_dir.path(),
    )?;

    let Some(perf) = summary_for_flavor(&binary, Flavor::Perf) else {
        // No privilege to attach at all — nothing to compare.
        eprintln!("skipping: perf flavor could not attach (need root or CAP_BPF/CAP_PERFMON)");
        return Ok(());
    };

    let Some(token) = summary_for_flavor(&binary, Flavor::Token) else {
        // Perf worked but the token flavor is unavailable (kernel < 6.6). The
        // perf fallback is still validated; there is nothing to compare against.
        return Ok(());
    };

    assert_eq!(
        perf, token,
        "perf and token flavors disagree on tracked allocations\nperf:  {perf:#?}\ntoken: {token:#?}"
    );

    // Both flavors agree, so a single snapshot pins down what the workload is
    // expected to produce and guards against both paths drifting together.
    insta::assert_debug_snapshot!("flavor_equivalence_summary", perf);

    Ok(())
}
