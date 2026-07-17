#[macro_use]
mod shared;

use itertools::Itertools;
use rstest::rstest;
use runner_shared::artifacts::{MemtrackEvent, MemtrackEventKind};
use serde::Serialize;
use std::collections::BTreeMap;
use std::process::Command;
use tempfile::TempDir;

const MIB: u64 = 1024 * 1024;

fn mib_16(bytes: u64) -> u64 {
    (bytes + 8 * MIB) / (16 * MIB) * 16
}

fn parse_report(report: &str) -> BTreeMap<String, u64> {
    report
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let key = parts.next()?.to_string();
            let kb: u64 = parts.next()?.parse().ok()?;
            Some((key, mib_16(kb * 1024)))
        })
        .collect()
}

#[derive(Debug, Serialize)]
struct PidRss {
    pid: i32,
    file_mib: u64,
    anon_mib: u64,
    shmem_mib: u64,
    max_rss_mib: u64,
}

#[derive(Serialize)]
struct RssSummary {
    report: BTreeMap<String, u64>,
    rss_stat: Vec<PidRss>,
    rmap: Vec<PidRss>,
}

#[derive(Default)]
struct PeakAccum {
    latest: [i64; 4],
    peaks: [i64; 4],
    max_rss: i64,
}

impl PeakAccum {
    /// Absolute assignment (rss_stat: the kernel counter's current value).
    fn set(&mut self, index: usize, bytes: i64) {
        self.latest[index] = bytes;
        self.update_peaks();
    }

    /// Delta accumulation (rmap: summed folio add/remove deltas from zero).
    fn add(&mut self, index: usize, delta_bytes: i64) {
        self.latest[index] += delta_bytes;
        self.update_peaks();
    }

    /// Fork: child inherits the parent's current resident values.
    fn seed(&mut self, latest: [i64; 4]) {
        self.latest = latest;
        self.update_peaks();
    }

    /// Exec/Exit: reset the running value only; recorded peaks are retained
    /// (an absolute rss_stat re-syncs on the next event; rmap re-accumulates).
    fn reset(&mut self) {
        self.latest = [0; 4];
    }

    fn update_peaks(&mut self) {
        for (peak, latest) in self.peaks.iter_mut().zip(self.latest) {
            *peak = (*peak).max(latest);
        }
        self.max_rss = self
            .max_rss
            .max(self.latest[0] + self.latest[1] + self.latest[3]);
    }
}

/// Reduce the raw event stream to per-pid resident peaks in BYTES.
/// Returns first-activity pid order plus rss_stat and rmap accumulators.
fn per_pid_raw(
    events: &[MemtrackEvent],
) -> (Vec<i32>, BTreeMap<i32, PeakAccum>, BTreeMap<i32, PeakAccum>) {
    // Pid values can wrap, so numeric order is not stable; both views emit
    // their rows in one shared first-activity order, keeping the relative
    // order of processes consistent between `rss_stat` and `rmap` after
    // the pids are redacted.
    let mut order: Vec<i32> = Vec::new();
    let mut rss: BTreeMap<i32, PeakAccum> = BTreeMap::new();
    let mut rmap: BTreeMap<i32, PeakAccum> = BTreeMap::new();

    fn seen(order: &mut Vec<i32>, pid: i32) {
        if !order.contains(&pid) {
            order.push(pid);
        }
    }

    for event in events.iter().sorted_by_key(|event| event.timestamp) {
        match event.kind {
            MemtrackEventKind::Rss { member, size } => {
                let Ok(index @ 0..4) = usize::try_from(member) else {
                    continue;
                };
                seen(&mut order, event.pid);
                rss.entry(event.pid).or_default().set(index, size as i64);
            }
            MemtrackEventKind::Rmap { member, delta } => {
                let Ok(index @ 0..4) = usize::try_from(member) else {
                    continue;
                };
                seen(&mut order, event.pid);
                rmap.entry(event.pid)
                    .or_default()
                    .add(index, delta * page_size() as i64);
            }
            MemtrackEventKind::Fork { parent_pid } => {
                seen(&mut order, event.pid);
                let seed = rss.get(&parent_pid).map(|p| p.latest).unwrap_or_default();
                rss.entry(event.pid).or_default().seed(seed);
                let seed = rmap.get(&parent_pid).map(|p| p.latest).unwrap_or_default();
                rmap.entry(event.pid).or_default().seed(seed);
            }
            MemtrackEventKind::Exec | MemtrackEventKind::Exit => {
                if let Some(acc) = rss.get_mut(&event.pid) {
                    acc.reset();
                }
                if let Some(acc) = rmap.get_mut(&event.pid) {
                    acc.reset();
                }
            }
            _ => {}
        }
    }
    (order, rss, rmap)
}

