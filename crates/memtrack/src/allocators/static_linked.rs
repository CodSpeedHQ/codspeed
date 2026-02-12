use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::allocators::{AllocatorKind, AllocatorLib};

impl AllocatorKind {
    /// Returns the symbol names used to detect this allocator in binaries.
    pub fn symbols(&self) -> &'static [&'static str] {
        match self {
            AllocatorKind::Libc => &["malloc", "free"],
            AllocatorKind::LibCpp => &["_Znwm", "_Znam", "_ZdlPv", "_ZdaPv"],
            AllocatorKind::Jemalloc => &["_rjem_malloc", "je_malloc", "je_malloc_default"],
            AllocatorKind::Mimalloc => &["mi_malloc_aligned", "mi_malloc", "mi_free"],
            AllocatorKind::Tcmalloc => &["tc_malloc", "tc_free", "tc_version"],
        }
    }
}

/// Walk upward and downward from current directory to find build directories.
/// Returns all found build directories in order of preference.
fn find_build_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let Ok(current_dir) = std::env::current_dir() else {
        return dirs;
    };

    let patterns = ["target/codspeed/analysis", "bazel-bin", "build"];
    let mut check_patterns = |dir: &Path| {
        for pattern in &patterns {
            let path = dir.join(pattern);
            if path.is_dir() {
                dirs.push(path);
            }
        }
    };

    // Walk upward from parent directories
    // Note: We skip current_dir here since the downward walk (below) already checks it
    let mut current = current_dir.clone();
    while current.pop() {
        check_patterns(&current);
    }

    // Walk downward from current directory
    let mut stack = vec![current_dir];
    while let Some(dir) = stack.pop() {
        check_patterns(&dir);

        // Read subdirectories
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };

        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();

            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };

            // Skip hidden dirs and common excludes
            if name.starts_with('.') || matches!(name, "node_modules" | "vendor" | "venv") {
                continue;
            }

            // Don't recursive into dirs that we want to match.
            // This can happen with `target` as it contains build dirs for statically linked crates.
            if matches!(name, "target" | "bazel-bin" | "build") {
                continue;
            }

            if path.is_file() {
                continue;
            }

            stack.push(path);
        }
    }

    dirs
}

fn find_binaries_in_dir(dir: &Path) -> Vec<PathBuf> {
    glob::glob(&format!("{}/**/*", dir.display()))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|p| p.is_file() && super::is_elf(p))
        .collect::<Vec<_>>()
}

fn find_statically_linked_allocator(path: &Path) -> Option<AllocatorKind> {
    use object::{Object, ObjectSymbol};

    let data = fs::read(path).ok()?;
    let file = object::File::parse(&*data).ok()?;

    let symbols: HashSet<_> = file
        .symbols()
        .chain(file.dynamic_symbols())
        .filter(|s| s.is_definition())
        .filter_map(|s| s.name().ok())
        .collect();

    // FIXME: We don't support multiple statically linked allocators for now

    AllocatorKind::all()
        .iter()
        .find(|kind| kind.symbols().iter().any(|s| symbols.contains(s)))
        .copied()
}

pub fn find_all() -> anyhow::Result<Vec<AllocatorLib>> {
    let build_dirs = find_build_dirs();
    if build_dirs.is_empty() {
        return Ok(vec![]);
    }

    let mut allocators = Vec::new();
    for build_dir in build_dirs {
        let bins = find_binaries_in_dir(&build_dir);

        for bin in bins {
            let Some(kind) = find_statically_linked_allocator(&bin) else {
                continue;
            };

            allocators.push(AllocatorLib { kind, path: bin });
        }
    }

    Ok(allocators)
}

impl AllocatorLib {
    pub fn from_path_static(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let kind = find_statically_linked_allocator(path).ok_or("No allocator found")?;
        Ok(Self {
            kind,
            path: path.to_path_buf(),
        })
    }
}
