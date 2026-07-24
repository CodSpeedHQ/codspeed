use crate::prelude::*;
use libbpf_rs::UprobeOpts;
use paste::paste;
use std::path::Path;

use super::{MemtrackBpf, ResolvedSymbols, resolve_symbol_offsets};
use crate::allocators::{AllocatorKind, AllocatorLib};

impl MemtrackBpf {
    attach_uprobe_uretprobe!(attach_malloc, uprobe_malloc, uretprobe_malloc);
    attach_uprobe_uretprobe!(attach_calloc, uprobe_calloc, uretprobe_calloc);
    attach_uprobe_uretprobe!(attach_realloc, uprobe_realloc, uretprobe_realloc);
    attach_uprobe_uretprobe!(
        attach_aligned_alloc,
        uprobe_aligned_alloc,
        uretprobe_aligned_alloc
    );
    attach_uprobe_uretprobe!(attach_memalign, uprobe_memalign, uretprobe_memalign);
    attach_uprobe_uretprobe!(
        attach_posix_memalign,
        uprobe_posix_memalign,
        uretprobe_posix_memalign
    );
    attach_uprobe!(attach_free, uprobe_free);

    /// Attach probes for every discovered allocator library.
    pub fn attach_allocators(&mut self, libs: &[AllocatorLib]) -> Result<()> {
        use rayon::prelude::*;

        let resolved = libs
            .par_iter()
            .map(|lib| resolve_symbol_offsets(&lib.path).map(|offsets| (lib, offsets)))
            .collect::<Result<Vec<_>>>()?;

        for (lib, offsets) in resolved {
            let before = self.probes.len();
            self.attach_allocator_probes_with_offsets(lib.kind, &lib.path, &offsets)?;
            debug!(
                "Attached {} links to {} ({} resolved symbols)",
                self.probes.len() - before,
                lib.path.display(),
                offsets.offsets.len()
            );
        }

        Ok(())
    }

    /// Attach probes for a specific allocator kind.
    /// This attaches both standard probes (if the allocator exports them) and
    /// allocator-specific prefixed probes.
    pub fn attach_allocator_probes(&mut self, kind: AllocatorKind, lib_path: &Path) -> Result<()> {
        let offsets = resolve_symbol_offsets(lib_path)?;
        self.attach_allocator_probes_with_offsets(kind, lib_path, &offsets)
    }

    fn attach_allocator_probes_with_offsets(
        &mut self,
        kind: AllocatorKind,
        lib_path: &Path,
        offsets: &ResolvedSymbols,
    ) -> Result<()> {
        debug!(
            "Attaching {} probes to: {}",
            kind.name(),
            lib_path.display()
        );

        match kind {
            AllocatorKind::Libc => {
                // Libc only has standard probes, and they must succeed: a libc
                // with an uninstrumented core entry point would silently
                // produce an incomplete trace.
                for symbol in ["malloc", "calloc", "realloc", "free"] {
                    ensure!(
                        offsets.offset(symbol).is_some(),
                        "Required allocator symbol {symbol} has no resolvable file offset in {}",
                        lib_path.display()
                    );
                }
                self.attach_libc_probes(lib_path, offsets)
            }
            AllocatorKind::LibCpp => {
                // libc++ exports C++ operator new/delete symbols
                self.attach_libcpp_probes(lib_path, offsets)
            }
            AllocatorKind::Jemalloc => {
                // Jemalloc exposes libc/libcpp compatible allocator functions:
                self.attach_compat_probes(lib_path, offsets);
                self.attach_jemalloc_probes(lib_path, offsets)
            }
            AllocatorKind::Mimalloc => {
                // Mimalloc exposes libc/libcpp compatible allocator functions:
                self.attach_compat_probes(lib_path, offsets);
                self.attach_mimalloc_probes(lib_path, offsets)
            }
            AllocatorKind::Tcmalloc => {
                // Tcmalloc exposes libc/libcpp compatible allocator functions:
                self.attach_compat_probes(lib_path, offsets);
                self.attach_tcmalloc_probes(lib_path, offsets)
            }
        }
    }

