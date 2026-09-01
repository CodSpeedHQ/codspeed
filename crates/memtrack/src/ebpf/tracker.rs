use crate::ebpf::attach_worker::AttachWorker;
use crate::ebpf::spawn::{resume, spawn_stopped, wrap_stopped};
use crate::ebpf::stacks::StackCaptureStats;
use crate::ebpf::{BpfVariant, MemtrackBpf, OwnershipMaps};
use crate::perf_mappings::PerfMappingPoller;
use crate::prelude::*;
use crate::session::Session;
use parking_lot::Mutex;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use typed_builder::TypedBuilder;

#[derive(Debug, Clone, Copy, TypedBuilder)]
pub struct TrackerOptions {
    /// BPF attach mechanism, or automatic detection when unset.
    #[builder(default)]
    pub variant: Option<BpfVariant>,
    /// Attach allocator uprobes (malloc/free/calloc/...) through the
    /// exec-mapping watcher.
    #[builder(default = true)]
    pub allocators: bool,
    /// Reconstruct per-process RSS from the folio rmap fentry hooks.
    #[builder(default = false)]
    pub rmap: bool,
    /// Capture allocation call stacks.
    #[builder(default = true)]
    pub stack_capture: bool,
    /// Maximum bytes of user stack to copy per captured call stack.
    #[builder(default = 8192)]
    pub stack_budget: u32,
}

impl TrackerOptions {
    fn from_env() -> Self {
        Self::builder()
            .allocators(!matches!(
                std::env::var("CODSPEED_MEMTRACK_TRACK_ALLOCATORS").as_deref(),
                Ok("0") | Ok("false")
            ))
            .rmap(std::env::var("CODSPEED_MEMTRACK_TRACK_RMAP").is_ok_and(|v| v == "1"))
            .stack_capture(!matches!(
                std::env::var("CODSPEED_MEMTRACK_CAPTURE_STACKS").as_deref(),
                Ok("0") | Ok("false")
            ))
            .stack_budget(
                std::env::var("CODSPEED_MEMTRACK_STACK_BUDGET")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(8192),
            )
            .build()
    }
}

impl Default for TrackerOptions {
    fn default() -> Self {
        Self::builder().build()
    }
}

pub struct Tracker {
    bpf: Arc<Mutex<MemtrackBpf>>,
    worker: Mutex<Option<AttachWorker>>,
    options: TrackerOptions,
    /// Number of native perf mapping records lost due to ring-buffer overflow.
    mapping_lost: Arc<AtomicU64>,
}

fn kill_and_wait(child: &mut std::process::Child) {
    // Cleanup is best effort so the setup error remains the returned error.
    let _ = child.kill();
    let _ = child.wait();
}

impl Tracker {
    /// Create a new tracker. The exec-mapping watcher discovers and attaches
    /// allocator probes as the tracked process tree maps executable files.
    pub fn new() -> Result<Self> {
        Self::with_options(TrackerOptions::from_env())
    }

    /// Create a tracker from an explicit probe selection rather than the environment.
    pub fn with_options(options: TrackerOptions) -> Result<Self> {
        let bpf = MemtrackBpf::new(&options)?;
        Self::build(bpf, options)
    }

    /// Build a tracker: attach lifetime tracepoints (and rmap fentries when the
    /// skeleton was opened for them), plus, when `allocators` is set, the
    /// exec-mapping watcher and the on-demand allocator-attach worker.
    fn build(mut bpf: MemtrackBpf, options: TrackerOptions) -> Result<Self> {
        Self::bump_memlock_rlimit()?;

        bpf.attach_tracepoints()?;
        if options.allocators {
            bpf.attach_exec_watcher()?;
        }

        let bpf = Arc::new(Mutex::new(bpf));
        let worker = if options.allocators {
            Some(AttachWorker::start(bpf.clone())?)
        } else {
            None
        };

        Ok(Self {
            bpf,
            worker: Mutex::new(worker),
            options,
            mapping_lost: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Spawn `cmd` under tracking: the target is wrapped so it stops itself
    /// before exec'ing, its pid is armed while stopped, then it is resumed. When
    /// the tracker runs an exec-mapping watcher, arming the pid before resume
    /// ensures no allocation mapping escapes untracked; spawning after
    /// [`Self::finish`] is an error in that mode.
    ///
    /// `uid_gid` drops the child's privileges (a `Command`'s uid/gid cannot be
    /// read back, so it cannot be preserved through the wrap).
    pub fn spawn(&self, cmd: &Command, uid_gid: Option<(u32, u32)>) -> Result<Session> {
        let capture_stacks = self.options.stack_capture;

        let mut wrapped = wrap_stopped(cmd);
        if let Some((uid, gid)) = uid_gid {
            wrapped.uid(uid).gid(gid);
        }

        let mut child = spawn_stopped(&mut wrapped)?;
        let pid = child.id() as i32;

        let setup = (|| -> Result<_> {
            match self.worker.lock().as_ref() {
                Some(worker) => worker.set_root_pid(pid),
                // No watcher to arm means exec mappings would be missed.
                None if self.options.allocators => bail!("tracker already finished"),
                None => {}
            }

            let (tx, rx) = mpsc::channel();
            let (poller, stack_poller) = {
                let mut bpf = self.bpf.lock();
                bpf.add_tracked_pid(pid)?;
                let stack_poller = capture_stacks
                    .then(|| bpf.poll_stacks(10, tx.clone()))
                    .transpose()?;
                (bpf.poll_events_with_channel(10, tx.clone())?, stack_poller)
            };
            let perf_mapping_poller = capture_stacks
                .then(|| PerfMappingPoller::start(pid, tx, self.mapping_lost.clone()))
                .transpose()?;

            Ok((rx, poller, stack_poller, perf_mapping_poller))
        })();
        let (rx, poller, stack_poller, perf_mapping_poller) = match setup {
            Ok(pollers) => pollers,
            Err(error) => {
                kill_and_wait(&mut child);
                return Err(error);
            }
        };

        if let Err(error) = resume(pid) {
            kill_and_wait(&mut child);
            return Err(error);
        }

        Ok(Session::new(
            child,
            rx,
            poller,
            stack_poller,
            perf_mapping_poller,
        ))
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
        Ok(self.bpf.lock().dropped_events_count()? + self.mapping_lost.load(Ordering::Relaxed))
    }

    /// Per-cause counts of stack captures that were skipped or truncated.
    pub fn stack_capture_stats(&self) -> Result<StackCaptureStats> {
        self.bpf.lock().stack_capture_stats()
    }

    pub fn stack_capture_enabled(&self) -> bool {
        self.options.stack_capture
    }

    /// Only meaningful while the BPF object is alive; teardown frees the maps.
    pub fn ownership_maps(&self) -> Result<OwnershipMaps> {
        self.bpf.lock().ownership_maps()
    }

    /// Stop the attach worker, if any, and surface any fatal error it recorded,
    /// including missed exec mappings (incomplete allocator coverage).
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
