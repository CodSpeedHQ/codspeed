use crate::{AllocatorKind, AllocatorLib};
use std::path::PathBuf;

/// Returns the glob patterns used to find this allocator's shared libraries.
fn get_allocator_paths(lib: &AllocatorKind) -> &'static [&'static str] {
    match lib {
        AllocatorKind::Libc => &[
            // Debian, Ubuntu: Standard Linux multiarch paths
            "/lib/*-linux-gnu/libc.so.6",
            "/usr/lib/*-linux-gnu/libc.so.6",
            // RHEL, Fedora, CentOS, Arch
            "/lib*/libc.so.6",
            "/usr/lib*/libc.so.6",
            // NixOS: find all glibc versions in the Nix store
            "/nix/store/*glibc*/lib/libc.so.6",
        ],
        AllocatorKind::LibCpp => &[
            // Standard Linux multiarch paths
            "/lib/*-linux-gnu/libstdc++.so*",
            "/usr/lib/*-linux-gnu/libstdc++.so*",
            // RHEL, Fedora, CentOS, Arch
            "/lib*/libstdc++.so*",
            "/usr/lib*/libstdc++.so*",
            // NixOS: find all gcc lib versions in the Nix store
            "/nix/store/*gcc*/lib/libstdc++.so*",
        ],
        AllocatorKind::Jemalloc => &[
            // Debian, Ubuntu: Standard Linux multiarch paths
            "/lib/*-linux-gnu/libjemalloc.so*",
            "/usr/lib/*-linux-gnu/libjemalloc.so*",
            // RHEL, Fedora, CentOS, Arch
            "/lib*/libjemalloc.so*",
            "/usr/lib*/libjemalloc.so*",
            "/usr/local/lib*/libjemalloc.so*",
            // NixOS
            "/nix/store/*jemalloc*/lib/libjemalloc.so*",
        ],
        AllocatorKind::Mimalloc => &[
            // Debian, Ubuntu: Standard Linux multiarch paths
            "/lib/*-linux-gnu/libmimalloc.so*",
            "/usr/lib/*-linux-gnu/libmimalloc.so*",
            // RHEL, Fedora, CentOS, Arch
            "/lib*/libmimalloc.so*",
            "/usr/lib*/libmimalloc.so*",
            "/usr/local/lib*/libmimalloc.so*",
            // NixOS
            "/nix/store/*mimalloc*/lib/libmimalloc.so*",
        ],
        AllocatorKind::Tcmalloc => &[
            // gperftools tcmalloc variants
            // Debian, Ubuntu: Standard Linux multiarch paths
            "/lib/*-linux-gnu/libtcmalloc.so*",
            "/lib/*-linux-gnu/libtcmalloc_minimal.so*",
            "/lib/*-linux-gnu/libtcmalloc_debug.so*",
            "/lib/*-linux-gnu/libtcmalloc_and_profiler.so*",
            "/usr/lib/*-linux-gnu/libtcmalloc.so*",
            "/usr/lib/*-linux-gnu/libtcmalloc_minimal.so*",
            "/usr/lib/*-linux-gnu/libtcmalloc_debug.so*",
            "/usr/lib/*-linux-gnu/libtcmalloc_and_profiler.so*",
            // RHEL, Fedora, CentOS, Arch
            "/lib*/libtcmalloc*.so*",
            "/usr/lib*/libtcmalloc*.so*",
            "/usr/local/lib*/libtcmalloc*.so*",
            // NixOS
            "/nix/store/*tcmalloc*/lib/libtcmalloc*.so*",
            "/nix/store/*gperftools*/lib/libtcmalloc*.so*",
        ],
    }
}

/// Find dynamically linked allocator libraries on the system.
pub fn find_all() -> anyhow::Result<Vec<AllocatorLib>> {
    use std::collections::HashSet;

    let mut results = Vec::new();
    let mut seen_paths: HashSet<PathBuf> = HashSet::new();

    for kind in AllocatorKind::all() {
        let mut found_any = false;

        for pattern in get_allocator_paths(kind) {
            let paths = glob::glob(pattern)
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
