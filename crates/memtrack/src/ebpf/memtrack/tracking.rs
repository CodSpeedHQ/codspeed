use super::MemtrackBpf;
use crate::prelude::*;
use paste::paste;

impl MemtrackBpf {
    attach_tracepoint!(sched_fork);

    pub fn attach_tracepoints(&mut self) -> Result<()> {
        self.attach_sched_fork()?;
        Ok(())
    }

    /// Attach the exec-mapping watcher (fentry/security_mmap_file). Only used in
    /// on-demand mode; the program is loaded and verified in all modes.
    pub fn attach_exec_watcher(&mut self) -> Result<()> {
        let link = with_skel!(mut self, skel => skel.progs.watch_exec_mmap.attach())
            .context("Failed to attach exec-mapping watcher")?;
        self.probes.push(link);
        Ok(())
    }
}
