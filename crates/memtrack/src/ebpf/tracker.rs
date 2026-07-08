use crate::ebpf::MemtrackBpf;
use crate::ebpf::attach_worker::AttachWorker;
use crate::ebpf::spawn::{resume, spawn_stopped, wrap_stopped};
use crate::prelude::*;
use crate::session::Session;
use parking_lot::Mutex;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::Arc;
use std::sync::mpsc;

pub struct Tracker {
    bpf: Arc<Mutex<MemtrackBpf>>,
    worker: Mutex<Option<AttachWorker>>,
}

impl Tracker {
    /// Create a new tracker. The exec-mapping watcher discovers and attaches
    /// allocator probes as the tracked process tree maps executable files.
    pub fn new() -> Result<Self> {
        Self::bump_memlock_rlimit()?;

        let mut bpf = MemtrackBpf::new()?;
        bpf.attach_tracepoints()?;
        bpf.attach_exec_watcher()?;

        let bpf = Arc::new(Mutex::new(bpf));
        let worker = AttachWorker::start(bpf.clone())?;

        Ok(Self {
            bpf,
            worker: Mutex::new(Some(worker)),
        })
    }

    /// Spawn `cmd` under tracking: the target is wrapped so it stops itself
    /// before exec'ing, its pid is armed while stopped, then it is resumed.
    /// The watcher observes the target's own `execve` mappings — no allocation
    /// escapes untracked.
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
        self.worker
            .lock()
            .as_ref()
            .context("tracker already finished")?
            .set_root_pid(pid);

        let (tx, rx) = mpsc::channel();
        let poller = {
            let mut bpf = self.bpf.lock();
            bpf.add_tracked_pid(pid)?;
            bpf.poll_events_with_channel(10, tx)?
        };
        resume(pid)?;

        Ok(Session::new(child, rx, poller))
    }

    /// Enable event tracking in the BPF program
    pub fn enable_tracking(&self) -> Result<()> {
        self.bpf.lock().enable_tracking()
    }

    /// Disable event tracking in the BPF program
    pub fn disable_tracking(&self) -> Result<()> {
        self.bpf.lock().disable_tracking()
    }

    /// Number of events the kernel dropped because the ring buffer was full.
    /// A non-zero value means the resulting trace is incomplete.
    pub fn dropped_events_count(&self) -> Result<u64> {
        self.bpf.lock().dropped_events_count()
    }

    /// Stop the attach worker and surface any fatal error it recorded,
    /// including missed exec mappings (incomplete allocator coverage).
    pub fn finish(&self) -> Result<()> {
        let worker = self
            .worker
            .lock()
            .take()
            .context("tracker already finished")?;
        worker.finish()
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
