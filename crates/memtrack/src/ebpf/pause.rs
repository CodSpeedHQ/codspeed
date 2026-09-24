use crate::prelude::*;
use libbpf_rs::{MapCore, MapFlags, MapHandle};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Processes stopped by BPF, either for ring pressure or for an attach request.
///
/// A single SIGCONT resumes a process no matter how many times it was stopped,
/// so a process stopped for both reasons resumes only once both are released.
/// Each side deletes its own entry before checking the other's, so two
/// concurrent releases cannot both skip the resume.
pub(crate) struct StoppedProcesses {
    pressure_stopped: MapHandle,
    attach_stopped: MapHandle,
    // Stop counts, only used for stats.
    pressure_stops: AtomicU64,
    attach_stops: AtomicU64,
}

impl StoppedProcesses {
    pub(crate) fn new(pressure_stopped: MapHandle, attach_stopped: MapHandle) -> Self {
        Self {
            pressure_stopped,
            attach_stopped,
            pressure_stops: AtomicU64::new(0),
            attach_stops: AtomicU64::new(0),
        }
    }

    /// Attach worker is done with pid.
    pub(crate) fn release_attach(&self, pid: u32) -> Result<()> {
        debug!("Releasing attach stop of pid {pid}");
        self.attach_stops.fetch_add(1, Relaxed);
        Self::release(pid, &self.attach_stopped, &self.pressure_stopped)
    }

    /// Resume every pressure-stopped producer; call once a ring is flushed.
    pub(crate) fn release_pressure(&self) -> Result<()> {
        // Deleting while iterating restarts hash iteration, so snapshot the keys first.
        let keys: Vec<Vec<u8>> = self.pressure_stopped.keys().collect();
        if keys.is_empty() {
            return Ok(());
        }
        self.pressure_stops.fetch_add(keys.len() as u64, Relaxed);
        for key in keys {
            let pid = u32::from_le_bytes(
                key.as_slice()
                    .try_into()
                    .context("Invalid pressure_stopped key size")?,
            );
            debug!("Releasing pressure stop of pid {pid}");
            Self::release(pid, &self.pressure_stopped, &self.attach_stopped)?;
        }
        Ok(())
    }

    /// Deletes `pid` from `own`, then resumes it unless `other` still holds it.
    /// A missing entry means the process already exited or was released.
    fn release(pid: u32, own: &MapHandle, other: &MapHandle) -> Result<()> {
        let key = pid.to_le_bytes();
        match own.delete(&key) {
            Ok(()) => {}
            Err(err) if err.kind() == libbpf_rs::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err).context("Failed to delete stop entry"),
        }
        if other.lookup(&key, MapFlags::ANY)?.is_some() {
            return Ok(());
        }
        crate::ebpf::spawn::resume(pid as i32)
    }
}

impl Drop for StoppedProcesses {
    fn drop(&mut self) {
        debug!(
            "Process stops: {} pressure, {} attach",
            self.pressure_stops.get_mut(),
            self.attach_stops.get_mut(),
        );
    }
}