    /// Best-effort attach of the libc/libcpp compatible symbols that
    /// non-libc allocators may also export.
    fn attach_compat_probes(&mut self, lib_path: &Path, offsets: &ResolvedSymbols) {
        if let Err(e) = self.attach_libc_probes(lib_path, offsets) {
            warn!("libc-compatible probes for {}: {e:#}", lib_path.display());
        }
        if let Err(e) = self.attach_libcpp_probes(lib_path, offsets) {
            warn!("libcpp-compatible probes for {}: {e:#}", lib_path.display());
        }
    }

    fn attach_standard_probes(
        &mut self,
        lib_path: &Path,
        prefixes: &[&str],
        suffixes: &[&str],
        offsets: &ResolvedSymbols,
    ) -> Result<()> {
        // Always include "" to capture the basic case
        let prefixes_with_base: Vec<&str> = std::iter::once("")
            .chain(prefixes.iter().copied())
            .unique()
            .collect();

        let suffixes_with_base: Vec<&str> = std::iter::once("")
            .chain(suffixes.iter().copied())
            .unique()
            .collect();

        for prefix in &prefixes_with_base {
            for suffix in &suffixes_with_base {
                self.attach_malloc_if_found(lib_path, &format!("{prefix}malloc{suffix}"), offsets)?;
                self.attach_malloc_if_found(lib_path, &format!("{prefix}valloc{suffix}"), offsets)?;
                self.attach_malloc_if_found(
                    lib_path,
                    &format!("{prefix}pvalloc{suffix}"),
                    offsets,
                )?;
                self.attach_calloc_if_found(lib_path, &format!("{prefix}calloc{suffix}"), offsets)?;
                self.attach_realloc_if_found(
                    lib_path,
                    &format!("{prefix}realloc{suffix}"),
                    offsets,
                )?;
                self.attach_aligned_alloc_if_found(
                    lib_path,
                    &format!("{prefix}aligned_alloc{suffix}"),
                    offsets,
                )?;
                self.attach_memalign_if_found(
                    lib_path,
                    &format!("{prefix}memalign{suffix}"),
                    offsets,
                )?;
                self.attach_posix_memalign_if_found(
                    lib_path,
                    &format!("{prefix}posix_memalign{suffix}"),
                    offsets,
                )?;
                self.attach_free_if_found(lib_path, &format!("{prefix}free{suffix}"), offsets)?;
                self.attach_free_if_found(
                    lib_path,
                    &format!("{prefix}free_sized{suffix}"),
                    offsets,
                )?;
                self.attach_free_if_found(
                    lib_path,
                    &format!("{prefix}free_aligned_sized{suffix}"),
                    offsets,
                )?;
                self.attach_free_if_found(lib_path, &format!("{prefix}cfree{suffix}"), offsets)?;
            }
        }

        Ok(())
    }

    /// Attach standard library allocation probes (libc-style: malloc, free, calloc, etc.)
    /// This works for libc and allocators that export standard symbol names.
    /// For non-libc allocators, standard names are optional - just try them silently.
    fn attach_libc_probes(&mut self, lib_path: &Path, offsets: &ResolvedSymbols) -> Result<()> {
        self.attach_standard_probes(lib_path, &[], &[], offsets)
    }

    /// Attach C++ operator new/delete probes.
    /// These are mangled C++ symbols that wrap the underlying allocator.
    /// C++ operators have identical signatures to malloc/free, so we reuse those handlers.
    fn attach_libcpp_probes(&mut self, lib_path: &Path, offsets: &ResolvedSymbols) -> Result<()> {
        self.attach_malloc_if_found(lib_path, "_Znwm", offsets)?; // operator new(size_t)
        self.attach_malloc_if_found(lib_path, "_Znam", offsets)?; // operator new[](size_t)
        self.attach_malloc_if_found(lib_path, "_ZnwmSt11align_val_t", offsets)?; // operator new(size_t, std::align_val_t)
        self.attach_malloc_if_found(lib_path, "_ZnamSt11align_val_t", offsets)?; // operator new[](size_t, std::align_val_t)
        self.attach_free_if_found(lib_path, "_ZdlPv", offsets)?; // operator delete(void*)
        self.attach_free_if_found(lib_path, "_ZdaPv", offsets)?; // operator delete[](void*)
        self.attach_free_if_found(lib_path, "_ZdlPvm", offsets)?; // operator delete(void*, size_t) - C++14 sized delete
        self.attach_free_if_found(lib_path, "_ZdaPvm", offsets)?; // operator delete[](void*, size_t) - C++14 sized delete
        self.attach_free_if_found(lib_path, "_ZdlPvSt11align_val_t", offsets)?; // operator delete(void*, std::align_val_t)
        self.attach_free_if_found(lib_path, "_ZdaPvSt11align_val_t", offsets)?; // operator delete[](void*, std::align_val_t)
        self.attach_free_if_found(lib_path, "_ZdlPvmSt11align_val_t", offsets)?; // operator delete(void*, size_t, std::align_val_t)
        self.attach_free_if_found(lib_path, "_ZdaPvmSt11align_val_t", offsets)?; // operator delete[](void*, size_t, std::align_val_t)

        Ok(())
    }

