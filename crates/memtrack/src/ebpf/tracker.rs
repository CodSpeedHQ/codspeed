use crate::ebpf::attach_worker::AttachWorker;
use crate::ebpf::spawn::{resume, spawn_stopped, wrap_stopped};
use crate::ebpf::{BpfVariant, MemtrackBpf, OwnershipMaps};
use crate::prelude::*;
use crate::session::Session;
use parking_lot::Mutex;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::Arc;
use std::sync::mpsc;
use typed_builder::TypedBuilder;

/// Which probes a tracker attaches. [`Tracker::new`] resolves these from the
/// environment, [`Tracker::with_options`] takes them directly.
#[derive(Debug, Clone, Copy, TypedBuilder)]
pub struct TrackerOptions {
    /// Attach allocator uprobes (malloc/free/calloc/...) through the
    /// exec-mapping watcher. The coarse mmap/munmap/brk tracepoints fire either
    /// way, so disabling this still yields memory events.
    #[builder(default = true)]
    pub allocators: bool,
    /// Reconstruct per-process RSS from the folio rmap fentry hooks.
    #[builder(default = false)]
    pub rmap: bool,
}

impl TrackerOptions {
    fn from_env() -> Self {
        Self::builder()
            .allocators(!matches!(
                std::env::var("CODSPEED_MEMTRACK_TRACK_ALLOCATORS").as_deref(),
                Ok("0") | Ok("false")
            ))
            .rmap(std::env::var("CODSPEED_MEMTRACK_TRACK_RMAP").is_ok_and(|v| v == "1"))
            .build()
    }
}

pub struct Tracker {
    bpf: Arc<Mutex<MemtrackBpf>>,
    worker: Mutex<Option<AttachWorker>>,
}

impl Tracker {
    /// Create a new tracker. The exec-mapping watcher discovers and attaches
    /// allocator probes as the tracked process tree maps executable files.
    pub fn new() -> Result<Self> {
        Self::with_options(TrackerOptions::from_env())
    }

    /// Create a tracker from an explicit probe selection rather than the environment.
    pub fn with_options(options: TrackerOptions) -> Result<Self> {
        Self::build(
            MemtrackBpf::new_with_rmap(options.rmap)?,
            options.allocators,
        )
    }

    /// Like [`Tracker::new`], but pinned to a specific BPF variant instead of
    /// the detected one.
    pub fn with_variant(variant: BpfVariant) -> Result<Self> {
        let track_rmap = TrackerOptions::from_env().rmap;
        Self::build(MemtrackBpf::with_variant(variant, track_rmap)?, true)
    }

    /// Build a tracker: attach lifetime tracepoints (and rmap fentries when the
    /// skeleton was opened for them), plus, when `allocators` is set, the
    /// exec-mapping watcher and the on-demand allocator-attach worker.
    fn build(mut bpf: MemtrackBpf, allocators: bool) -> Result<Self> {
        Self::bump_memlock_rlimit()?;

        bpf.attach_tracepoints()?;
        if allocators {
            bpf.attach_exec_watcher()?;
        }

        let bpf = Arc::new(Mutex::new(bpf));
        let worker = if allocators {
            Some(AttachWorker::start(bpf.clone())?)
        } else {
            None
        };

        Ok(Self {
            bpf,
            worker: Mutex::new(worker),
        })
    }

    /// Spawn `cmd` under tracking: the target is wrapped so it stops itself
    /// before exec'ing, its pid is armed while stopped, then it is resumed. When
    /// the tracker runs an exec-mapping watcher, arming the pid before resume
    /// ensures no allocation mapping escapes untracked.
    ///
    /// `uid_gid` drops the child's privileges (a `Command`'s uid/gid cannot be
    /// read back, so it cannot be preserved through the wrap).
    pub fn spawn(&self, cmd: &Command, uid_gid: Option<(u32, u32)>) -> Result<Session> {
        let mut wrapped = wrap_stopped(cmd);
        if let Some((uid, gid)) = uid_gid {
            wrapped.uid(uid).gid(gid);
        }

        let child = spawn_stopped(&mut wrapped)?;
        let pid = child.id() as i32;
        if let Some(worker) = self.worker.lock().as_ref() {
            worker.set_root_pid(pid);
        }

        let (tx, rx) = mpsc::channel();
        let poller = {
            let mut bpf = self.bpf.lock();
            bpf.add_tracked_pid(pid)?;
            bpf.poll_events_with_channel(10, tx)?
        };
        resume(pid)?;

        Ok(Session::new(child, rx, poller))
    }

    /// Enable allocator-event tracking in the BPF program. Lifetime events
    /// (rss_stat, rmap, fork/exec/exit) are emitted for tracked pids
    /// regardless of this toggle.
    pub fn enable_tracking(&self) -> Result<()> {
        self.bpf.lock().enable_tracking()
    }

    /// Disable allocator-event tracking in the BPF program
    pub fn disable_tracking(&self) -> Result<()> {
        self.bpf.lock().disable_tracking()
    }

    /// Number of events the kernel dropped because the ring buffer was full.
    /// A non-zero value means the resulting trace is incomplete.
    pub fn dropped_events_count(&self) -> Result<u64> {
        self.bpf.lock().dropped_events_count()
    }

    /// Only meaningful while the BPF object is alive; teardown frees the maps.
    pub fn ownership_maps(&self) -> Result<OwnershipMaps> {
        self.bpf.lock().ownership_maps()
    }

    /// Stop the attach worker, if any, and surface any fatal error it recorded,
    /// including missed exec mappings (incomplete allocator coverage). A tracker
    /// without an allocator watcher has no worker, so this is a no-op.
    pub fn finish(&self) -> Result<()> {
        match self.worker.lock().take() {
            Some(worker) => worker.finish(),
            None => Ok(()),
        }
    }

    /// Detach all attached probes. Called explicitly at teardown because the
    /// process may exit without ever dropping the tracker (the IPC thread holds
    /// an Arc clone), in which case the kernel would close each link fd serially.
    pub fn detach(&self) {
        self.bpf.lock().detach_probes();
    }

    /// Bump RLIMIT_MEMLOCK for kernels older than 5.11. Newer kernels account BPF
    /// memory against the cgroup, so a denied raise (no CAP_SYS_RESOURCE) is harmless.
    fn bump_memlock_rlimit() -> Result<()> {
        let rlimit = libc::rlimit {
            rlim_cur: libc::RLIM_INFINITY,
            rlim_max: libc::RLIM_INFINITY,
        };

        let ret = unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &rlimit) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            debug!(
                "Could not raise RLIMIT_MEMLOCK ({err}); continuing since kernels >= 5.11 don't require it"
            );
        }

        Ok(())
    }
}
