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

/// A [`RingBufferPoller`] whose parsed items need a further, potentially
/// expensive step (e.g. a BPF map lookup, which is a syscall) before they are
/// forwarded on `tx`. That step runs on a dedicated resolver thread instead
/// of the poll thread, so a slow per-record resolve can't make the poll
/// thread fall behind the ring and drop records.
pub struct ThreadedRingBufferPoller {
    // Declaration order is the shutdown order: dropping `ring` first
    // disconnects its control channel and joins its poll thread, which drops
    // the internal sender the resolver reads from. Only then can the
    // resolver's `recv` loop observe disconnection, finish forwarding
    // whatever it already has, and let `resolver`'s join below return. This
    // gives callers the same "disconnect, fully drain, then join" contract
    // as a plain `RingBufferPoller`.
    ring: Option<RingBufferPoller>,
    resolver: Option<JoinHandle<()>>,
}

impl ThreadedRingBufferPoller {
    /// Poll `rb_map` with `parse` like [`RingBufferPoller::new`], but run
    /// `resolve` on a separate thread: `parse` results are forwarded over an
    /// internal channel, and `resolve` turns each one into the value sent on
    /// `tx`.
    pub fn new<M, T, U, F, R>(
        rb_map: &M,
        parse: F,
        resolve: R,
        tx: Sender<U>,
        poll_interval_ms: u64,
    ) -> Result<Self>
    where
        M: MapCore,
        T: Send + 'static,
        U: Send + 'static,
        F: Fn(&[u8]) -> Option<T> + Send + 'static,
        R: Fn(T) -> U + Send + 'static,
    {
        let (parsed_tx, parsed_rx) = mpsc::channel::<T>();
        let ring = RingBufferPoller::new(rb_map, parse, parsed_tx, poll_interval_ms)?;
        let resolver = std::thread::spawn(move || {
            for item in parsed_rx {
                let _ = tx.send(resolve(item));
            }
        });

        Ok(Self {
            ring: Some(ring),
            resolver: Some(resolver),
        })
    }
}

impl Drop for ThreadedRingBufferPoller {
    fn drop(&mut self) {
        drop(self.ring.take());
        if let Some(resolver) = self.resolver.take() {
            let _ = resolver.join();
        }
    }
}
