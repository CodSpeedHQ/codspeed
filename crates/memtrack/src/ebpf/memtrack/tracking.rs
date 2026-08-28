use super::{MemtrackBpf, RmapSupport};
use crate::ebpf::mappings::MappingSupport;
use crate::prelude::*;
use paste::paste;

impl MemtrackBpf {
    attach_tracepoint!(rss_stat);
    attach_tracepoint!(sched_process_fork);
    attach_tracepoint!(sched_process_exec);
    attach_tracepoint!(sched_process_exit);
    attach_tracepoint!(sys_enter_mmap);
    attach_tracepoint!(sys_exit_mmap);
    attach_tracepoint!(sys_enter_munmap);
    attach_tracepoint!(sys_enter_brk);
    attach_tracepoint!(sys_exit_brk);

    pub fn attach_tracepoints(&mut self) -> Result<()> {
        self.attach_sched_process_fork()?;
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

        // Defined here rather than as a method per group because the per-program
        // attach has to close over `self`.
        macro_rules! attach_rmap_prog {
            ($name:ident) => {
                paste! {
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
            };
        }
        match self.rmap {
            RmapSupport::Unsupported => {}
            RmapSupport::Core => {
                for_each_rmap_core_prog!(attach_rmap_prog);
            }
            RmapSupport::CoreAndPud => {
                for_each_rmap_core_prog!(attach_rmap_prog);
                for_each_rmap_pud_prog!(attach_rmap_prog);
            }
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

    /// Attach the mapping recorder: the LSM hook caching resolved paths and the
    /// `perf_event_mmap` fentry emitting the address records. Only the LSM
    /// variant the running kernel supports was loaded.
    pub fn attach_mapping_recorder(&mut self) -> Result<()> {
        let link = match self.mappings {
            MappingSupport::Unsupported => return Ok(()),
            MappingSupport::Legacy => {
                with_skel!(mut self, skel => skel.progs.cache_mmap_path_legacy.attach())
            }
            MappingSupport::Kfunc => {
                with_skel!(mut self, skel => skel.progs.cache_mmap_path_kfunc.attach())
            }
        }
        .context("Failed to attach the mmap path resolver")?;
        self.probes.push(link);

        let link = with_skel!(mut self, skel => skel.progs.record_mmap.attach())
            .context("Failed to attach the mapping recorder")?;
        self.probes.push(link);
        Ok(())
    }
}
