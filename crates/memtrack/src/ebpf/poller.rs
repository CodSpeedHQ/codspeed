use crate::ebpf::stats::{self, RingSampler};
use anyhow::{Context, Result};
use libbpf_rs::{AsRawLibbpf, MapCore, RingBuffer, RingBufferBuilder, libbpf_sys};
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

/// Ring-buffer poll interval shared by every poller.
pub(crate) const POLL_INTERVAL_MS: u64 = 1;

/// Called with the ring's map name each time the poller finds the ring empty.
pub(crate) type OnDrained = Box<dyn Fn(&str) + Send>;

/// Items buffered before a channel send. `std::sync::mpsc` allocates a block
/// every 31 messages, so sending one item at a time makes that allocation
/// dominate the pipeline; batching amortizes it over a whole batch.
const BATCH_ITEMS: usize = 1024;

/// The lock is released before the send so a slow consumer never blocks the
/// ring-buffer callback.
fn flush_batch<T>(batch: &Mutex<Vec<T>>, tx: &Sender<Vec<T>>) {
    let mut buf = batch.lock();
    if buf.is_empty() {
        return;
    }
    let items = std::mem::replace(&mut *buf, Vec::with_capacity(BATCH_ITEMS));
    drop(buf);
    let _ = tx.send(items);
}

