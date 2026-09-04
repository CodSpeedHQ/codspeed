use crate::prelude::*;
use libbpf_rs::Link;
use libbpf_rs::skel::OpenSkel;
use libbpf_rs::skel::SkelBuilder;
use std::collections::HashMap;
use std::mem::MaybeUninit;
use std::path::{Path, PathBuf};

use crate::ebpf::poller::RingBufferPoller;

mod token {
    include!(concat!(env!("OUT_DIR"), "/memtrack_token.skel.rs"));
}
mod legacy {
    include!(concat!(env!("OUT_DIR"), "/memtrack_legacy.skel.rs"));
}

#[macro_use]
mod macros;
mod allocator;
mod maps;
mod rmap;
mod tracking;

pub use maps::OwnershipMaps;
pub use rmap::RmapSupport;

use crate::bpf_token::has_delegated_bpf_token;

/// Which attach mechanism a loaded skeleton uses for its uprobes. See
/// `src/ebpf/c/utils/variant.h` for why only one of them is delegatable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BpfVariant {
    /// `uprobe_multi` links, attached through `bpf()` so a BPF token can
    /// authorize them. Requires kernel >= 6.6.
    Token,
    /// `perf_event_open`-based uprobes, for kernels predating `uprobe_multi`.
    /// Needs `CAP_PERFMON` in the init user namespace.
    Legacy,
}

/// The loaded skeleton. Both variants come from the same BPF source and expose
/// identical maps and program names; only the uprobe attach mechanism differs.
pub(super) enum Skel {
    Token(Box<token::MemtrackTokenSkel<'static>>),
    Legacy(Box<legacy::MemtrackLegacySkel<'static>>),
}

/// Device and inode of our PID namespace, in the form
/// `bpf_get_ns_current_pid_tgid` takes them. `None` if unreadable, which leaves
/// the programs reporting global PIDs.
fn current_pidns_ids() -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata("/proc/self/ns/pid").ok()?;
    Some((meta.dev(), meta.ino()))
}

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

fn page_shift() -> Result<u32> {
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    ensure!(page_size > 0, "Failed to read system page size");
    Ok((page_size as u32).trailing_zeros())
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

pub struct MemtrackBpf {
    pub(super) skel: Skel,
    pub(super) probes: Vec<Link>,
    rmap: RmapSupport,
    /// Attach sites already claimed, as (program, library, offset, retprobe).
    /// Allocator entry points share addresses through aliases (`free`,
    /// `cfree` and `__libc_free` are one symbol in glibc), and attaching the
    /// same program twice at one address makes it run twice per call.
    pub(super) attached_sites: std::collections::HashSet<(&'static str, PathBuf, usize, bool)>,
}

impl MemtrackBpf {
    /// Reserve an attach site, returning false if it is already instrumented.
    pub(super) fn claim_site(
        &mut self,
        prog: &'static str,
        lib_path: &Path,
        offset: usize,
        retprobe: bool,
    ) -> bool {
        self.attached_sites
            .insert((prog, lib_path.to_path_buf(), offset, retprobe))
    }
}

impl MemtrackBpf {
    /// Load the skeleton, picking the variant a BPF token is available for.
    pub fn new_with_rmap(track_rmap: bool) -> Result<Self> {
        let variant = if has_delegated_bpf_token() {
            BpfVariant::Token
        } else {
            BpfVariant::Legacy
        };
        Self::with_variant(variant, track_rmap)
    }

    /// Load a specific variant rather than the one [`Self::new_with_rmap`]
    /// would detect. Either attaches given host privileges; the token only
    /// matters when `bpf()` is called from an unprivileged user namespace.
    pub fn with_variant(variant: BpfVariant, track_rmap: bool) -> Result<Self> {
        crate::kernel::KernelBtf::ensure_available()?;

        let page_shift = page_shift()?;
        let rmap = if track_rmap {
            RmapSupport::detect()
        } else {
            RmapSupport::Unsupported
        };

        // Both variants expose `rodata_data` and `progs` under the same field
        // names, but as distinct generated types, so this can't be a function
        // over the two.
        macro_rules! open_and_load {
            ($builder:expr, $skel:path) => {{
                let open_object = Box::leak(Box::new(MaybeUninit::uninit()));
                let mut open_skel = $builder
                    .open(open_object)
                    .context("Failed to open memtrack BPF skeleton")?;

                {
                    let rodata = open_skel
                        .maps
                        .rodata_data
                        .as_deref_mut()
                        .context("rodata map missing")?;
                    rodata.page_shift = page_shift;
                    if let Some((dev, ino)) = current_pidns_ids() {
                        rodata.target_pidns_dev = dev;
                        rodata.target_pidns_ino = ino;
                    }
                }

                // Autoload is decided before load(), so fentries whose targets
                // the kernel lacks have to be turned off here or the whole
                // skeleton fails to load.
                macro_rules! disable_rmap_prog {
                    ($name:ident) => {
                        paste::paste! {
                            open_skel.progs.[<fentry_ $name>].set_autoload(false);
                        }
                    };
                }
                // Mirrors the attach match in `tracking.rs`.
                match rmap {
                    RmapSupport::Unsupported => {
                        for_each_rmap_core_prog!(disable_rmap_prog);
                        for_each_rmap_pud_prog!(disable_rmap_prog);
                    }
                    RmapSupport::Core => {
                        for_each_rmap_pud_prog!(disable_rmap_prog);
                    }
                    RmapSupport::CoreAndPud => {}
                }

                $skel(Box::new(
                    open_skel
                        .load()
                        .context("Failed to load memtrack BPF skeleton")?,
                ))
            }};
        }

        let skel = match variant {
            BpfVariant::Token => {
                open_and_load!(token::MemtrackTokenSkelBuilder::default(), Skel::Token)
            }
            BpfVariant::Legacy => {
                open_and_load!(legacy::MemtrackLegacySkelBuilder::default(), Skel::Legacy)
            }
        };

        Ok(Self {
            skel,
            probes: Vec::new(),
            rmap,
            attached_sites: std::collections::HashSet::new(),
        })
    }

    /// Poll the allocation-event ring buffer into `tx`. The returned poller
    /// keeps the pipeline alive; events stop flowing when it is dropped.
    pub fn poll_events_with_channel(
        &self,
        poll_interval_ms: u64,
        tx: std::sync::mpsc::Sender<runner_shared::artifacts::MemtrackEvent>,
    ) -> Result<RingBufferPoller> {
        with_skel!(self, skel => RingBufferPoller::new(
            &skel.maps.events,
            crate::ebpf::events::parse_event,
            tx,
            poll_interval_ms,
        ))
    }

    /// Poll the exec-mapping request ring buffer into `tx`. Same contract as
    /// [`Self::poll_events_with_channel`].
    pub(crate) fn poll_attach_with_channel(
        &self,
        poll_interval_ms: u64,
        tx: std::sync::mpsc::Sender<crate::ebpf::events::AttachRequest>,
    ) -> Result<RingBufferPoller> {
        with_skel!(self, skel => RingBufferPoller::new(
            &skel.maps.attach_requests,
            crate::ebpf::events::AttachRequest::parse,
            tx,
            poll_interval_ms,
        ))
    }

    /// Number of currently-attached probes/links.
    pub fn probe_count(&self) -> usize {
        self.probes.len()
    }

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
