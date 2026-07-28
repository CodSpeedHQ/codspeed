use crate::prelude::*;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A mapping resolved from `/proc/<pid>/maps` back to an attachable path.
#[derive(Debug)]
pub(super) struct ResolvedMapping {
    /// `/proc/<pid>/map_files/<start>-<end>` — resolves uniformly for replaced
    /// and deleted files, but opening it requires `CAP_CHECKPOINT_RESTORE` or
    /// `CAP_SYS_ADMIN` *in the init user namespace* (`proc_map_files_get_link`
    /// calls `checkpoint_restore_ns_capable(&init_user_ns)`). A process in a
    /// nested user namespace cannot satisfy that, however privileged it is there.
    pub attach_path: PathBuf,
    /// The pathname column, for logs and as the fallback attach path.
    pub display: String,
}

impl ResolvedMapping {
    /// Paths to try opening the mapped file through, in preference order.
    ///
    /// [`Self::attach_path`] comes first because it references the mapped inode,
    /// so it stays correct even when the path has been replaced or unlinked. The
    /// pathname column is the fallback: it needs no privilege beyond read access,
    /// but being a name rather than an inode reference it can resolve to
    /// different contents than the process actually mapped.
    ///
    /// A `[deleted]`-suffixed or non-absolute pathname (`[heap]`, `[vdso]`, an
    /// anonymous mapping) names no openable file and is skipped.
    pub fn candidate_paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.attach_path.clone()];
        if self.display.starts_with('/') && !self.display.ends_with(" (deleted)") {
            paths.push(PathBuf::from(&self.display));
        }
        paths
    }
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

/// The outcome of resolving a watcher `(dev, ino)` against `/proc/<pid>/maps`.
#[derive(Debug)]
pub(super) enum Resolution {
    /// A matching file-backed mapping was found.
    Resolved(ResolvedMapping),
    /// `/proc/<pid>/maps` is gone: the process exited before it could be
    /// classified. Nothing to attach.
    ProcessGone,
    /// maps was read but no row matches the watcher's `(dev, ino)`. The process
    /// is stopped with the mapping necessarily present (the SIGSTOP lands on the
    /// mmap syscall's return), so an absent row is a real coverage gap — e.g. an
    /// inode-namespace mismatch on overlayfs — not a benign retry.
    Unresolved,
}

/// Resolve the `(dev, ino)` reported by the watcher to an attachable mapping.
///
/// `dev` is the kernel `s_dev` encoding `(major << 20) | minor`; the maps dev
/// column prints `MAJOR:MINOR` in lowercase hex (`major = dev >> 20`,
/// `minor = dev & 0xFFFFF`).
pub(super) fn resolve_mapping(pid: u32, dev: u64, ino: u64) -> Resolution {
    let maps = match std::fs::read_to_string(format!("/proc/{pid}/maps")) {
        Ok(maps) => maps,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Resolution::ProcessGone,
        // A non-NotFound read error means coverage cannot be verified; fail
        // closed rather than silently continuing.
        Err(_) => return Resolution::Unresolved,
    };
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
        let Some(name) = map_files_name(range) else {
            return Resolution::Unresolved;
        };
        return Resolution::Resolved(ResolvedMapping {
            attach_path: PathBuf::from(format!("/proc/{pid}/map_files/{name}")),
            display,
        });
    }

    Resolution::Unresolved
}

/// Re-emit a `/proc/<pid>/maps` range column as a `map_files` dirent name.
///
/// The maps column is zero-padded to a minimum of 8 hex digits, but `map_files`
/// dirents use the kernel's canonical no-leading-zero hex, so a low-address
/// (e.g. non-PIE) mapping must be normalized or the resulting path will not exist.
fn map_files_name(range: &str) -> Option<String> {
    let (start, end) = range.split_once('-')?;
    let start = u64::from_str_radix(start, 16).ok()?;
    let end = u64::from_str_radix(end, 16).ok()?;
    Some(format!("{start:x}-{end:x}"))
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

    #[test]
    fn map_files_name_strips_leading_zeros() {
        // The maps column zero-pads to >=8 hex digits; map_files dirents don't.
        assert_eq!(
            map_files_name("00400000-00452000").as_deref(),
            Some("400000-452000")
        );
        assert_eq!(
            map_files_name("7f0000000000-7f0000001000").as_deref(),
            Some("7f0000000000-7f0000001000")
        );
        assert_eq!(map_files_name("1000"), None);
        assert_eq!(map_files_name("zzzz-1000"), None);
        assert_eq!(map_files_name("1000-zzzz"), None);
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

        let resolved = match resolve_mapping(pid, s_dev, ino) {
            Resolution::Resolved(m) => m,
            other => panic!("mapping should resolve, got {other:?}"),
        };
        assert_eq!(resolved.display, path);
        assert!(
            resolved
                .attach_path
                .starts_with(format!("/proc/{pid}/map_files/")),
            "attach_path was {:?}",
            resolved.attach_path
        );
        // The canonical name must actually resolve. A zero-padded range yields
        // an ENOENT path (a failure); a restricted map_files yields
        // PermissionDenied (tolerated).
        match std::fs::read_link(&resolved.attach_path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {}
            Err(e) => panic!(
                "map_files path {:?} did not resolve: {e}",
                resolved.attach_path
            ),
        }
    }

    /// A tracker that cannot open `map_files` reaches the mapped file only
    /// through the pathname column, so it must be offered.
    #[test]
    fn candidate_paths_falls_back_to_the_pathname_column() {
        let mapping = ResolvedMapping {
            attach_path: PathBuf::from("/proc/1/map_files/400000-452000"),
            display: "/lib/x86_64-linux-gnu/libc.so.6".to_string(),
        };
        assert_eq!(
            mapping.candidate_paths(),
            vec![
                PathBuf::from("/proc/1/map_files/400000-452000"),
                PathBuf::from("/lib/x86_64-linux-gnu/libc.so.6"),
            ],
            "map_files must be preferred, with the pathname column as fallback"
        );
    }

    /// A pathname naming no openable file must not be offered: opening it either
    /// fails or, for a reused path, reads the wrong file.
    #[test]
    fn candidate_paths_skips_unopenable_pathnames() {
        for display in [
            "/tmp/libfoo.so (deleted)",
            "[heap]",
            "[vdso]",
            "<anonymous>",
        ] {
            let mapping = ResolvedMapping {
                attach_path: PathBuf::from("/proc/1/map_files/400000-452000"),
                display: display.to_string(),
            };
            assert_eq!(
                mapping.candidate_paths(),
                vec![PathBuf::from("/proc/1/map_files/400000-452000")],
                "{display} should not be offered as an attach path"
            );
        }
    }

    #[test]
    fn resolve_mapping_unresolved_for_wrong_inode_on_live_process() {
        // Our own maps is readable (process alive), but no row carries this
        // bogus (dev, ino), so coverage cannot be confirmed: fail closed.
        let pid = std::process::id();
        assert!(matches!(
            resolve_mapping(pid, 0, u64::MAX),
            Resolution::Unresolved
        ));
    }

    #[test]
    fn resolve_mapping_process_gone_for_absent_pid() {
        // A pid with no /proc entry is a benign, non-fatal miss.
        assert!(matches!(
            resolve_mapping(u32::MAX, 0, 1),
            Resolution::ProcessGone
        ));
    }
}
