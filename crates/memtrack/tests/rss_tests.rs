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

fn page_size() -> u64 {
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    assert!(size > 0, "sysconf(_SC_PAGESIZE) failed");
    size as u64
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
    let temp_dir = TempDir::new()?;
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
///
/// Only own-context events (tid == pid; the fixture is single-threaded) are
/// considered: foreign actors like kcompactd migrating a page produce a
/// remove-then-add on arbitrary pages, which the attribution of foreign rmap
/// events makes visible here.
fn assert_rmap_hole_addresses(
    events: &[MemtrackEvent],
    base: u64,
    hole_off: u64,
    hole_len: u64,
    len: u64,
) {
    let page = page_size();
    let n_pages = (len / page) as usize;
    let mut first_remove = vec![u64::MAX; n_pages];
    let mut added = vec![false; n_pages];
    let mut readded = vec![false; n_pages];

    for event in events.iter().sorted_by_key(|e| e.timestamp) {
        let MemtrackEventKind::Rmap { delta, .. } = event.kind else {
            continue;
        };
        if event.tid != event.pid {
            continue;
        }
        if event.addr < base || event.addr >= base + len {
            continue;
        }
        let first = ((event.addr - base) / page) as usize;
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

    let hole = (hole_off / page) as usize..((hole_off + hole_len) / page) as usize;
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

/// A foreign actor reclaiming another process's memory must be attributed to
/// the OWNER of that memory, not the actor. The fixture forks a child B that
/// calls process_madvise(MADV_PAGEOUT) against parent A's file region from B's
/// own context (tid == B). Both accounting modes must show A's in-context faults
/// AND the foreign reclaim charged back to A via the mm_owner map.
enum Reclaim {
    /// Absolute file-RSS updates; a foreign reclaim appears as a decrement.
    RssStat,
    /// Reconstructed file-page deltas; a foreign reclaim appears as removes.
    Rmap,
}

#[test_with::env(GITHUB_ACTIONS)]
#[rstest]
#[case::rss_stat(Reclaim::RssStat)]
#[case::rmap(Reclaim::Rmap)]
fn test_rss_external_reclaim(#[case] mode: Reclaim) -> Result<(), Box<dyn std::error::Error>> {
    let track: fn(Command) -> shared::TrackResult = match mode {
        Reclaim::RssStat => shared::track_command,
        Reclaim::Rmap => shared::track_command_with_rmap,
    };
    let (_report, events) = track_fixture(
        include_str!("../testdata/rss/madvise_extern.c"),
        "madvise_extern",
        track,
    )?;

    // A = owner that faulted the file region; B = external caller, single-threaded
    // so its tid == its pid.
    let (a, b) = first_fork_pair(&events).expect("expected a fork event");

    match mode {
        Reclaim::RssStat => {
            let peak = events
                .iter()
                .filter_map(|e| match e.kind {
                    MemtrackEventKind::Rss { member: 0, size } if e.pid == a => Some(size),
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            assert!(peak >= 32 * MIB, "peak file RSS too small: {peak}");

            // A file decrement owned by A but emitted from B's context: only present
            // when out-of-context rss_stat updates are attributed to the owner.
            let external_decrement = events.iter().any(|e| {
                e.pid == a
                    && e.tid == b
                    && matches!(e.kind, MemtrackEventKind::Rss { member: 0, size } if size < peak)
            });
            assert!(
                external_decrement,
                "external file-RSS decrement not attributed to A (tid=B)"
            );
        }
        Reclaim::Rmap => {
            let in_context_bytes = events
                .iter()
                .filter_map(|e| match e.kind {
                    MemtrackEventKind::Rmap { member: 0, delta } if e.pid == a && delta > 0 => {
                        Some(delta)
                    }
                    _ => None,
                })
                .sum::<i64>() as u64
                * page_size();
            assert!(
                in_context_bytes >= 32 * MIB,
                "in-context file rmap adds too small: {in_context_bytes}"
            );

            // A file-page remove owned by A but emitted from B's context (tid == B),
            // only present when the foreign reclaim's rmap events are attributed to
            // the owner via the mm_owner map.
            let external_bytes = events
                .iter()
                .filter_map(|e| match e.kind {
                    MemtrackEventKind::Rmap { member: 0, delta }
                        if e.pid == a && e.tid == b && delta < 0 =>
                    {
                        Some(-delta)
                    }
                    _ => None,
                })
                .sum::<i64>() as u64
                * page_size();
            assert!(
                external_bytes >= 8 * MIB,
                "external MADV_PAGEOUT remove not attributed to the owner (pid=A, tid=B): only {external_bytes} bytes"
            );
        }
    }
    Ok(())
}

/// An rss_stat ownership registration is keyed on the kernel's `mm_id` hash,
/// which outlives the mm it was seeded from: once the owner execs, the freed
/// `mm_struct` can be recycled by an unrelated fork whose near-zero counters
/// hash to the same id. Those counters must be dropped rather than emitted as
/// the live owner's absolute RSS collapsing to ~0.
#[test_with::env(GITHUB_ACTIONS)]
#[test]
fn test_rss_stale_mm_owner_keeps_live_rss() -> Result<(), Box<dyn std::error::Error>> {
    let (_report, events) = track_fixture(
        include_str!("../testdata/rss/stale_mm_owner.c"),
        "stale_mm_owner",
        shared::track_command_with_rmap,
    )?;

    // The fixture's only fork before the burst is the child that execs into the
    // memory-holding image, so it keeps its pid across the exec.
    let (_parent, big) = first_fork_pair(&events).ok_or("no fork event")?;
    let exec_ts = events
        .iter()
        .find(|e| e.pid == big && matches!(e.kind, MemtrackEventKind::Exec))
        .map(|e| e.timestamp)
        .ok_or("no exec event for the memory-holding child")?;

    // Only the post-exec address space is of interest: the pre-exec image
    // faulted a single page, and that mm is the one whose slab slot gets reused.
    let anon_after_exec: Vec<_> = events
        .iter()
        .filter_map(|e| match e.kind {
            MemtrackEventKind::Rss { member: 1, size } if e.pid == big && e.timestamp > exec_ts => {
                Some((e, size))
            }
            _ => None,
        })
        .collect();

    let (peak_ts, peak) = anon_after_exec
        .iter()
        .max_by_key(|&&(_, size)| size)
        .map(|&(e, size)| (e.timestamp, size))
        .ok_or("no anon rss_stat event after the exec")?;
    assert!(peak >= 128 * MIB, "peak anon RSS too small: {peak}");

    // Past the peak the region is held untouched until the fixture is killed, so
    // no legitimate absolute sample may fall back near zero. Before it, samples
    // are just the fault-in ramp.
    let collapsed: Vec<_> = anon_after_exec
        .iter()
        .filter(|&&(e, size)| e.timestamp > peak_ts && size < peak / 4)
        .map(|&(e, size)| (e.tid, size))
        .collect();
    assert!(
        collapsed.is_empty(),
        "{} absolute anon samples for pid {big} collapsed below {} bytes while it held {peak}: \
         (tid, size) = {:?}",
        collapsed.len(),
        peak / 4,
        &collapsed[..collapsed.len().min(5)]
    );

    // The rmap-reconstructed total confirms the pages stayed resident the whole
    // time, so a collapsing absolute sample could only come from a recycled mm.
    let rmap_net = events
        .iter()
        .filter_map(|e| match e.kind {
            MemtrackEventKind::Rmap { member: 1, delta } if e.pid == big => Some(delta),
            _ => None,
        })
        .sum::<i64>()
        * page_size() as i64;
    assert!(
        rmap_net >= (peak / 2) as i64,
        "fixture never held the region per rmap: net {rmap_net} bytes"
    );
    Ok(())
}

/// A fork issued by a worker thread must still track the child: registration
/// keys on the parent's tgid (task_newtask fires in the cloning task, whose
/// pid_tgid upper half is the tgid), not on the raw creator tid.
#[test_with::env(GITHUB_ACTIONS)]
#[test]
fn test_rss_rmap_thread_fork_tracks_child() -> Result<(), Box<dyn std::error::Error>> {
    const REGION_MIB: u64 = 64;

    let (_raw_report, events) = track_fixture(
        include_str!("../testdata/rss/rmap_thread_fork.c"),
        "rmap_thread_fork",
        shared::track_command_with_rmap,
    )?;

    // Single fork in the fixture: parent = the fixture process (tgid), child =
    // the region-faulting process.
    let (parent, child) =
        first_fork_pair(&events).ok_or("no fork event: worker-thread fork was not tracked")?;
    assert_ne!(parent, child);

    let child_anon: i64 = events
        .iter()
        .filter_map(|e| match e.kind {
            MemtrackEventKind::Rmap { member: 1, delta } if e.pid == child && delta > 0 => {
                Some(delta)
            }
            _ => None,
        })
        .sum();
    assert!(
        child_anon * page_size() as i64 >= ((REGION_MIB - 16) * MIB) as i64,
        "child of a worker-thread fork missed rmap tracking: anon adds {} bytes, expected ~{} MiB",
        child_anon * page_size() as i64,
        REGION_MIB
    );
    Ok(())
}

/// Fork copies page tables through the `folio_*_dup_*` helpers, which have no
/// rmap hooks, so pages inherited at fork never produce adds under the child's
/// pid — but unmapping them fires in-context removes. The event stream
/// therefore legitimately contains more removed than added pages for such a
/// pid, and consumers reconstructing RSS from the deltas must floor at zero
/// instead of going negative.
#[test_with::env(GITHUB_ACTIONS)]
#[test]
fn test_rss_rmap_fork_inherited_unmap_removes_exceed_adds() -> Result<(), Box<dyn std::error::Error>>
{
    const REGION_MIB: u64 = 32;

    let (_report, events) = track_fixture(
        include_str!("../testdata/rss/fork_unmap_inherited.c"),
        "fork_unmap_inherited",
        shared::track_command_with_rmap,
    )?;

    let (parent, child) = first_fork_pair(&events).ok_or("no fork event")?;
    assert_ne!(parent, child);

    let child_anon_net: i64 = events
        .iter()
        .filter_map(|e| match e.kind {
            MemtrackEventKind::Rmap { member: 1, delta } if e.pid == child => Some(delta),
            _ => None,
        })
        .sum();
    assert!(
        child_anon_net * page_size() as i64 <= -(((REGION_MIB - 8) * MIB) as i64),
        "expected the child's inherited-region unmap to leave a net-negative \
         anon delta of ~{REGION_MIB} MiB, got {} bytes",
        child_anon_net * page_size() as i64,
    );
    Ok(())
}
