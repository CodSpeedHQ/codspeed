use crate::prelude::*;
use libbpf_rs::Link;
use libbpf_rs::skel::OpenSkel;
use libbpf_rs::skel::SkelBuilder;
use libbpf_rs::{MapCore, UprobeMultiOpts, UprobeOpts};
use paste::paste;
use std::mem::MaybeUninit;
use std::path::Path;

use crate::allocators::AllocatorKind;
use crate::ebpf::poller::RingBufferPoller;

/// Attach via `bpf()` links (uprobe_multi + tp_btf), authorized by a delegated
/// BPF token. Requires a kernel with uprobe_multi (>= 6.6) and is the only path
/// that works inside the macro-agent sandbox.
mod token {
    include!(concat!(env!("OUT_DIR"), "/memtrack_token.skel.rs"));
}
/// Classic perf-based attach (uprobe/uretprobe + perf tracepoint). Works on
/// kernels predating uprobe_multi but needs CAP_PERFMON in the init user
/// namespace, so it cannot be delegated into the sandbox.
mod perf {
    include!(concat!(env!("OUT_DIR"), "/memtrack_perf.skel.rs"));
}

/// Whether a delegated BPF token is available to this process. libbpf reads the
/// token from the directory named by `LIBBPF_BPF_TOKEN_PATH` and attaches it to
/// its `bpf()` calls; its presence is also what tells us to use the bpf()-link
/// attach paths instead of the perf-based ones.
fn has_delegated_bpf_token() -> bool {
    std::env::var_os("LIBBPF_BPF_TOKEN_PATH").is_some_and(|p| !p.is_empty())
}

/// Which set of attach mechanisms a loaded skeleton uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    /// `bpf()`-link attach (uprobe_multi + tp_btf); delegatable via a BPF token,
    /// requires uprobe_multi (kernel >= 6.6).
    Token,
    /// Classic perf-based attach (uprobe/uretprobe + perf tracepoint); works on
    /// older kernels but needs CAP_PERFMON in the init user namespace.
    Perf,
}

