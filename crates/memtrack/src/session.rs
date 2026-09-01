use crate::ebpf::poller::RingBufferPoller;
use crate::perf_mappings::PerfMappingPoller;
use crate::prelude::*;
use runner_shared::artifacts::MemtrackEvent;
use std::process::{Child, ExitStatus};
use std::sync::mpsc::Receiver;

/// A spawned, tracked process together with its event pipeline. The pipeline
/// stays alive as long as the session does; dropping it stops event delivery.
pub struct Session {
    child: Child,
    events: Option<Receiver<MemtrackEvent>>,

    // Drop order is part of the artifact compatibility contract. Rust drops
    // fields in declaration order: both BPF pollers must stay before the perf
    // mapping poller. Their Drop implementations disconnect, fully drain, and
    // join their poll threads before PerfMappingPoller drops and emits its
    // buffered Mapping records as the terminal stream suffix.
    _poller: RingBufferPoller,
    _stack_poller: Option<RingBufferPoller>,
    _perf_mapping_poller: Option<PerfMappingPoller>,
}

impl Session {
    pub(crate) fn new(
        child: Child,
        events: Receiver<MemtrackEvent>,
        poller: RingBufferPoller,
        stack_poller: Option<RingBufferPoller>,
        perf_mapping_poller: Option<PerfMappingPoller>,
    ) -> Self {
        Self {
            child,
            events: Some(events),
            _poller: poller,
            _stack_poller: stack_poller,
            _perf_mapping_poller: perf_mapping_poller,
        }
    }

    pub fn pid(&self) -> i32 {
        self.child.id() as i32
    }

    /// Take ownership of the event receiver. Can only be taken once.
    pub fn take_events(&mut self) -> Result<Receiver<MemtrackEvent>> {
        self.events.take().context("events already taken")
    }

    /// Wait for the tracked process to exit.
    pub fn wait(&mut self) -> Result<ExitStatus> {
        Ok(self.child.wait()?)
    }
}