    /// Attach jemalloc-specific probes (prefixed and extended API).
    fn attach_jemalloc_probes(&mut self, lib_path: &Path, offsets: &ResolvedSymbols) -> Result<()> {
        // The following functions are used in Rust when setting a global allocator:
        // - rust_alloc: _rjem_malloc and _rjem_mallocx
        // - rust_alloc_zeroed: _rjem_mallocx / _rjem_calloc
        // - rust_dealloc: _rjem_sdallocx
        // - rust_realloc: _rjem_realloc / _rjem_rallocx

        // je_* API (internal jemalloc functions, static linking)
        // _rjem_* API (Rust jemalloc crate, dynamic linking)
        let prefixes = ["je_", "_rjem_"];
        let suffixes = ["", "_default"];

        self.attach_standard_probes(lib_path, &prefixes, &suffixes, offsets)?;

        // Non-standard API that has an additional flag parameter
        // See: https://jemalloc.net/jemalloc.3.html
        for prefix in prefixes {
            for suffix in suffixes {
                self.attach_malloc_if_found(
                    lib_path,
                    &format!("{prefix}mallocx{suffix}"),
                    offsets,
                )?;
                self.attach_realloc_if_found(
                    lib_path,
                    &format!("{prefix}rallocx{suffix}"),
                    offsets,
                )?;
                self.attach_free_if_found(lib_path, &format!("{prefix}dallocx{suffix}"), offsets)?;
                self.attach_free_if_found(lib_path, &format!("{prefix}sdallocx{suffix}"), offsets)?;
            }
        }

        Ok(())
    }

    /// Attach mimalloc-specific probes (mi_* API).
    fn attach_mimalloc_probes(&mut self, lib_path: &Path, offsets: &ResolvedSymbols) -> Result<()> {
        // The following functions are used in Rust when setting a global allocator:
        // - mi_malloc_aligned
        // - mi_free
        // - mi_realloc_aligned
        // - mi_zalloc_aligned

        self.attach_standard_probes(lib_path, &["mi_"], &[], offsets)?;

        // Zero-initialized and aligned variants
        self.attach_malloc_if_found(lib_path, "mi_malloc_aligned", offsets)?;
        self.attach_calloc_if_found(lib_path, "mi_zalloc", offsets)?;
        self.attach_calloc_if_found(lib_path, "mi_zalloc_aligned", offsets)?;
        self.attach_realloc_if_found(lib_path, "mi_realloc_aligned", offsets)?;

        Ok(())
    }

    /// Attach TCMalloc probes ( tc_* API).
    ///
    /// See:
    /// - https://github.com/google/tcmalloc/blob/master/docs/reference.md
    /// - https://github.com/gperftools/gperftools/blob/a47243150ec41097602730ff8779fafcc172d1fb/src/tcmalloc.cc#L178-L190
    fn attach_tcmalloc_probes(&mut self, lib_path: &Path, offsets: &ResolvedSymbols) -> Result<()> {
        self.attach_standard_probes(lib_path, &["tc_"], &[], offsets)?;

        self.attach_free_if_found(lib_path, "free_sized", offsets)?;
        self.attach_free_if_found(lib_path, "free_aligned_sized", offsets)?;
        self.attach_free_if_found(lib_path, "sdallocx", offsets)?;

        Ok(())
    }
}