fn per_pid_peaks(events: &[MemtrackEvent]) -> (Vec<PidRss>, Vec<PidRss>) {
    let (order, rss, rmap) = per_pid_raw(events);
    let project = |map: &BTreeMap<i32, PeakAccum>| -> Vec<PidRss> {
        order
            .iter()
            .filter_map(|pid| {
                let acc = map.get(pid)?;
                Some(PidRss {
                    pid: *pid,
                    file_mib: mib_16(acc.peaks[0].max(0) as u64),
                    anon_mib: mib_16(acc.peaks[1].max(0) as u64),
                    shmem_mib: mib_16(acc.peaks[3].max(0) as u64),
                    max_rss_mib: mib_16(acc.max_rss.max(0) as u64),
                })
            })
            .collect()
    };
    (project(&rss), project(&rmap))
}

/// Compile a fixture that writes a `/proc` RSS report to its argv[1], run it under
/// `track`, and return the raw report text alongside the collected events.
///
/// The report read is best-effort: some fixtures write no report.
fn track_fixture(
    source: &str,
    name: &str,
    track: impl FnOnce(Command) -> shared::TrackResult,
) -> Result<(Option<String>, Vec<MemtrackEvent>), Box<dyn std::error::Error>> {
    // Fixtures mmap data files created next to the report path, so the temp dir
    // must be disk-backed: /tmp may be tmpfs (Ubuntu >= 25.04), which accounts
    // mapped file pages as shmem instead of file.
    std::fs::create_dir_all(env!("CARGO_TARGET_TMPDIR"))?;
    let temp_dir = TempDir::new_in(env!("CARGO_TARGET_TMPDIR"))?;
    std::fs::write(
        temp_dir.path().join("rss_report.h"),
        include_str!("../testdata/rss/rss_report.h"),
    )?;
    let binary = shared::compile_c_source(source, name, temp_dir.path())?;
    let report_path = temp_dir.path().join(format!("{name}.report"));
    let mut command = Command::new(&binary);
    command.arg(&report_path);

    let (events, thread_handle) = track(command)?;
    let raw_report = std::fs::read_to_string(&report_path).ok();
    thread_handle.join().unwrap();
    Ok((raw_report, events))
}

/// The first fork observed: `(parent_pid, child_pid)`.
fn first_fork_pair(events: &[MemtrackEvent]) -> Option<(i32, i32)> {
    events.iter().find_map(|e| match e.kind {
        MemtrackEventKind::Fork { parent_pid } => Some((parent_pid, e.pid)),
        _ => None,
    })
}

