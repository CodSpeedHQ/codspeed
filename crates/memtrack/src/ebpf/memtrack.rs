use crate::prelude::*;
use libbpf_rs::Link;
use libbpf_rs::skel::OpenSkel;
use libbpf_rs::skel::SkelBuilder;
use libbpf_rs::{MapCore, UprobeOpts};
use paste::paste;
use std::collections::HashMap;
use std::mem::MaybeUninit;
use std::path::Path;

use crate::allocators::{AllocatorKind, AllocatorLib};
use crate::ebpf::poller::RingBufferPoller;

pub mod memtrack_skel {
    include!(concat!(env!("OUT_DIR"), "/memtrack.skel.rs"));
}
pub use memtrack_skel::*;

/// Resolve libbpf attach targets for every defined symbol in `lib_path`.
pub fn resolve_symbol_offsets(lib_path: &Path) -> Result<ResolvedSymbols> {
    use object::{Object, ObjectSymbol};

    let data = std::fs::read(lib_path)?;
    let file = object::File::parse(&*data)?;
    let mut offsets = HashMap::new();

    for symbol in file.symbols().chain(file.dynamic_symbols()) {
        if !symbol.is_definition() {
            continue;
        }

        let Ok(name) = symbol.name() else {
            continue;
        };

        if let Some(file_offset) = symbol_file_offset(&file, &symbol) {
            offsets.insert(name.to_owned(), file_offset);
        }
    }

    Ok(ResolvedSymbols { offsets })
}

/// The libbpf file offset for `symbol`, or `None` when it has no address in a
/// file-backed section (absolute, `SHT_NOBITS`, ...).
fn symbol_file_offset<'a>(
    file: &object::File,
    symbol: &impl object::ObjectSymbol<'a>,
) -> Option<usize> {
    use object::{Object, ObjectSection};

    let address = symbol.address();
    if address == 0 {
        return None;
    }

    let section = file.section_by_index(symbol.section_index()?).ok()?;
    let (sh_offset, _) = section.file_range()?;
    Some((address - section.address() + sh_offset) as usize)
}

/// Attach targets resolved from a library's symbol tables.
pub struct ResolvedSymbols {
    offsets: HashMap<String, usize>,
}

impl ResolvedSymbols {
    fn offset(&self, symbol: &str) -> Option<usize> {
        self.offsets.get(symbol).copied()
    }
}

/// Macro to attach a function with both entry and return probes at a resolved
/// file offset. Also generates an `attach_*_if_found` variant that skips
/// symbols absent from the offset table (returning whether it attached) and
/// propagates attach failures.
macro_rules! attach_uprobe_uretprobe {
    ($name:ident, $prog_entry:ident, $prog_return:ident) => {
        paste! {
            fn [<try_ $name>](&mut self, lib_path: &Path, offset: usize) -> Result<()> {
                let link = self
                    .skel
                    .progs
                    .$prog_entry
                    .attach_uprobe_with_opts(
                        -1,
                        lib_path,
                        offset,
                        UprobeOpts {
                            retprobe: false,
                            ..Default::default()
                        },
                    )
                    .context(format!(
                        "Failed to attach uprobe at offset {:#x} in {}",
                        offset,
                        lib_path.display()
                    ))?;
                self.probes.push(link);

                let link = self
                    .skel
                    .progs
                    .$prog_return
                    .attach_uprobe_with_opts(
                        -1,
                        lib_path,
                        offset,
                        UprobeOpts {
                            retprobe: true,
                            ..Default::default()
                        },
                    )
                    .context(format!(
                        "Failed to attach uretprobe at offset {:#x} in {}",
                        offset,
                        lib_path.display()
                    ))?;
                self.probes.push(link);

                Ok(())
            }

            fn [<$name _if_found>](
                &mut self,
                lib_path: &Path,
                symbol: &str,
                symbols: &ResolvedSymbols,
            ) -> Result<bool> {
                let Some(offset) = symbols.offset(symbol) else {
                    return Ok(false);
                };
                self.[<try_ $name>](lib_path, offset)
                    .with_context(|| format!("Failed to attach {symbol}"))?;
                log::trace!("Attached {} at {:#x}", symbol, offset);
                Ok(true)
            }
        }
    };
}