/// The loaded skeleton, in whichever attach flavor [`MemtrackBpf::new`] selected.
/// Both flavors are generated from the same BPF source and share identical maps;
/// only the programs' attach mechanism differs.
enum Skel {
    Token(Box<token::MemtrackTokenSkel<'static>>),
    Perf(Box<perf::MemtrackPerfSkel<'static>>),
}

/// Run `$body` against the loaded skeleton, binding `$skel` to the concrete
/// skeleton of whichever flavor is active. Used where both flavors expose the
/// same field/method names (all maps, and the auto-attached programs).
macro_rules! with_skel {
    ($self:expr, $skel:ident => $body:expr) => {
        match &$self.skel {
            Skel::Token($skel) => $body,
            Skel::Perf($skel) => $body,
        }
    };
    (mut $self:expr, $skel:ident => $body:expr) => {
        match &mut $self.skel {
            Skel::Token($skel) => $body,
            Skel::Perf($skel) => $body,
        }
    };
}

/// Device and inode of our PID namespace, as `bpf_get_ns_current_pid_tgid`
/// expects them (`stat` of `/proc/self/ns/pid`). Returns `None` if it can't be
/// read, in which case tracking falls back to global PIDs.
fn current_pidns_ids() -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata("/proc/self/ns/pid").ok()?;
    Some((meta.dev(), meta.ino()))
}

/// Resolve a symbol to its file offset in the target library.
///
/// uprobe_multi attaches by offset rather than symbol name: libbpf's own
/// name-based resolution misses some libc symbols (returning ENOENT), so we
/// resolve from the ELF symbol tables ourselves, exactly as the offset the probe
/// needs. Checks both `.symtab` and `.dynsym`.
fn resolve_symbol_offset(lib_path: &Path, symbol_name: &str) -> Result<usize> {
    use object::{Object, ObjectSymbol};

    let data = std::fs::read(lib_path)?;
    let file = object::File::parse(&*data)?;

    for symbol in file.symbols().chain(file.dynamic_symbols()) {
        if !symbol.is_definition() {
            continue;
        }

        let Ok(name) = symbol.name() else {
            continue;
        };

        if name == symbol_name {
            let addr = symbol.address();
            if addr != 0 {
                return Ok(addr as usize);
            }
        }
    }

    bail!("Symbol {symbol_name} not found in {}", lib_path.display())
}

/// Attach a single program (entry or return) to `symbol` in `lib_path`, using
/// the attach mechanism matching the loaded skeleton flavor.
///
/// `$prog` is the program field name, identical across both flavors; `$skel`
/// binds the concrete skeleton so field access type-checks in each arm.
macro_rules! attach_one {
    ($self:expr, $prog:ident, $lib_path:expr, $offset:expr, $retprobe:expr) => {{
        let lib_path = $lib_path;
        let offset = $offset;
        let retprobe = $retprobe;
        match &mut $self.skel {
            Skel::Token(skel) => skel.progs.$prog.attach_uprobe_multi_with_opts(
                -1,
                lib_path,
                "",
                UprobeMultiOpts {
                    offsets: vec![offset],
                    retprobe,
                    ..Default::default()
                },
            ),
            Skel::Perf(skel) => skel.progs.$prog.attach_uprobe_with_opts(
                -1,
                lib_path,
                offset,
                UprobeOpts {
                    retprobe,
                    ..Default::default()
                },
            ),
        }
    }};
}

/// Macro to attach a function with both entry and return probes.
/// Also generates a `try_attach_*` variant that logs errors instead of returning them.
///
/// Resolves the symbol to a file offset and attaches through whichever
/// mechanism the loaded skeleton uses. Fails if the symbol is not found.
macro_rules! attach_uprobe_uretprobe {
    ($name:ident, $prog_entry:ident, $prog_return:ident) => {
        fn $name(&mut self, lib_path: &Path, symbol: &str) -> Result<()> {
            let offset = resolve_symbol_offset(lib_path, symbol)?;

            let link = attach_one!(self, $prog_entry, lib_path, offset, false).context(format!(
                "Failed to attach {} uprobe in {}",
                symbol,
                lib_path.display()
            ))?;
            self.probes.push(link);

            let link = attach_one!(self, $prog_return, lib_path, offset, true).context(format!(
                "Failed to attach {} uretprobe in {}",
                symbol,
                lib_path.display()
            ))?;
            self.probes.push(link);

            Ok(())
        }

        paste! {
            fn [<try_ $name>](&mut self, lib_path: &Path, symbol: &str) {
                let result = self.$name(lib_path, symbol);
                log::trace!("{} uprobe attach result: {:?}", symbol, result);
            }
        }
    };
}

/// Macro to attach a function with only an entry probe (no return probe).
/// Also generates a `try_attach_*` variant that logs errors instead of returning them.
///
/// Resolves the symbol to a file offset and attaches through whichever
/// mechanism the loaded skeleton uses. Fails if the symbol is not found.
macro_rules! attach_uprobe {
    ($name:ident, $prog:ident) => {
        fn $name(&mut self, lib_path: &Path, symbol: &str) -> Result<()> {
            let offset = resolve_symbol_offset(lib_path, symbol)?;

            let link = attach_one!(self, $prog, lib_path, offset, false).context(format!(
                "Failed to attach {} uprobe in {}",
                symbol,
                lib_path.display()
            ))?;
            self.probes.push(link);
            Ok(())
        }

        paste! {
            fn [<try_ $name>](&mut self, lib_path: &Path, symbol: &str) {
                let result = self.$name(lib_path, symbol);
                log::trace!("{} uprobe attach result: {:?}", symbol, result);
            }
        }
    };
}

pub struct MemtrackBpf {
    skel: Skel,
    probes: Vec<Link>,
}

/// Set the PID-namespace rodata on an open skeleton, so both flavors resolve
/// PIDs the same way. `$open` is the open skeleton, of either flavor.
macro_rules! set_pidns_rodata {
    ($open:expr, $ids:expr) => {
        if let (Some((dev, ino)), Some(rodata)) = ($ids, $open.maps.rodata_data.as_deref_mut()) {
            rodata.target_pidns_dev = dev;
            rodata.target_pidns_ino = ino;
        }
    };
}

impl MemtrackBpf {
    /// Load the skeleton, selecting the attach flavor from the environment: a
    /// delegated BPF token means we are the sandboxed workload and must use the
    /// token path; otherwise the perf path, which also supports kernels
    /// predating uprobe_multi.
    pub fn new() -> Result<Self> {
        let flavor = if has_delegated_bpf_token() {
            Flavor::Token
        } else {
            Flavor::Perf
        };
        Self::with_flavor(flavor)
    }