/// Pins reconstructed rmap addresses to the exact punched range: hole pages are
/// the only ones removed and later re-added; every other page in the region is
/// added first and only removed afterwards.
fn assert_rmap_hole_addresses(
    events: &[MemtrackEvent],
    base: u64,
    hole_off: u64,
    hole_len: u64,
    len: u64,
) {
    const PAGE: u64 = 4096;
    let n_pages = (len / PAGE) as usize;
    let mut first_remove = vec![u64::MAX; n_pages];
    let mut added = vec![false; n_pages];
    let mut readded = vec![false; n_pages];

    for event in events.iter().sorted_by_key(|e| e.timestamp) {
        let MemtrackEventKind::Rmap { delta, .. } = event.kind else {
            continue;
        };
        if event.addr < base || event.addr >= base + len {
            continue;
        }
        let first = ((event.addr - base) / PAGE) as usize;
        let last = (first + delta.unsigned_abs() as usize).min(n_pages);
        for page in first..last {
            if delta > 0 {
                added[page] = true;
                if event.timestamp > first_remove[page] {
                    readded[page] = true;
                }
            } else {
                first_remove[page] = first_remove[page].min(event.timestamp);
            }
        }
    }

    let hole = (hole_off / PAGE) as usize..((hole_off + hole_len) / PAGE) as usize;
    for page in 0..n_pages {
        assert!(added[page], "page {page} never saw an rmap add");
        assert_eq!(
            readded[page],
            hole.contains(&page),
            "page {page}: remove-then-add pattern does not match the hole range"
        );
    }
}

#[test_with::env(GITHUB_ACTIONS)]
#[rstest]
#[case::anon(include_str!("../testdata/rss/anon.c"), "anon")]
#[case::file(include_str!("../testdata/rss/file.c"), "file")]
#[case::shmem(include_str!("../testdata/rss/shmem.c"), "shmem")]
#[case::fork(include_str!("../testdata/rss/fork.c"), "fork")]
#[case::fork_idle(include_str!("../testdata/rss/fork_idle.c"), "fork_idle")]
#[case::triangle(include_str!("../testdata/rss/triangle.c"), "triangle")]
#[case::madvise(include_str!("../testdata/rss/madvise.c"), "madvise")]
#[case::munmap_hole(include_str!("../testdata/rss/munmap_hole.c"), "munmap_hole")]
#[case::mremap_move(include_str!("../testdata/rss/mremap_move.c"), "mremap_move")]
fn test_rss_rmap_tracking(
    #[case] source: &str,
    #[case] name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let (raw_report, events) = track_fixture(source, name, shared::track_command_with_rmap)?;
    let raw_report = raw_report.ok_or("fixture wrote no rss report")?;
    let (rss_stat, rmap) = per_pid_peaks(&events);
    let summary = RssSummary {
        report: parse_report(&raw_report),
        rss_stat,
        rmap,
    };
    insta::assert_json_snapshot!(format!("rss_{name}"), summary, {
        ".rss_stat[].pid" => "[pid]",
        ".rmap[].pid" => "[pid]",
    });

    if let Some(layout) = raw_report
        .lines()
        .find_map(|line| line.strip_prefix("Layout:"))
    {
        let values: Vec<u64> = layout
            .split_whitespace()
            .map(|token| u64::from_str_radix(token.trim_start_matches("0x"), 16))
            .collect::<Result<_, _>>()?;
        let [base, hole_off, hole_len, len] = values[..] else {
            panic!("malformed Layout line: {layout}");
        };
        assert_rmap_hole_addresses(&events, base, hole_off, hole_len, len);
    }

    // ThpKb > 0 means the MADV_HUGEPAGE region really faulted as PMD folios, so
    // huge-folio accounting must be visible: a +512-page delta from the new-anon
    // fault path and a -512-page delta that can only come from the
    // folio_remove_rmap_pmd hook (MADV_DONTNEED / munmap of a pmd-mapped THP).
    if let Some(thp) = raw_report
        .lines()
        .find_map(|line| line.strip_prefix("ThpKb:"))
    {
        let thp_kb = u64::from_str_radix(thp.trim().trim_start_matches("0x"), 16)?;
        if thp_kb > 0 {
            let deltas = events.iter().filter_map(|e| match e.kind {
                MemtrackEventKind::Rmap { delta, .. } => Some(delta),
                _ => None,
            });
            let (mut huge_add, mut huge_remove) = (false, false);
            for delta in deltas {
                huge_add |= delta >= 512;
                huge_remove |= delta <= -512;
            }
            assert!(
                huge_add,
                "THP present ({thp_kb} kB) but no huge-folio rmap add"
            );
            assert!(
                huge_remove,
                "THP present ({thp_kb} kB) but no pmd-sized rmap remove"
            );
        }
    }
    Ok(())
}

