use crate::AllocatorLib;
use crate::ebpf::MemtrackBpf;
use crate::ebpf::events::AttachRequest;
use crate::ebpf::poller::RingBufferPoller;
use crate::prelude::*;
use parking_lot::Mutex;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::Duration;

use super::proc_fs::{resolve_mapping, wait_all_stopped};

const STOP_DEADLINE: Duration = Duration::from_secs(1);
const POLL_INTERVAL_MS: u64 = 10;
const RECV_TIMEOUT: Duration = Duration::from_millis(100);

/// SIGCONTs `pid` on drop, ignoring errors. Guarantees a stopped process is
/// resumed on every exit path, including panics.
struct ContGuard(i32);

impl Drop for ContGuard {
    fn drop(&mut self) {
        // SAFETY: kill with SIGCONT has no memory effects; errors (e.g. the
        // process already exited) are intentionally ignored.
        unsafe {
            libc::kill(self.0, libc::SIGCONT);
        }
    }
}

/// Background worker that stops tracked processes on their first mapping of an
/// unknown executable file, classifies it, attaches allocator probes, and
/// resumes them.
pub(crate) struct AttachWorker {
    handle: Option<JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
    fatal: Arc<Mutex<Option<String>>>,
    root_pid: Arc<AtomicI32>,
    bpf: Arc<Mutex<MemtrackBpf>>,
}

impl AttachWorker {
    pub(crate) fn start(bpf: Arc<Mutex<MemtrackBpf>>) -> Result<Self> {
        let shutdown = Arc::new(AtomicBool::new(false));
        let fatal = Arc::new(Mutex::new(None));
        let root_pid = Arc::new(AtomicI32::new(0));

        let (tx, rx) = mpsc::channel();
        let poller = bpf.lock().poll_attach_with_channel(POLL_INTERVAL_MS, tx)?;

        let worker = Worker {
            poller,
            rx,
            bpf: bpf.clone(),
            shutdown: shutdown.clone(),
            fatal: fatal.clone(),
            root_pid: root_pid.clone(),
        };

        let handle = std::thread::spawn(move || worker.run());

        Ok(Self {
            handle: Some(handle),
            shutdown,
            fatal,
            root_pid,
            bpf,
        })
    }

    /// Tell the worker which pid to SIGKILL on a fatal error.
    pub(crate) fn set_root_pid(&self, pid: i32) {
        self.root_pid.store(pid, Ordering::SeqCst);
    }

    /// Stop the worker, join it, and surface any fatal error it recorded.
    /// Fails when the attach-request ring buffer overflowed: exec mappings were
    /// missed, so allocator coverage would be incomplete.
    pub(crate) fn finish(mut self) -> Result<()> {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take()
            && let Err(panic) = handle.join()
        {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic payload".to_string());
            bail!("attach worker thread panicked: {msg}");
        }
        if let Some(err) = self.fatal.lock().take() {
            bail!("{err}");
        }

        let dropped = self.bpf.lock().attach_request_dropped_count()?;
        if dropped > 0 {
            bail!(
                "Memtrack attach-request ring buffer overflowed: {dropped} exec mappings missed, \
                 aborting since allocator coverage is incomplete."
            );
        }

        Ok(())
    }
}

struct Worker {
    poller: RingBufferPoller,
    rx: mpsc::Receiver<AttachRequest>,
    bpf: Arc<Mutex<MemtrackBpf>>,
    shutdown: Arc<AtomicBool>,
    fatal: Arc<Mutex<Option<String>>>,
    root_pid: Arc<AtomicI32>,
}

impl Worker {
    fn run(self) {
        let mut known: HashSet<(u64, u64)> = HashSet::new();

        loop {
            if self.shutdown.load(Ordering::SeqCst) {
                // Producers are gone: a synchronous drain flushes everything.
                if let Err(e) = self.poller.drain() {
                    self.record_fatal(e);
                    return;
                }
                let mut batch: Vec<AttachRequest> = self.rx.try_iter().collect();
                if batch.is_empty() {
                    break;
                }
                if let Err(e) = self.process_batch(&mut batch, &mut known) {
                    self.record_fatal(e);
                    return;
                }
                continue;
            }

            let first = match self.rx.recv_timeout(RECV_TIMEOUT) {
                Ok(req) => req,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            let mut batch: Vec<AttachRequest> =
                std::iter::once(first).chain(self.rx.try_iter()).collect();

            if let Err(e) = self.process_batch(&mut batch, &mut known) {
                self.record_fatal(e);
                return;
            }
        }
    }

    /// Stop every producing pid (fixpoint, draining until no new pid appears),
    /// then classify + attach for each unique `(dev, ino)`. `guards` resume every
    /// stopped pid exactly once when this returns, including the error path.
    fn process_batch(
        &self,
        batch: &mut Vec<AttachRequest>,
        known: &mut HashSet<(u64, u64)>,
    ) -> Result<()> {
        let mut guards: Vec<ContGuard> = Vec::new();
        let mut stopped: HashSet<u32> = HashSet::new();

        loop {
            let new_pids: HashSet<u32> = batch
                .iter()
                .map(|r| r.pid)
                .filter(|pid| !stopped.contains(pid))
                .collect();

            if new_pids.is_empty() {
                break;
            }

            for pid in new_pids {
                stopped.insert(pid);
                guards.push(ContGuard(pid as i32));
                wait_all_stopped(pid, STOP_DEADLINE)?;
            }

            // Every producer is stopped, so a synchronous drain is complete.
            self.poller.drain()?;
            batch.extend(self.rx.try_iter());
        }

        let mut seen: HashSet<(u64, u64)> = HashSet::new();
        for req in batch.iter() {
            let key = (req.dev, req.ino);
            if !seen.insert(key) || known.contains(&key) {
                continue;
            }

            if self.handle_request(req)? {
                self.bpf.lock().insert_known_inode(req.dev, req.ino)?;
                known.insert(key);
            }
        }

        Ok(())
    }

    /// Returns `Ok(true)` when the inode should be marked known (attached, or
    /// definitively not an allocator), `Ok(false)` when the mapping vanished
    /// (retry later). An attach failure is a hard error (fail-closed).
    fn handle_request(&self, req: &AttachRequest) -> Result<bool> {
        let Some(mapping) = resolve_mapping(req.pid, req.dev, req.ino) else {
            return Ok(false);
        };

        let Ok(lib) = AllocatorLib::from_path_static(&mapping.attach_path) else {
            debug!("on-demand: not an allocator: {}", mapping.display);
            return Ok(true);
        };

        let kind = lib.kind;
        let mut bpf = self.bpf.lock();
        let before = bpf.probe_count();
        bpf.attach_allocator_probes(lib.kind, &lib.path)?;
        if bpf.probe_count() == before {
            bail!(
                "{} classified at {} but zero probes attached",
                kind.name(),
                mapping.display
            );
        }

        info!(
            "on-demand attached {} probes to {}",
            kind.name(),
            mapping.display
        );
        Ok(true)
    }

    fn record_fatal(&self, err: anyhow::Error) {
        let msg = format!("{err:#}");
        error!("on-demand attach worker fatal: {msg}");
        *self.fatal.lock() = Some(msg);

        let root = self.root_pid.load(Ordering::SeqCst);
        if root > 0 {
            // SAFETY: kill with SIGKILL has no memory effects.
            unsafe {
                libc::kill(root, libc::SIGKILL);
            }
        }
    }
}