    /// Load the skeleton for a specific attach flavor, bypassing environment
    /// detection. Used by tests to exercise either path directly; both attach
    /// fine on a privileged kernel without a token (the token only authorizes
    /// `bpf()` from inside the unprivileged sandbox).
    pub fn with_flavor(flavor: Flavor) -> Result<Self> {
        // Resolve PIDs relative to our own PID namespace. When we run inside a
        // namespace (e.g. the macro-agent sandbox), the PIDs we register are
        // namespace-local while eBPF sees global PIDs; telling the programs which
        // namespace to resolve in keeps tracking correct. In the init namespace
        // this resolves to global PIDs, matching the previous behavior.
        let pidns_ids = current_pidns_ids();

        let skel = match flavor {
            Flavor::Token => {
                let builder = token::MemtrackTokenSkelBuilder::default();
                let open_object = Box::leak(Box::new(MaybeUninit::uninit()));
                let mut open_skel = builder
                    .open(open_object)
                    .context("Failed to open memtrack BPF skeleton")?;
                set_pidns_rodata!(open_skel, pidns_ids);
                Skel::Token(Box::new(
                    open_skel
                        .load()
                        .context("Failed to load memtrack BPF skeleton")?,
                ))
            }
            Flavor::Perf => {
                let builder = perf::MemtrackPerfSkelBuilder::default();
                let open_object = Box::leak(Box::new(MaybeUninit::uninit()));
                let mut open_skel = builder
                    .open(open_object)
                    .context("Failed to open memtrack BPF skeleton")?;
                set_pidns_rodata!(open_skel, pidns_ids);
                Skel::Perf(Box::new(
                    open_skel
                        .load()
                        .context("Failed to load memtrack BPF skeleton")?,
                ))
            }
        };

        Ok(Self {
            skel,
            probes: Vec::new(),
        })
    }

    pub fn add_tracked_pid(&mut self, pid: i32) -> Result<()> {
        with_skel!(self, skel => skel.maps.tracked_pids.update(
            &pid.to_le_bytes(),
            &1u8.to_le_bytes(),
            libbpf_rs::MapFlags::ANY,
        ))
        .context("Failed to add PID to uprobes tracked set")?;

        Ok(())
    }

    /// Enable event tracking
    pub fn enable_tracking(&mut self) -> Result<()> {
        let key = 0u32;
        let value = true as u8;
        with_skel!(self, skel => skel.maps.tracking_enabled.update(
            &key.to_le_bytes(),
            &value.to_le_bytes(),
            libbpf_rs::MapFlags::ANY,
        ))
        .context("Failed to enable tracking")?;
        Ok(())
    }

