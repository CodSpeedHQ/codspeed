use crate::ebpf::poller::RingBufferPoller;
use crate::prelude::*;
use crate::{AllocatorLib, ebpf::MemtrackBpf};
use crossbeam_channel::Receiver;
use runner_shared::artifacts::MemtrackEvent as Event;

pub struct Tracker {
    bpf: MemtrackBpf,
    poller: Option<RingBufferPoller>,
}

impl Tracker {
    /// Create a new tracker instance
    ///
    /// This will:
    /// - Initialize the BPF subsystem
    /// - Bump memlock limits
    /// - Attach uprobes to all libc instances
    /// - Attach tracepoints for fork tracking
    pub fn new() -> Result<Self> {
        let mut instance = Self::new_without_allocators()?;

        let allocators = AllocatorLib::find_all()?;
        debug!("Found {} allocator instance(s)", allocators.len());
        instance.attach_allocators(&allocators)?;

        Ok(instance)
    }

    pub fn new_without_allocators() -> Result<Self> {
        // Bump memlock limits
        Self::bump_memlock_rlimit()?;

        let mut bpf = MemtrackBpf::new()?;
        bpf.attach_tracepoints()?;

        Ok(Self { bpf, poller: None })
    }

    pub fn attach_allocators(&mut self, libs: &[AllocatorLib]) -> Result<()> {
        self.bpf.attach_allocators(libs)
    }

    pub fn attach_allocator(&mut self, lib: &AllocatorLib) -> Result<()> {
        self.bpf.attach_allocator_probes(lib.kind, &lib.path)
    }

    /// Start tracking allocations for a specific PID.
    ///
    /// Returns a receiver of allocation events. The poller is owned by the tracker
    /// and keeps running until [`Tracker::stop_polling`] is called or the tracker
    /// is dropped.
    pub fn track(&mut self, pid: i32) -> Result<Receiver<Event>> {
        self.bpf.add_tracked_pid(pid)?;
        debug!("Tracking PID {pid}");

        let (poller, event_rx) = self.bpf.start_polling_with_channel(10)?;
        self.poller = Some(poller);

        Ok(event_rx)
    }

    /// Stop the poll thread, draining ring-buffer stragglers. This closes the
    /// event channel returned by [`track`].
    pub fn stop_polling(&mut self) {
        self.poller.take();
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

    /// Enable event tracking in the BPF program
    pub fn enable(&mut self) -> anyhow::Result<()> {
        self.bpf.enable_tracking()
    }

    /// Disable event tracking in the BPF program
    pub fn disable(&mut self) -> anyhow::Result<()> {
        self.bpf.disable_tracking()
    }

    /// Detach all attached probes. Called explicitly at teardown because the
    /// process may exit without ever dropping the tracker, in which case the
    /// kernel would close each link fd serially at exit.
    pub fn detach(&mut self) {
        self.bpf.detach_probes();
    }

    /// Number of events the kernel dropped because the ring buffer was full.
    /// A non-zero value means the resulting trace is incomplete.
    pub fn dropped_events_count(&self) -> anyhow::Result<u64> {
        self.bpf.dropped_events_count()
    }
}
