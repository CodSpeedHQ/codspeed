use crate::{AllocatorKind, AllocatorLib};
use std::path::{Path, PathBuf};
use std::process::Command;

impl AllocatorKind {
    /// Build glob patterns for finding this allocator's shared libraries.
    fn search_patterns(&self) -> Vec<String> {
        const LIB_DIRS: &[&str] = &[
            // Debian, Ubuntu: multiarch paths
            "/lib/*-linux-gnu",
            "/usr/lib/*-linux-gnu",
            // RHEL, Fedora, CentOS, Arch
            "/lib*",
            "/usr/lib*",
            // Local installs
            "/usr/local/lib*",
        ];

        let (filenames, nix_hints): (&[&str], &[&str]) = match self {
            AllocatorKind::Libc => (&["libc.so.6"], &["glibc"]),
            AllocatorKind::LibCpp => (&["libstdc++.so*"], &["gcc"]),
            AllocatorKind::Jemalloc => (&["libjemalloc.so*"], &["jemalloc"]),
            AllocatorKind::Mimalloc => (&["libmimalloc.so*"], &["mimalloc"]),
            AllocatorKind::Tcmalloc => (&["libtcmalloc*.so*"], &["tcmalloc", "gperftools"]),
        };

        let mut patterns = Vec::new();

        for dir in LIB_DIRS {
            for filename in filenames {
                patterns.push(format!("{dir}/{filename}"));
            }
        }

        for hint in nix_hints {
            for filename in filenames {
                patterns.push(format!("/nix/store/*{hint}*/lib/{filename}"));
            }
        }

        patterns
    }
}

/// Path-list of exact allocator libraries to instrument. When set, the
/// filesystem globbing below is skipped.
///
/// This avoids the nix store problem: the glob patterns match every copy of
/// glibc/libstdc++ in `/nix/store` (each a distinct file), and we attach probes
/// to all of them even though the target loads only one.
const ALLOCATOR_LIBS_ENV: &str = "CODSPEED_MEMTRACK_ALLOCATOR_LIBS";

/// Toggle to auto-resolve the allocator libraries the target binaries actually
/// load, instead of globbing. Same effect as [`ALLOCATOR_LIBS_ENV`] without
/// spelling out paths: binaries come from [`BINARIES_ENV`], resolved via each
/// binary's own loader (which honours nix RPATH).
const AUTO_LIBS_ENV: &str = "CODSPEED_MEMTRACK_AUTO_LIBS";

/// Path-list of target binaries, populated by the runner. Also used for static
/// allocator discovery in the parent module.
const BINARIES_ENV: &str = "CODSPEED_MEMTRACK_BINARIES";

fn env_is_truthy(name: &str) -> bool {
    std::env::var(name)
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Read explicit allocator library paths from [`ALLOCATOR_LIBS_ENV`], if set.
/// The kind is detected from each library's symbols; unrecognized files are
/// skipped.
fn find_from_explicit_env() -> Option<Vec<AllocatorLib>> {
    let raw = std::env::var_os(ALLOCATOR_LIBS_ENV)?;

    let libs = std::env::split_paths(&raw)
        .filter(|p| !p.as_os_str().is_empty())
        .filter_map(|p| match AllocatorLib::from_path_static(&p) {
            Ok(lib) => {
                log::debug!(
                    "Using {} allocator from {ALLOCATOR_LIBS_ENV}: {}",
                    lib.kind.name(),
                    p.display()
                );
                Some(lib)
            }
            Err(e) => {
                log::debug!("Skipping {ALLOCATOR_LIBS_ENV} entry {}: {e}", p.display());
                None
            }
        })
        .collect();

    Some(libs)
}

/// Read the ELF interpreter (dynamic loader) path from a binary's `.interp`
/// section, e.g. `/nix/store/<hash>-glibc-<ver>/lib/ld-linux-x86-64.so.2`.
fn read_interp(bin: &Path) -> Option<PathBuf> {
    use object::{Object, ObjectSection};

    let data = std::fs::read(bin).ok()?;
    let file = object::File::parse(&*data).ok()?;
    let section = file.section_by_name(".interp")?;
    let bytes = section.data().ok()?;
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let interp = std::str::from_utf8(&bytes[..end]).ok()?;
    Some(PathBuf::from(interp))
}

/// Parse `ldd`-style loader output into resolved absolute library paths.
/// Handles both `name => /path (0x...)` and bare `/path (0x...)` lines.
fn parse_loader_output(stdout: &str) -> Vec<PathBuf> {
    stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let candidate = if let Some(idx) = line.find(" => ") {
                &line[idx + 4..]
            } else if line.starts_with('/') {
                line
            } else {
                return None;
            };
            let path = candidate.split(" (").next().unwrap_or("").trim();
            if path.is_empty() || path == "not found" {
                return None;
            }
            Some(PathBuf::from(path))
        })
        .collect()
}

