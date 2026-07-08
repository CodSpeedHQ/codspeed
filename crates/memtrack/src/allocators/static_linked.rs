use std::collections::HashSet;
use std::fs;
use std::path::Path;

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

impl AllocatorLib {
    pub fn from_path_static(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let kind = find_statically_linked_allocator(path).ok_or("No allocator found")?;
        Ok(Self {
            kind,
            path: path.to_path_buf(),
        })
    }
}
