use crate::ebpf::poller::RingBufferPoller;
use crate::prelude::*;
use runner_shared::artifacts::MemtrackEvent;
use std::process::{Child, ExitStatus};
use std::sync::mpsc::Receiver;

/// A spawned, tracked process together with its event pipeline. The pipeline
/// stays alive as long as the session does; dropping it stops event delivery.
pub struct Session {
    child: Child,
    events: Option<Receiver<MemtrackEvent>>,
    _poller: RingBufferPoller,
}

impl Session {
    pub(crate) fn new(
        child: Child,
        events: Receiver<MemtrackEvent>,
        poller: RingBufferPoller,
    ) -> Self {
        Self {
            child,
            events: Some(events),
            _poller: poller,
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
