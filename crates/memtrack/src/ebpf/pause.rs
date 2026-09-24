use crate::prelude::*;
use libbpf_rs::{MapCore, MapFlags, MapHandle};

/// Processes stopped by BPF, either for ring pressure or for an attach request.
///
/// A single SIGCONT resumes a process no matter how many times it was stopped,
/// so a process stopped for both reasons resumes only once both are released.
/// Each side deletes its own entry before checking the other's, so two
/// concurrent releases cannot both skip the resume.
pub(crate) struct StoppedProcesses {
    pressure_stopped: MapHandle,
    attach_stopped: MapHandle,
}

impl StoppedProcesses {
    pub(crate) fn new(pressure_stopped: MapHandle, attach_stopped: MapHandle) -> Self {
        Self {
            pressure_stopped,
            attach_stopped,
        }
    }

    /// Attach worker is done with pid.
    pub(crate) fn release_attach(&self, pid: u32) -> Result<()> {
        Self::release(pid, &self.attach_stopped, &self.pressure_stopped)
    }

    /// Resume every pressure-stopped producer; call once a ring is flushed.
    pub(crate) fn release_pressure(&self) -> Result<()> {
        // Deleting while iterating restarts hash iteration, so snapshot the keys first.
        let keys: Vec<Vec<u8>> = self.pressure_stopped.keys().collect();
        if !keys.is_empty() {
            debug!("Resuming {} pressure-stopped producers", keys.len());
        }
        for key in keys {
            let pid = u32::from_le_bytes(
                key.as_slice()
                    .try_into()
                    .context("Invalid pressure_stopped key size")?,
            );
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