    /// Read the count of events dropped because the ring buffer was full.
    pub fn dropped_events_count(&self) -> Result<u64> {
        let key = 0u32;
        let value = with_skel!(self, skel => skel
            .maps
            .dropped_events
            .lookup(&key.to_le_bytes(), libbpf_rs::MapFlags::ANY))
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
        with_skel!(self, skel => skel.maps.tracking_enabled.update(
            &key.to_le_bytes(),
            &value.to_le_bytes(),
            libbpf_rs::MapFlags::ANY,
        ))
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

    /// Attach probes for a specific allocator kind.
    /// This attaches both standard probes (if the allocator exports them) and
    /// allocator-specific prefixed probes.
    pub fn attach_allocator_probes(&mut self, kind: AllocatorKind, lib_path: &Path) -> Result<()> {
        debug!(
            "Attaching {} probes to: {}",
            kind.name(),
            lib_path.display()
        );

        match kind {
            AllocatorKind::Libc => {
                // Libc only has standard probes, and they must succeed
                self.attach_libc_probes(lib_path)
            }
            AllocatorKind::LibCpp => {
                // libc++ exports C++ operator new/delete symbols
                self.attach_libcpp_probes(lib_path)
            }
            AllocatorKind::Jemalloc => {
                // Jemalloc exposes libc/libcpp compatible allocator functions:
                let _ = self.attach_libc_probes(lib_path);
                let _ = self.attach_libcpp_probes(lib_path);
                self.attach_jemalloc_probes(lib_path)
            }
            AllocatorKind::Mimalloc => {
                // Mimalloc exposes libc/libcpp compatible allocator functions:
                let _ = self.attach_libc_probes(lib_path);
                let _ = self.attach_libcpp_probes(lib_path);
                self.attach_mimalloc_probes(lib_path)
            }
            AllocatorKind::Tcmalloc => {
                // Tcmalloc exposes libc/libcpp compatible allocator functions:
                let _ = self.attach_libc_probes(lib_path);
                let _ = self.attach_libcpp_probes(lib_path);
                self.attach_tcmalloc_probes(lib_path)
            }
        }
    }

    fn attach_standard_probes(
        &mut self,
        lib_path: &Path,
        prefixes: &[&str],
        suffixes: &[&str],
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
                self.try_attach_malloc(lib_path, &format!("{prefix}malloc{suffix}"));
                self.try_attach_malloc(lib_path, &format!("{prefix}valloc{suffix}"));
                self.try_attach_malloc(lib_path, &format!("{prefix}pvalloc{suffix}"));
                self.try_attach_calloc(lib_path, &format!("{prefix}calloc{suffix}"));
                self.try_attach_realloc(lib_path, &format!("{prefix}realloc{suffix}"));
                self.try_attach_aligned_alloc(lib_path, &format!("{prefix}aligned_alloc{suffix}"));
                self.try_attach_memalign(lib_path, &format!("{prefix}memalign{suffix}"));
                self.try_attach_memalign(lib_path, &format!("{prefix}posix_memalign{suffix}"));
                self.try_attach_free(lib_path, &format!("{prefix}free{suffix}"));
                self.try_attach_free(lib_path, &format!("{prefix}free_sized{suffix}"));
                self.try_attach_free(lib_path, &format!("{prefix}free_aligned_sized{suffix}"));
                self.try_attach_free(lib_path, &format!("{prefix}cfree{suffix}"));
            }
        }

        Ok(())
    }

    /// Attach standard library allocation probes (libc-style: malloc, free, calloc, etc.)
    /// This works for libc and allocators that export standard symbol names.
    /// For non-libc allocators, standard names are optional - just try them silently.
    fn attach_libc_probes(&mut self, lib_path: &Path) -> Result<()> {
        self.attach_standard_probes(lib_path, &[], &[])
    }

    /// Attach C++ operator new/delete probes.
    /// These are mangled C++ symbols that wrap the underlying allocator.
    /// C++ operators have identical signatures to malloc/free, so we reuse those handlers.
    fn attach_libcpp_probes(&mut self, lib_path: &Path) -> Result<()> {
        self.try_attach_malloc(lib_path, "_Znwm"); // operator new(size_t)
        self.try_attach_malloc(lib_path, "_Znam"); // operator new[](size_t)
        self.try_attach_malloc(lib_path, "_ZnwmSt11align_val_t"); // operator new(size_t, std::align_val_t)
        self.try_attach_malloc(lib_path, "_ZnamSt11align_val_t"); // operator new[](size_t, std::align_val_t)
        self.try_attach_free(lib_path, "_ZdlPv"); // operator delete(void*)
        self.try_attach_free(lib_path, "_ZdaPv"); // operator delete[](void*)
        self.try_attach_free(lib_path, "_ZdlPvm"); // operator delete(void*, size_t) - C++14 sized delete
        self.try_attach_free(lib_path, "_ZdaPvm"); // operator delete[](void*, size_t) - C++14 sized delete
        self.try_attach_free(lib_path, "_ZdlPvSt11align_val_t"); // operator delete(void*, std::align_val_t)
        self.try_attach_free(lib_path, "_ZdaPvSt11align_val_t"); // operator delete[](void*, std::align_val_t)
        self.try_attach_free(lib_path, "_ZdlPvmSt11align_val_t"); // operator delete(void*, size_t, std::align_val_t)
        self.try_attach_free(lib_path, "_ZdaPvmSt11align_val_t"); // operator delete[](void*, size_t, std::align_val_t)

        Ok(())
    }