/// Macro to attach a function with only an entry probe (no return probe) at a
/// resolved file offset. Also generates an `attach_*_if_found` variant that
/// skips symbols absent from the offset table (returning whether it attached)
/// and propagates attach failures.
macro_rules! attach_uprobe {
    ($name:ident, $prog:ident) => {
        paste! {
            fn [<try_ $name>](&mut self, lib_path: &Path, offset: usize) -> Result<()> {
                let link = self
                    .skel
                    .progs
                    .$prog
                    .attach_uprobe_with_opts(
                        -1,
                        lib_path,
                        offset,
                        UprobeOpts {
                            retprobe: false,
                            ..Default::default()
                        },
                    )
                    .context(format!(
                        "Failed to attach uprobe at offset {:#x} in {}",
                        offset,
                        lib_path.display()
                    ))?;
                self.probes.push(link);
                Ok(())
            }

            fn [<$name _if_found>](
                &mut self,
                lib_path: &Path,
                symbol: &str,
                symbols: &ResolvedSymbols,
            ) -> Result<bool> {
                let Some(offset) = symbols.offset(symbol) else {
                    return Ok(false);
                };
                self.[<try_ $name>](lib_path, offset)
                    .with_context(|| format!("Failed to attach {symbol}"))?;
                log::trace!("Attached {} at {:#x}", symbol, offset);
                Ok(true)
            }
        }
    };
}

macro_rules! attach_tracepoint {
    ($func:ident, $prog:ident) => {
        fn $func(&mut self) -> Result<()> {
            let link = self
                .skel
                .progs
                .$prog
                .attach()
                .context(format!("Failed to attach {} tracepoint", stringify!($prog)))?;
            self.probes.push(link);
            Ok(())
        }
    };
    ($name:ident) => {
        paste! {
            attach_tracepoint!([<attach_ $name>], [<tracepoint_ $name>]);
        }
    };
}

pub struct MemtrackBpf {
    skel: Box<MemtrackSkel<'static>>,
    probes: Vec<Link>,
}

impl MemtrackBpf {
    pub fn new() -> Result<Self> {
        // Build and open the syscalls BPF program
        let builder = MemtrackSkelBuilder::default();
        let open_object = Box::leak(Box::new(MaybeUninit::uninit()));
        let open_skel = builder
            .open(open_object)
            .context("Failed to open syscalls BPF skeleton")?;

        let skel = Box::new(
            open_skel
                .load()
                .context("Failed to load syscalls BPF skeleton")?,
        );

        Ok(Self {
            skel,
            probes: Vec::new(),
        })
    }

    pub fn add_tracked_pid(&mut self, pid: i32) -> Result<()> {
        self.skel
            .maps
            .tracked_pids
            .update(
                &pid.to_le_bytes(),
                &1u8.to_le_bytes(),
                libbpf_rs::MapFlags::ANY,
            )
            .context("Failed to add PID to uprobes tracked set")?;

        Ok(())
    }

    /// Enable event tracking
    pub fn enable_tracking(&mut self) -> Result<()> {
        let key = 0u32;
        let value = true as u8;
        self.skel
            .maps
            .tracking_enabled
            .update(
                &key.to_le_bytes(),
                &value.to_le_bytes(),
                libbpf_rs::MapFlags::ANY,
            )
            .context("Failed to enable tracking")?;
        Ok(())
    }

