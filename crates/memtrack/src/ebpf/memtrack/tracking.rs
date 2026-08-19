use super::MemtrackBpf;
use crate::prelude::*;
use paste::paste;

impl MemtrackBpf {
    attach_tracepoint!(rss_stat);
    attach_tracepoint!(task_newtask);
    attach_tracepoint!(sched_process_exec);
    attach_tracepoint!(sched_process_exit);

    pub fn attach_tracepoints(&mut self) -> Result<()> {
        self.attach_task_newtask()?;
        self.attach_sched_process_exec()?;
        self.attach_sched_process_exit()?;
        if let Err(e) = self.attach_rss_stat() {
            warn!("Failed to attach rss_stat tracepoint, RSS collection disabled: {e:#}");
        }
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