    /// Attach jemalloc-specific probes (prefixed and extended API).
    fn attach_jemalloc_probes(&mut self, lib_path: &Path) -> Result<()> {
        // The following functions are used in Rust when setting a global allocator:
        // - rust_alloc: _rjem_malloc and _rjem_mallocx
        // - rust_alloc_zeroed: _rjem_mallocx / _rjem_calloc
        // - rust_dealloc: _rjem_sdallocx
        // - rust_realloc: _rjem_realloc / _rjem_rallocx

        // je_* API (internal jemalloc functions, static linking)
        // _rjem_* API (Rust jemalloc crate, dynamic linking)
        let prefixes = ["je_", "_rjem_"];
        let suffixes = ["", "_default"];

        self.attach_standard_probes(lib_path, &prefixes, &suffixes)?;

        // Non-standard API that has an additional flag parameter
        // See: https://jemalloc.net/jemalloc.3.html
        for prefix in prefixes {
            for suffix in suffixes {
                self.try_attach_malloc(lib_path, &format!("{prefix}mallocx{suffix}"));
                self.try_attach_realloc(lib_path, &format!("{prefix}rallocx{suffix}"));
                self.try_attach_free(lib_path, &format!("{prefix}dallocx{suffix}"));
                self.try_attach_free(lib_path, &format!("{prefix}sdallocx{suffix}"));
            }
        }

        Ok(())
    }

    /// Attach mimalloc-specific probes (mi_* API).
    fn attach_mimalloc_probes(&mut self, lib_path: &Path) -> Result<()> {
        // The following functions are used in Rust when setting a global allocator:
        // - mi_malloc_aligned
        // - mi_free
        // - mi_realloc_aligned
        // - mi_zalloc_aligned

        self.attach_standard_probes(lib_path, &["mi_"], &[])?;

        // Zero-initialized and aligned variants
        self.try_attach_malloc(lib_path, "mi_malloc_aligned");
        self.try_attach_calloc(lib_path, "mi_zalloc");
        self.try_attach_calloc(lib_path, "mi_zalloc_aligned");
        self.try_attach_realloc(lib_path, "mi_realloc_aligned");

        Ok(())
    }

    /// Attach TCMalloc probes ( tc_* API).
    ///
    /// See:
    /// - https://github.com/google/tcmalloc/blob/master/docs/reference.md
    /// - https://github.com/gperftools/gperftools/blob/a47243150ec41097602730ff8779fafcc172d1fb/src/tcmalloc.cc#L178-L190
    fn attach_tcmalloc_probes(&mut self, lib_path: &Path) -> Result<()> {
        self.attach_standard_probes(lib_path, &["tc_"], &[])?;

        self.try_attach_free(lib_path, "free_sized");
        self.try_attach_free(lib_path, "free_aligned_sized");
        self.try_attach_free(lib_path, "sdallocx");

        Ok(())
    }

    pub fn attach_tracepoints(&mut self) -> Result<()> {
        // The fork hook auto-attaches by its section (tp_btf or classic
        // tracepoint, depending on the flavor); both go through `attach()`.
        let link = with_skel!(mut self, skel => skel.progs.tracepoint_sched_fork.attach())
            .context("Failed to attach sched_process_fork tracepoint")?;
        self.probes.push(link);
        Ok(())
    }

    /// Start polling with an mpsc channel for events
    pub fn start_polling_with_channel(
        &self,
        poll_timeout_ms: u64,
    ) -> Result<(
        RingBufferPoller,
        std::sync::mpsc::Receiver<runner_shared::artifacts::MemtrackEvent>,
    )> {
        with_skel!(self, skel => RingBufferPoller::with_channel(&skel.maps.events, poll_timeout_ms))
    }
}

impl Drop for MemtrackBpf {
    fn drop(&mut self) {
        if self.probes.len() > 10 {
            warn!(
                "Dropping the MemtrackBpf instance, this can take some time when having many probes attached"
            );
        }
    }
}
