use crate::prelude::*;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A mapping resolved from `/proc/<pid>/maps` back to an attachable path.
pub(super) struct ResolvedMapping {
    /// `/proc/<pid>/map_files/<start>-<end>` — resolves uniformly for deleted
    /// files and requires root (which memtrack already has for uprobes).
    pub attach_path: PathBuf,
    /// The pathname column, for logs.
    pub display: String,
}

/// Block until every thread of `pid` is group-stopped.
///
/// The process state is the first non-space char after the LAST `)` in
/// `/proc/<pid>/task/<tid>/stat`. Success means every thread is `T`/`t`.
///
/// - A vanished process (`/proc/<pid>` gone) is success: the stop is moot.
/// - At the deadline, threads still in uninterruptible sleep (`D`) are treated
///   as stopped — they cannot execute user code and join the group stop when the
///   syscall returns. Any thread still `R`/`S` is a hard error: leaving it
///   running breaks the drain guarantee.
pub(super) fn wait_all_stopped(pid: u32, deadline: Duration) -> Result<()> {
    let start = Instant::now();
    let task_dir = format!("/proc/{pid}/task");

    loop {
        let Ok(entries) = std::fs::read_dir(&task_dir) else {
            return Ok(());
        };

        let mut all_stopped = true;
        let mut running_tid: Option<u32> = None;

        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(tid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };

            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            let Some(state) = task_state(&stat) else {
                continue;
            };

            match state {
                'T' | 't' => {}
                'D' => all_stopped = false,
                _ => {
                    all_stopped = false;
                    running_tid = Some(tid);
                }
            }
        }

        if all_stopped {
            return Ok(());
        }

        if start.elapsed() >= deadline {
            let Some(tid) = running_tid else {
                warn!(
                    "pid {pid}: thread(s) in uninterruptible sleep (D) at stop deadline; treating as stopped"
                );
                return Ok(());
            };
            bail!("thread {tid} of {pid} did not stop within {deadline:?}");
        }

        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The process state char: first non-space after the LAST `)` (comm can contain
/// both `)` and spaces).
fn task_state(stat: &str) -> Option<char> {
    let idx = stat.rfind(')')?;
    stat[idx + 1..].trim_start().chars().next()
}

/// Resolve the `(dev, ino)` reported by the watcher to an attachable mapping.
///
/// `dev` is the kernel `s_dev` encoding `(major << 20) | minor`; the maps dev
/// column prints `MAJOR:MINOR` in lowercase hex (`major = dev >> 20`,
/// `minor = dev & 0xFFFFF`). Returns `None` if no row matches (the mmap failed
/// after the LSM hook, or the process exited) — the caller retries on the next
/// sighting.
pub(super) fn resolve_mapping(pid: u32, dev: u64, ino: u64) -> Option<ResolvedMapping> {
    let maps = std::fs::read_to_string(format!("/proc/{pid}/maps")).ok()?;
    let want_major = (dev >> 20) as u32;
    let want_minor = (dev & 0xFFFFF) as u32;

    for line in maps.lines() {
        // range perms offset dev inode pathname
        let mut fields = line.split_whitespace();
        let (Some(range), Some(_perms), Some(_offset), Some(dev_col), Some(inode_col)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };

        if inode_col.parse::<u64>().ok() != Some(ino) {
            continue;
        }
        if parse_dev(dev_col) != Some((want_major, want_minor)) {
            continue;
        }

        let display = fields.next().unwrap_or("<anonymous>").to_string();
        return Some(ResolvedMapping {
            attach_path: PathBuf::from(format!("/proc/{pid}/map_files/{range}")),
            display,
        });
    }

    None
}

/// Parse a `MAJOR:MINOR` lowercase-hex dev column.
fn parse_dev(col: &str) -> Option<(u32, u32)> {
    let (major, minor) = col.split_once(':')?;
    Some((
        u32::from_str_radix(major, 16).ok()?,
        u32::from_str_radix(minor, 16).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_state_reads_char_after_last_paren() {
        assert_eq!(task_state("1234 (bash) S 1 0 0"), Some('S'));
        assert_eq!(task_state("1234 (weird )name) T 1"), Some('T'));
        assert_eq!(task_state("1 (x) R"), Some('R'));
        assert_eq!(task_state("no paren here"), None);
    }

    #[test]
    fn parse_dev_reads_hex_major_minor() {
        assert_eq!(parse_dev("08:01"), Some((8, 1)));
        assert_eq!(parse_dev("fd:00"), Some((0xfd, 0)));
        assert_eq!(parse_dev("00:1a"), Some((0, 0x1a)));
        assert_eq!(parse_dev("nope"), None);
        assert_eq!(parse_dev("zz:01"), None);
    }

    /// Round-trips the kernel dev encoding `(major << 20) | minor` through
    /// resolve_mapping against our own maps (no root needed): a file-backed
    /// mapping's dev/ino must resolve back to that same mapping.
    #[test]
    fn resolve_mapping_round_trips_self_maps() {
        let pid = std::process::id();
        let maps = std::fs::read_to_string(format!("/proc/{pid}/maps")).unwrap();

        let target = maps.lines().find_map(|line| {
            let mut f = line.split_whitespace();
            let (_range, _perms, _off, dev, inode) =
                (f.next()?, f.next()?, f.next()?, f.next()?, f.next()?);
            let path = f.next()?;
            if !path.starts_with('/') {
                return None;
            }
            let ino: u64 = inode.parse().ok()?;
            if ino == 0 {
                return None;
            }
            let (maj, min) = parse_dev(dev)?;
            let s_dev = ((maj as u64) << 20) | (min as u64);
            Some((s_dev, ino, path.to_string()))
        });

        let Some((s_dev, ino, path)) = target else {
            return;
        };

        let resolved = resolve_mapping(pid, s_dev, ino).expect("mapping should resolve");
        assert_eq!(resolved.display, path);
        assert!(
            resolved
                .attach_path
                .starts_with(format!("/proc/{pid}/map_files/")),
            "attach_path was {:?}",
            resolved.attach_path
        );
    }
}