/// `consume()` also stops at a record a producer is still writing, so retry
/// until everything reserved before the call has been consumed. Bounding by a
/// producer-position snapshot, not an empty ring, keeps producers that are
/// never stopped from starving the drain.
fn consume_all(ringbuf: &RingBuffer, ring: *mut libbpf_sys::ring) {
    let target = unsafe { libbpf_sys::ring__producer_pos(ring) };
    loop {
        let _ = ringbuf.consume();
        if unsafe { libbpf_sys::ring__consumer_pos(ring) } >= target {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn poll_iteration<T>(
    control: std::result::Result<Sender<()>, RecvTimeoutError>,
    consume: impl FnOnce(),
    poll: impl FnOnce(),
    batch: &Mutex<Vec<T>>,
    tx: &Sender<Vec<T>>,
) -> bool {
    match control {
        Ok(ack) => {
            consume();
            // `drain` promises pending entries are in the channel before returning.
            flush_batch(batch, tx);
            let _ = ack.send(());
            true
        }
        Err(RecvTimeoutError::Timeout) => {
            poll();
            flush_batch(batch, tx);
            true
        }
        Err(RecvTimeoutError::Disconnected) => {
            consume();
            flush_batch(batch, tx);
            false
        }
    }
}

fn ring_of(ringbuf: &RingBuffer) -> *mut libbpf_sys::ring {
    // SAFETY: a built `RingBuffer` holds exactly the one ring added in `new`.
    unsafe { libbpf_sys::ring_buffer__ring(ringbuf.as_libbpf_object().as_ptr(), 0) }
}

/// Polls a BPF ring buffer in a background thread, parsing raw entries with a
/// user-supplied closure and forwarding them to an mpsc channel in batches.
///
/// The poll thread runs until the poller is dropped, doing a final full
/// `consume()` on shutdown so no buffered entries are lost.
pub struct RingBufferPoller {
    ctl: Option<Sender<Sender<()>>>,
    poll_thread: Option<JoinHandle<()>>,
}

impl RingBufferPoller {
    pub fn new<M, T, F>(
        rb_map: &M,
        parse: F,
        tx: Sender<Vec<T>>,
        poll_interval_ms: u64,
        on_drained: Option<OnDrained>,
    ) -> Result<Self>
    where
        M: MapCore,
        T: Send + 'static,
        F: Fn(&[u8]) -> Option<T> + Send + 'static,
    {
        // `Arc<Mutex<_>>` rather than `Rc<RefCell<_>>`: the built `RingBuffer` moves
        // into the poll thread, so the callback must be `Send`.
        let batch = Arc::new(Mutex::new(Vec::with_capacity(BATCH_ITEMS)));
        let cb_batch = Arc::clone(&batch);
        let cb_tx = tx.clone();

        let mut builder = RingBufferBuilder::new();
        builder.add(rb_map, move |data| {
            let Some(item) = parse(data) else {
                return 0;
            };
            let mut buf = cb_batch.lock();
            buf.push(item);
            if buf.len() < BATCH_ITEMS {
                return 0;
            }
            let items = std::mem::replace(&mut *buf, Vec::with_capacity(BATCH_ITEMS));
            drop(buf);
            let _ = cb_tx.send(items);
            0
        })?;
        let ringbuf = builder.build()?;
        let name = rb_map.name().to_string_lossy().into_owned();

        // The control channel doubles as the poll pacing: a received message is
        // a drain request (acked after a full consume), a timeout is a regular
        // poll tick, and disconnection is the shutdown signal.
        let (ctl, ctl_rx) = mpsc::channel::<Sender<()>>();
        let poll_thread = std::thread::spawn(move || {
            let mut sampler = RingSampler::new(name.clone(), ring_of(&ringbuf));
            loop {
                let control = ctl_rx.recv_timeout(Duration::from_millis(poll_interval_ms));
                let tick = sampler.as_ref().map(RingSampler::begin);
                let running = poll_iteration(
                    control,
                    || consume_all(&ringbuf, ring_of(&ringbuf)),
                    || {
                        let _ = ringbuf.poll(Duration::ZERO);
                    },
                    &batch,
                    &tx,
                );
                if let (Some(sampler), Some(tick)) = (&mut sampler, tick) {
                    sampler.end(tick);
                }
                if !running {
                    break;
                }
                if let Some(on_drained) = &on_drained
                    && unsafe { libbpf_sys::ring__avail_data_size(ring_of(&ringbuf)) } == 0
                {
                    on_drained(&name);
                }
            }
            if let Some(on_drained) = &on_drained {
                on_drained(&name);
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
    // Drop `ring` first: its poll thread drops the resolver's input sender.
    // The resolver then drains parsed items and can be joined safely.
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
        tx: Sender<Vec<U>>,
        poll_interval_ms: u64,
        on_drained: Option<OnDrained>,
    ) -> Result<Self>
    where
        M: MapCore,
        T: Send + 'static,
        U: Send + 'static,
        F: Fn(&[u8]) -> Option<T> + Send + 'static,
        R: Fn(T) -> U + Send + 'static,
    {
        let (parsed_tx, parsed_rx) = mpsc::channel::<Vec<T>>();
        let ring = RingBufferPoller::new(rb_map, parse, parsed_tx, poll_interval_ms, on_drained)?;
        let resolver = std::thread::spawn(move || {
            let record_stats = stats::enabled();
            for batch in parsed_rx {
                let t0 = record_stats.then(stats::now_ns);
                let n = batch.len();
                let resolved = batch.into_iter().map(&resolve).collect();
                if let Some(t0) = t0 {
                    stats::emit(&stats::Record::Resolve {
                        t0,
                        t1: stats::now_ns(),
                        n,
                    });
                }
                let _ = tx.send(resolved);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn partial_batch() -> Vec<u8> {
        vec![42; BATCH_ITEMS - 1]
    }

    #[test]
    fn timeout_flushes_partial_batch() {
        let expected = partial_batch();
        let batch = Mutex::new(expected.clone());
        let (tx, rx) = mpsc::channel();
        let polled = Cell::new(false);

        assert!(poll_iteration(
            Err(RecvTimeoutError::Timeout),
            || unreachable!(),
            || polled.set(true),
            &batch,
            &tx,
        ));

        assert!(polled.get());
        assert_eq!(rx.recv().unwrap(), expected);
    }

    #[test]
    fn drain_flushes_partial_batch() {
        let expected = partial_batch();
        let batch = Mutex::new(expected.clone());
        let (tx, rx) = mpsc::channel();
        let (ack_tx, ack_rx) = mpsc::channel();
        let consumed = Cell::new(false);

        assert!(poll_iteration(
            Ok(ack_tx),
            || consumed.set(true),
            || unreachable!(),
            &batch,
            &tx,
        ));

        assert!(consumed.get());
        assert_eq!(rx.recv().unwrap(), expected);
        ack_rx.recv().unwrap();
    }

    #[test]
    fn shutdown_flushes_partial_batch() {
        let expected = partial_batch();
        let batch = Mutex::new(expected.clone());
        let (tx, rx) = mpsc::channel();
        let consumed = Cell::new(false);

        assert!(!poll_iteration(
            Err(RecvTimeoutError::Disconnected),
            || consumed.set(true),
            || unreachable!(),
            &batch,
            &tx,
        ));

        assert!(consumed.get());
        assert_eq!(rx.recv().unwrap(), expected);
    }
}
