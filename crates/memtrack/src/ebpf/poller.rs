use anyhow::Result;
use crossbeam_channel::{Receiver, Sender, unbounded};
use libbpf_rs::{MapCore, RingBufferBuilder};
use runner_shared::artifacts::MemtrackEvent as Event;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use super::events::parse_batch;

/// Polls a BPF ring buffer on a background thread and forwards events over a channel.
pub struct RingBufferPoller {
    shutdown: Arc<AtomicBool>,
    poll_thread: Option<JoinHandle<()>>,
}

impl RingBufferPoller {
    /// Poll `rb_map` and forward each parsed event over the returned channel.
    ///
    /// Also returns a sender for the same channel so callers can inject
    /// events drained outside the poll loop (e.g. staged partial batches).
    pub fn with_channel<M: MapCore + 'static>(
        rb_map: &M,
        poll_timeout_ms: u64,
    ) -> Result<(Self, Sender<Event>, Receiver<Event>)> {
        let (tx, rx) = unbounded::<Event>();

        let mut builder = RingBufferBuilder::new();
        let batch_tx = tx.clone();
        builder.add(rb_map, move |data| {
            parse_batch(data, |event| {
                let _ = batch_tx.send(event);
            });
            0
        })?;
        let ringbuf = builder.build()?;

        let shutdown = Arc::new(AtomicBool::new(false));
        let poll_thread = std::thread::spawn({
            let shutdown = shutdown.clone();
            let timeout = Duration::from_millis(poll_timeout_ms);
            move || {
                while !shutdown.load(Ordering::Relaxed) {
                    let _ = ringbuf.poll(timeout);

                    // Records produced while poll() was draining are consumed
                    // directly (no epoll_wait syscall) until the buffer reads
                    // empty, so bursts are drained at full speed instead of
                    // paying a wakeup round-trip per poll cycle.
                    while ringbuf.consume_raw() > 0 {}
                }

                // Events may still be sitting in the ring buffer after the last
                // poll; consume once more so they reach the channel before it closes.
                let _ = ringbuf.consume();
            }
        });

        Ok((
            Self {
                shutdown,
                poll_thread: Some(poll_thread),
            },
            tx,
            rx,
        ))
    }

    /// Stop the polling thread and wait for it to finish draining.
    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(thread) = self.poll_thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for RingBufferPoller {
    fn drop(&mut self) {
        self.shutdown();
    }
}