    /// Read the count of events dropped because the ring buffer was full.
    pub fn dropped_events_count(&self) -> Result<u64> {
        let key = 0u32;
        let value = self
            .skel
            .maps
            .dropped_events
            .lookup(&key.to_le_bytes(), libbpf_rs::MapFlags::ANY)
            .context("Failed to read dropped_events counter")?
            .ok_or_else(|| anyhow!("dropped_events slot 0 missing"))?;

        let bytes: [u8; 8] = value
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("dropped_events value has unexpected size"))?;
        Ok(u64::from_le_bytes(bytes))
    }

    /// Disable event tracking
    pub fn disable_tracking(&mut self) -> Result<()> {
        let key = 0u32;
        let value = false as u8;
        self.skel
            .maps
            .tracking_enabled
            .update(
                &key.to_le_bytes(),
                &value.to_le_bytes(),
                libbpf_rs::MapFlags::ANY,
            )
            .context("Failed to disable tracking")?;
        Ok(())
    }

    // =========================================================================
    // Allocation probe functions (symbol passed at call time)
    // =========================================================================
    attach_uprobe_uretprobe!(attach_malloc, uprobe_malloc, uretprobe_malloc);
    attach_uprobe_uretprobe!(attach_calloc, uprobe_calloc, uretprobe_calloc);
    attach_uprobe_uretprobe!(attach_realloc, uprobe_realloc, uretprobe_realloc);
    attach_uprobe_uretprobe!(
        attach_aligned_alloc,
        uprobe_aligned_alloc,
        uretprobe_aligned_alloc
    );
    attach_uprobe_uretprobe!(attach_memalign, uprobe_memalign, uretprobe_memalign);
    attach_uprobe!(attach_free, uprobe_free);

    // =========================================================================
    // Attach methods grouped by allocator
    // =========================================================================

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
                self.attach_memalign_if_found(
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
    attach_tracepoint!(sched_fork);

    pub fn attach_tracepoints(&mut self) -> Result<()> {
        self.attach_sched_fork()?;
        Ok(())
    }

    /// Start polling, forwarding each event over the returned channel.
    pub fn start_polling_with_channel(
        &self,
        poll_timeout_ms: u64,
    ) -> Result<(
        RingBufferPoller,
        crossbeam_channel::Receiver<runner_shared::artifacts::MemtrackEvent>,
    )> {
        // Use the syscalls skeleton's ring buffer (both programs share the same one)
        RingBufferPoller::with_channel(&self.skel.maps.events, poll_timeout_ms)
    }
}

impl MemtrackBpf {
    /// Detach all BPF links in parallel. Closing a uprobe link blocks on two
    /// RCU grace periods in the kernel, but concurrent waiters share grace
    /// periods, so closing from many threads scales near-linearly.
    pub fn detach_probes(&mut self) {
        const DETACH_THREADS: usize = 32;

        let mut probes = std::mem::take(&mut self.probes);
        if probes.is_empty() {
            return;
        }

        debug!("Detaching {} BPF links", probes.len());
        let start = std::time::Instant::now();
        let chunk_size = probes.len().div_ceil(DETACH_THREADS);
        std::thread::scope(|scope| {
            while !probes.is_empty() {
                let split_at = probes.len().saturating_sub(chunk_size);
                let chunk = probes.split_off(split_at);
                scope.spawn(move || drop(chunk));
            }
        });
        debug!("Detached BPF links in {:?}", start.elapsed());
    }
}

impl Drop for MemtrackBpf {
    fn drop(&mut self) {
        self.detach_probes();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Allocator entry points must resolve to file offsets; a symbol that
    /// silently fails to resolve attaches nothing and loses all events.
    #[test]
    fn libc_allocator_symbols_resolve_to_offsets() {
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
        let libc_path = maps
            .lines()
            .find_map(|line| {
                let path = line.split_whitespace().last()?;
                path.contains("libc.so.6").then(|| path.to_owned())
            })
            .expect("test process has no mapped libc.so.6");

        let symbols = resolve_symbol_offsets(Path::new(&libc_path)).unwrap();
        for symbol in ["malloc", "calloc", "realloc", "free"] {
            assert!(
                symbols.offset(symbol).is_some(),
                "{symbol} in {libc_path} did not resolve to a file offset"
            );
        }
    }
}
