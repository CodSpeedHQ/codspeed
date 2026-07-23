use super::MemtrackBpf;
use crate::prelude::*;
use paste::paste;

impl MemtrackBpf {
    attach_tracepoint!(rss_stat);
    attach_tracepoint!(task_newtask);
    attach_tracepoint!(sched_process_exec);
    attach_tracepoint!(sched_process_exit);
    attach_tracepoint!(sys_enter_mmap);
    attach_tracepoint!(sys_exit_mmap);
    attach_tracepoint!(sys_enter_munmap);
    attach_tracepoint!(sys_enter_brk);
    attach_tracepoint!(sys_exit_brk);

    fn rmap_target_enabled(&self, target: &str) -> bool {
        !self.btf_disabled_rmap_targets.contains(&target)
    }

    pub fn attach_tracepoints(&mut self) -> Result<()> {
        self.attach_task_newtask()?;
        self.attach_sched_process_exec()?;
        self.attach_sched_process_exit()?;
        self.attach_sys_enter_mmap()?;
        self.attach_sys_exit_mmap()?;
        self.attach_sys_enter_munmap()?;
        self.attach_sys_enter_brk()?;
        self.attach_sys_exit_brk()?;
        if let Err(e) = self.attach_rss_stat() {
            warn!("Failed to attach rss_stat tracepoint, RSS collection disabled: {e:#}");
        }
        if self.track_rmap {
            macro_rules! attach_rmap_prog {
                ($name:ident) => {
                    paste! {
                        if self.rmap_target_enabled(stringify!($name)) {
                            let link = with_skel!(
                                mut self,
                                skel => skel.progs.[<fentry_ $name>].attach()
                            )
                            .context(format!(
                                "Failed to attach {} fentry",
                                stringify!([<fentry_ $name>])
                            ))?;
                            self.probes.push(link);
                        }
                    }
                };
            }
            for_each_rmap_prog!(attach_rmap_prog);
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
