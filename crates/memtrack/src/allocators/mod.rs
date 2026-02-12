//! Generic allocator discovery infrastructure.
//!
//! This module provides a framework for discovering and attaching to different
//! memory allocators. It's designed to be easily extensible for adding new allocators.

use std::path::PathBuf;

mod dynamic;
mod static_linked;

/// Represents the different allocator types we support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AllocatorKind {
    /// Standard C library (glibc, musl, etc.)
    Libc,
    /// C++ standard library (libstdc++, libc++) - provides operator new/delete
    LibCpp,
    /// jemalloc - used by FreeBSD, Firefox, many Rust projects
    Jemalloc,
    /// mimalloc - Microsoft's allocator
    Mimalloc,
    /// TCMalloc - Google's thread-caching malloc
    ///
    /// Two variants exist:
    /// - **gperftools** (github.com/gperftools/gperftools): Original ~2005 release.
    ///   Exports both standard symbols (malloc/free) AND tc_* prefixed symbols.
    /// - **google/tcmalloc** (github.com/google/tcmalloc): Modern ~2020 rewrite.
    ///   Exports ONLY standard symbols (malloc/free/etc.) - no tc_* prefix.
    ///
    /// We'll always try to attach to both the standard and `tc_*` API. If the newer rewrite is
    /// used, we'll only attach to the standard API.
    Tcmalloc,
    // Future allocators:
    // Hoard,
    // Rpmalloc,
}

impl AllocatorKind {
    /// Returns all supported allocator kinds.
    pub fn all() -> &'static [AllocatorKind] {
        // IMPORTANT: Check non-default allocators first, because they will contain compatibility
        // layers for the default allocators.
        &[
            AllocatorKind::Jemalloc,
            AllocatorKind::Mimalloc,
            AllocatorKind::Tcmalloc,
            AllocatorKind::LibCpp,
            AllocatorKind::Libc,
        ]
    }

    /// Returns a human-readable name for the allocator.
    pub fn name(&self) -> &'static str {
        match self {
            AllocatorKind::Libc => "libc",
            AllocatorKind::LibCpp => "libc++",
            AllocatorKind::Jemalloc => "jemalloc",
            AllocatorKind::Mimalloc => "mimalloc",
            AllocatorKind::Tcmalloc => "tcmalloc",
        }
    }

    /// Returns true if this allocator is required (must be found).
    pub fn is_required(&self) -> bool {
        matches!(self, AllocatorKind::Libc)
    }
}

/// Discovered allocator library with its kind and path.
#[derive(Debug, Clone)]
pub struct AllocatorLib {
    pub kind: AllocatorKind,
    pub path: PathBuf,
}

impl AllocatorLib {
    pub fn find_all() -> anyhow::Result<Vec<AllocatorLib>> {
        let mut allocators = static_linked::find_all()?;
        allocators.extend(dynamic::find_all()?);
        Ok(allocators)
    }
}

/// Check if a file is an ELF binary by reading its magic bytes.
fn is_elf(path: &std::path::Path) -> bool {
    use std::fs;
    use std::io::Read;

    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };

    let mut magic = [0u8; 4];
    if file.read_exact(&mut magic).is_err() {
        return false;
    }

    // ELF magic: 0x7F 'E' 'L' 'F'
    magic == [0x7F, b'E', b'L', b'F']
}
