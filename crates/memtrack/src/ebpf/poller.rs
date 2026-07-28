use anyhow::{Context, Result};
use libbpf_rs::{MapCore, RingBufferBuilder};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

/// Polls a BPF ring buffer in a background thread, parsing raw entries with a
/// user-supplied closure and forwarding them to an mpsc channel.
///
/// The poll thread runs until the poller is dropped, doing a final full
/// `consume()` on shutdown so no buffered entries are lost.
pub struct RingBufferPoller {
    ctl: Option<Sender<Sender<()>>>,
    poll_thread: Option<JoinHandle<()>>,
}

impl RingBufferPoller {
    pub fn new<M, T, F>(rb_map: &M, parse: F, tx: Sender<T>, poll_interval_ms: u64) -> Result<Self>
    where
        M: MapCore,
        T: Send + 'static,
        F: Fn(&[u8]) -> Option<T> + Send + 'static,
    {
        let mut builder = RingBufferBuilder::new();
        builder.add(rb_map, move |data| {
            if let Some(item) = parse(data) {
                let _ = tx.send(item);
            }
            0
        })?;
        let ringbuf = builder.build()?;

        // The control channel doubles as the poll pacing: a received message is
        // a drain request (acked after a full consume), a timeout is a regular
        // poll tick, and disconnection is the shutdown signal.
        let (ctl, ctl_rx) = mpsc::channel::<Sender<()>>();
        let poll_thread = std::thread::spawn(move || {
            loop {
                match ctl_rx.recv_timeout(Duration::from_millis(poll_interval_ms)) {
                    Ok(ack) => {
                        let _ = ringbuf.consume();
                        let _ = ack.send(());
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        let _ = ringbuf.poll(Duration::ZERO);
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        let _ = ringbuf.consume();
                        break;
                    }
                }
            }
        });

        Ok(Self {
            ctl: Some(ctl),
            poll_thread: Some(poll_thread),
        })
    }

    /// Block until a full `consume()` of the ring buffer completes. When every
    /// producer is stopped, all pending entries are in the channel afterwards.
    pub fn drain(&self) -> Result<()> {
        let (ack_tx, ack_rx) = mpsc::channel();
        let ctl = self.ctl.as_ref().context("poller already shut down")?;
        ctl.send(ack_tx).context("poll thread is gone")?;
        ack_rx.recv().context("poll thread died during drain")?;
        Ok(())
    }
}

impl Drop for RingBufferPoller {
    fn drop(&mut self) {
        drop(self.ctl.take());
        if let Some(thread) = self.poll_thread.take() {
            let _ = thread.join();
        }
    }
}