#[test_with::env(GITHUB_ACTIONS)]
#[test]
fn test_rss_external_reclaim() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let binary = shared::compile_c_source(
        include_str!("../testdata/rss/madvise_extern.c"),
        "madvise_extern",
        temp_dir.path(),
    )?;
    let (events, handle) = shared::track_command(Command::new(&binary))?;
    handle.join().unwrap();

    // Single fork: parent_pid == A (owner), event.pid == B (external caller, single-threaded
    // so its tid == its pid).
    let (a, b) = events
        .iter()
        .find_map(|e| match e.kind {
            MemtrackEventKind::Fork { parent_pid } => Some((parent_pid, e.pid)),
            _ => None,
        })
        .expect("expected a fork event");

    let peak = events
        .iter()
        .filter_map(|e| match e.kind {
            MemtrackEventKind::Rss { member: 0, size } if e.pid == a => Some(size),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    assert!(peak >= 32 * MIB, "peak file RSS too small: {peak}");

    // A file decrement owned by A but emitted from B's context: only present when
    // out-of-context rss_stat updates are attributed to the owning process.
    let external_decrement = events.iter().any(|e| {
        e.pid == a
            && e.tid == b
            && matches!(e.kind, MemtrackEventKind::Rss { member: 0, size } if size < peak)
    });
    assert!(
        external_decrement,
        "external file-RSS decrement not attributed to A (tid=B)"
    );
    Ok(())
}

/// TEMPORARY diagnostic: track a real-world workload (`ls /nix/store`, ~50 MiB
/// peak on a populated store) and cross-check the reconstructed rss_stat and
/// rmap peaks against the kernel's own accounting (`wait4` ru_maxrss) from an
/// identical untracked run. `ls` is single-process, so raw byte peaks are
/// accumulated directly without per-pid splitting.
#[test_with::env(GITHUB_ACTIONS)]
#[test]
fn test_rss_ls_nix_store() -> Result<(), Box<dyn std::error::Error>> {
    if !std::path::Path::new("/nix/store").is_dir() {
        eprintln!("skipping: /nix/store not available");
        return Ok(());
    }

    let ls_command = || {
        let mut cmd = Command::new("ls");
        cmd.arg("/nix/store").stdout(std::process::Stdio::null());
        cmd
    };

    // Ground truth: identical untracked run, reaped via wait4 for ru_maxrss.
    let child = ls_command().spawn()?;
    let pid = child.id() as i32;
    let mut status = 0i32;
    let mut rusage: libc::rusage = unsafe { std::mem::zeroed() };
    let reaped = unsafe { libc::wait4(pid, &mut status, 0, &mut rusage) };
    assert_eq!(reaped, pid, "wait4 failed");
    assert!(
        libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
        "untracked ls failed: status {status}"
    );
    let truth_bytes = rusage.ru_maxrss as u64 * 1024;

    let (events, handle) = shared::track_command_with_rmap(ls_command())?;
    handle.join().unwrap();

    let mut rss = RssAccum::default();
    let mut rmap = RmapAccum::default();
    for event in events.iter().sorted_by_key(|event| event.timestamp) {
        match event.kind {
            MemtrackEventKind::Rss { member, size } => {
                if let Ok(index @ 0..4) = usize::try_from(member) {
                    rss.latest[index] = size;
                    rss.update_peaks();
                }
            }
            MemtrackEventKind::Rmap { member, delta } => {
                if let Ok(index @ 0..4) = usize::try_from(member) {
                    rmap.totals[index] += delta * 4096;
                    rmap.update_peaks();
                }
            }
            _ => {}
        }
    }

    let rss_bytes = rss.max_rss;
    let rmap_bytes = rmap.max_rss.max(0) as u64;
    eprintln!(
        "ls /nix/store max RSS: wait4={:.1} MiB rss_stat={:.1} MiB rmap={:.1} MiB",
        truth_bytes as f64 / MIB as f64,
        rss_bytes as f64 / MIB as f64,
        rmap_bytes as f64 / MIB as f64,
    );

    let within = |measured: u64| {
        (truth_bytes as f64 * 0.8..=truth_bytes as f64 * 1.2).contains(&(measured as f64))
    };
    assert!(
        within(rss_bytes),
        "rss_stat peak {rss_bytes} outside 20% of wait4 {truth_bytes}"
    );
    assert!(
        within(rmap_bytes),
        "rmap peak {rmap_bytes} outside 20% of wait4 {truth_bytes}"
    );
    Ok(())
}

/// rss_stat is an absolute kernel counter; the rmap estimate is reconstructed
/// from zero by summing folio add/remove deltas. When tracking is enabled only
/// after a process has already faulted a resident region, rss_stat's first
/// reading includes that region but the rmap accumulator never saw its adds, so
/// rmap sits a fixed offset (the pre-enable resident set) below rss_stat.
///
/// The fixture faults a 64 MiB anon baseline before `enable_tracking`, then a
/// 64 MiB anon region after. Reduced with the same Exec-reset lifecycle the
/// production parser uses, rss_stat peaks at ~128 MiB (absolute) while rmap
/// peaks at ~64 MiB (post-enable growth only).
#[test_with::env(GITHUB_ACTIONS)]
#[test]
fn test_rss_rmap_late_enable_baseline_loss() -> Result<(), Box<dyn std::error::Error>> {
    const REGION_MIB: u64 = 64;

    let temp_dir = TempDir::new()?;
    std::fs::write(
        temp_dir.path().join("rss_report.h"),
        include_str!("../testdata/rss/rss_report.h"),
    )?;
    let binary = shared::compile_c_source(
        include_str!("../testdata/rss/rmap_late_enable.c"),
        "rmap_late_enable",
        temp_dir.path(),
    )?;
    let report_path = temp_dir.path().join("rmap_late_enable.report");
    let ready_path = temp_dir.path().join("ready");
    let go_path = temp_dir.path().join("go");

    let mut command = Command::new(&binary);
    command.arg(&report_path).arg(&ready_path).arg(&go_path);

    let (events, handle) =
        shared::track_command_with_rmap_late_enable(command, &ready_path, &go_path)?;
    handle.join().unwrap();

    let (rss_stat, rmap) = per_pid_peaks(&events);
    let rss = rss_stat.first().ok_or("no rss_stat pid observed")?;
    let rmap = rmap.first().ok_or("no rmap pid observed")?;
    eprintln!(
        "late-enable anon peaks: rss_stat={} MiB rmap={} MiB (region={} MiB each)",
        rss.anon_mib, rmap.anon_mib, REGION_MIB
    );

    // rss_stat's absolute counter covers baseline + growth.
    assert!(
        rss.anon_mib >= 2 * REGION_MIB - 16,
        "rss_stat anon peak {} MiB below the expected ~{} MiB baseline+growth",
        rss.anon_mib,
        2 * REGION_MIB
    );
    // Post-enable growth is visible to rmap.
    assert!(
        rmap.anon_mib >= REGION_MIB - 16,
        "rmap anon peak {} MiB too small; post-enable growth should be tracked",
        rmap.anon_mib
    );
    // The reproduction: rmap misses the pre-enable baseline, undercounting
    // rss_stat by roughly one region.
    let gap = rss.anon_mib.saturating_sub(rmap.anon_mib);
    assert!(
        gap >= REGION_MIB - 16,
        "expected rmap to undercount rss_stat by ~{} MiB (pre-enable baseline loss); gap was {} MiB",
        REGION_MIB,
        gap
    );
    Ok(())
}
