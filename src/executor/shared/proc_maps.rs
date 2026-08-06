use crate::prelude::*;
use std::path::PathBuf;
use tokio::net::unix::pid_t;

/// One executable file mapping of a live process, read from `/proc/<pid>/maps`.
#[derive(Debug, Clone)]
pub struct ProcMapping {
    /// Absolute path of the mapped ELF file.
    pub path: PathBuf,
    /// Runtime start address of the mapping.
    pub start: u64,
    /// Runtime end address of the mapping.
    pub end: u64,
    /// File offset of the mapping (matches perf's `MMAP2` page offset).
    pub offset: u64,
}

/// Best-effort snapshot of a live process's executable file mappings.
///
/// perf only emits `MMAP2`/`COMM` while its event is enabled, so any benchmark
/// process that exec()'d while sampling was disabled (every one after the first in
/// a run) is missing its code mappings in the perf data. Reading `/proc/<pid>/maps`
/// while the process is alive recovers those load addresses so its samples can be
/// symbolized. Returns empty on any error rather than failing the run.
pub fn read_proc_maps(pid: pid_t) -> Vec<ProcMapping> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }

    let path = format!("/proc/{pid}/maps");
    match std::fs::read_to_string(&path) {
        Ok(content) => parse_proc_maps(&content),
        Err(e) => {
            warn!("Failed to read {path} for benchmark pid {pid}: {e}");
            Vec::new()
        }
    }
}

/// Parse the executable file mappings out of `/proc/<pid>/maps` content.
///
/// Each line is `start-end perms offset dev inode pathname`. We keep only
/// executable (`x`) file-backed mappings; anonymous/special (`[vdso]`, `[heap]`, …)
/// and deleted files are skipped. The numeric fields never contain `/` or `[`, so
/// the pathname is taken from the first such character, which tolerates spaces in
/// the path.
fn parse_proc_maps(content: &str) -> Vec<ProcMapping> {
    let mut mappings = Vec::new();

    for line in content.lines() {
        let mut fields = line.split_whitespace();
        let (Some(range), Some(perms), Some(offset)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };

        if !perms.contains('x') {
            continue;
        }

        let Some(path_start) = line.find(['/', '[']) else {
            continue;
        };
        let pathname = line[path_start..].trim();
        if pathname.starts_with('[') {
            continue;
        }
        let pathname = pathname.strip_suffix(" (deleted)").unwrap_or(pathname);

        let Some((start, end)) = range.split_once('-') else {
            continue;
        };
        let (Ok(start), Ok(end), Ok(offset)) = (
            u64::from_str_radix(start, 16),
            u64::from_str_radix(end, 16),
            u64::from_str_radix(offset, 16),
        ) else {
            continue;
        };

        mappings.push(ProcMapping {
            path: PathBuf::from(pathname),
            start,
            end,
            offset,
        });
    }

    mappings
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn parse_proc_maps_keeps_only_executable_file_mappings() {
        let content = "\
55a0f0000000-55a0f0001000 r--p 00000000 fe:01 100 /bin/bench
55a0f0001000-55a0f0100000 r-xp 00001000 fe:01 100 /bin/bench
55a0f0100000-55a0f0200000 r--p 00100000 fe:01 100 /bin/bench
7f00aa000000-7f00aa100000 r-xp 00002000 fe:01 200 /usr/lib/libc.so.6
7f00bb000000-7f00bb001000 rw-p 00000000 00:00 0
7ffd00000000-7ffd00021000 rw-p 00000000 00:00 0 [stack]
7ffd00021000-7ffd00023000 r-xp 00000000 00:00 0 [vdso]
7f00cc000000-7f00cc010000 r-xp 00000000 fe:01 300 /tmp/old.so (deleted)
";
        let maps = parse_proc_maps(content);

        // Only the two live executable file mappings (bench .text, libc .text) and the
        // deleted one (path stripped) are kept; non-exec, anon, [stack], [vdso] dropped.
        let paths: Vec<_> = maps
            .iter()
            .map(|m| m.path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(paths, vec!["/bin/bench", "/usr/lib/libc.so.6", "/tmp/old.so"]);

        let bench = &maps[0];
        assert_eq!(bench.start, 0x55a0f0001000);
        assert_eq!(bench.end, 0x55a0f0100000);
        assert_eq!(bench.offset, 0x1000);
    }

    #[test]
    fn parse_proc_maps_handles_paths_with_spaces() {
        let content =
            "400000-401000 r-xp 00000000 fe:01 100 /home/user/my bench/target/deps/thing\n";
        let maps = parse_proc_maps(content);
        assert_eq!(maps.len(), 1);
        assert_eq!(
            maps[0].path.to_string_lossy(),
            "/home/user/my bench/target/deps/thing"
        );
    }
}