/// Resolve the shared libraries a binary loads via its own dynamic loader in
/// list mode (`ld.so --list`, equivalent to `ldd`). List mode does not execute
/// the program, and honours the binary's RPATH/RUNPATH, so it returns the exact
/// nix store paths the binary uses.
fn resolve_loaded_libs(bin: &Path) -> Vec<PathBuf> {
    let output = read_interp(bin).and_then(|interp| {
        Command::new(&interp)
            .arg("--list")
            .arg(bin)
            .output()
            .ok()
            .filter(|o| o.status.success())
    });

    // Fall back to `ldd` if we could not read/run the interpreter directly.
    let output = output.or_else(|| {
        Command::new("ldd")
            .arg(bin)
            .output()
            .ok()
            .filter(|o| o.status.success())
    });

    match output {
        Some(o) => parse_loader_output(&String::from_utf8_lossy(&o.stdout)),
        None => {
            log::debug!("Could not resolve loaded libraries for {}", bin.display());
            Vec::new()
        }
    }
}

/// Auto-resolve allocator libraries from the [`BINARIES_ENV`] target binaries
/// when [`AUTO_LIBS_ENV`] is enabled.
///
/// Returns `None` so the caller falls back to globbing when the toggle is off,
/// no binaries are known, or nothing resolved, rather than attaching to nothing.
fn find_from_target_binaries() -> Option<Vec<AllocatorLib>> {
    use std::collections::HashSet;

    if !env_is_truthy(AUTO_LIBS_ENV) {
        return None;
    }

    let Some(raw) = std::env::var_os(BINARIES_ENV) else {
        log::warn!(
            "{AUTO_LIBS_ENV} is set but {BINARIES_ENV} is empty; falling back to filesystem discovery"
        );
        return None;
    };

    let mut seen_libs: HashSet<PathBuf> = HashSet::new();
    let mut seen_allocs: HashSet<PathBuf> = HashSet::new();
    let mut results = Vec::new();

    for bin in std::env::split_paths(&raw).filter(|p| !p.as_os_str().is_empty()) {
        for lib in resolve_loaded_libs(&bin) {
            let Ok(lib) = lib.canonicalize() else {
                continue;
            };
            if !seen_libs.insert(lib.clone()) {
                continue;
            }
            if let Ok(alloc) = AllocatorLib::from_path_static(&lib) {
                if seen_allocs.insert(alloc.path.clone()) {
                    log::debug!(
                        "Auto-resolved {} allocator: {}",
                        alloc.kind.name(),
                        alloc.path.display()
                    );
                    results.push(alloc);
                }
            }
        }
    }

    if results.is_empty() {
        log::warn!(
            "{AUTO_LIBS_ENV} resolved no allocator libraries from {BINARIES_ENV}; falling back to filesystem discovery"
        );
        return None;
    }

    Some(results)
}

/// Find dynamically linked allocator libraries on the system.
pub fn find_all() -> anyhow::Result<Vec<AllocatorLib>> {
    use std::collections::HashSet;

    if let Some(libs) = find_from_explicit_env() {
        return Ok(libs);
    }

    if let Some(libs) = find_from_target_binaries() {
        return Ok(libs);
    }

    let mut results = Vec::new();
    let mut seen_paths: HashSet<PathBuf> = HashSet::new();

    for kind in AllocatorKind::all() {
        for pattern in kind.search_patterns() {
            let paths = glob::glob(&pattern)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(|p| p.ok())
                .filter_map(|p| p.canonicalize().ok())
                .filter(|path| {
                    std::fs::metadata(path)
                        .map(|m| m.is_file())
                        .unwrap_or(false)
                })
                .filter(|path| super::is_elf(path))
                .collect::<Vec<_>>();

            for path in paths {
                if seen_paths.insert(path.clone()) {
                    results.push(AllocatorLib { kind: *kind, path });
                }
            }
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_loader_output_extracts_resolved_paths() {
        // Representative `ld.so --list` / `ldd` output, including a vdso (no
        // path), a `name => /path` mapping, and a bare interpreter path.
        let out = "\tlinux-vdso.so.1 (0x00007ffffabc000)\n\
             \tlibc.so.6 => /nix/store/abc-glibc-2.42/lib/libc.so.6 (0x00007f00)\n\
             \tlibstdc++.so.6 => /nix/store/def-gcc/lib/libstdc++.so.6 (0x00007f10)\n\
             \t/nix/store/ghi-glibc-2.42/lib/ld-linux-x86-64.so.2 (0x00007f20)\n";

        let paths = parse_loader_output(out);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/nix/store/abc-glibc-2.42/lib/libc.so.6"),
                PathBuf::from("/nix/store/def-gcc/lib/libstdc++.so.6"),
                PathBuf::from("/nix/store/ghi-glibc-2.42/lib/ld-linux-x86-64.so.2"),
            ]
        );
    }

    #[test]
    fn parse_loader_output_skips_unresolved_and_blank() {
        let out = "\tlibmissing.so => not found\n\t\n\tsome noise\n";
        assert!(parse_loader_output(out).is_empty());
    }
}
