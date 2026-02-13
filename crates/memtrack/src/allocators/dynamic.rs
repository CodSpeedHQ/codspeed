use crate::{AllocatorKind, AllocatorLib};
use std::path::PathBuf;

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

/// Find dynamically linked allocator libraries on the system.
pub fn find_all() -> anyhow::Result<Vec<AllocatorLib>> {
    use std::collections::HashSet;

    let mut results = Vec::new();
    let mut seen_paths: HashSet<PathBuf> = HashSet::new();

    for kind in AllocatorKind::all() {
        let mut found_any = false;

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
                    found_any = true;
                }
            }
        }

        // FIXME: Do we still need this?
        if kind.is_required() && !found_any {
            anyhow::bail!("Could not find required allocator: {}", kind.name());
        }
    }

    Ok(results)
}
