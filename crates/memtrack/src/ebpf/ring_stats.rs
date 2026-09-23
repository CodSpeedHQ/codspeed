//! Ring throughput from libbpf's mmapped producer/consumer positions, enabled
//! with `CODSPEED_MEMTRACK_RING_STATS=1`.
//!
//! The producer position counts every reserved byte, including the 8-byte
//! record headers and records the BPF side later discarded, so "wrote" is ring
//! pressure rather than artifact bytes.

use crate::prelude::*;
use libbpf_rs::libbpf_sys;
use std::time::{Duration, Instant};

const REPORT_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) struct RingStats {
    name: String,
    ring: *const libbpf_sys::ring,
    run: Window,
    window: Window,
}

/// Taken right before a poll iteration: the backlog then is everything the
/// producers wrote since the previous iteration.
pub(crate) struct Tick {
    started: Instant,
    backlog: u64,
}

struct Window {
    started: Instant,
    produced: u64,
    consumed: u64,
    busy: Duration,
    max_tick: Duration,
    peak_backlog: u64,
}

impl RingStats {
    pub(crate) fn enabled() -> bool {
        std::env::var("CODSPEED_MEMTRACK_RING_STATS").is_ok_and(|v| v == "1")
    }

    /// `ring` must stay valid for as long as the stats are used.
    pub(crate) fn new(name: String, ring: *const libbpf_sys::ring) -> Self {
        Self {
            name,
            ring,
            run: Window::start(ring),
            window: Window::start(ring),
        }
    }

    pub(crate) fn begin(&self) -> Tick {
        Tick {
            started: Instant::now(),
            // SAFETY: `ring` is valid per `new`'s contract.
            backlog: unsafe { libbpf_sys::ring__avail_data_size(self.ring) } as u64,
        }
    }

    pub(crate) fn end(&mut self, tick: Tick) {
        let busy = tick.started.elapsed();
        self.run.record(tick.backlog, busy);
        self.window.record(tick.backlog, busy);
        if self.window.started.elapsed() < REPORT_INTERVAL {
            return;
        }
        debug!(
            "{} ring (1s): {}",
            self.name,
            self.window.summary(self.ring)
        );
        self.window = Window::start(self.ring);
    }

    pub(crate) fn report_run(&self) {
        info!("{} ring (run): {}", self.name, self.run.summary(self.ring));
    }
}

impl Window {
    fn start(ring: *const libbpf_sys::ring) -> Self {
        let (produced, consumed) = positions(ring);
        Self {
            started: Instant::now(),
            produced,
            consumed,
            busy: Duration::ZERO,
            max_tick: Duration::ZERO,
            peak_backlog: 0,
        }
    }

    fn record(&mut self, backlog: u64, busy: Duration) {
        self.busy += busy;
        self.max_tick = self.max_tick.max(busy);
        self.peak_backlog = self.peak_backlog.max(backlog);
    }

    /// `drain` is the consume rate while the poll thread is busy, i.e. the
    /// read speed the thread can sustain; `read` is averaged over wall time.
    fn summary(&self, ring: *const libbpf_sys::ring) -> String {
        let (produced, consumed) = positions(ring);
        let wall = self.started.elapsed();
        let consumed = consumed - self.consumed;
        // SAFETY: `ring` is valid per `RingStats::new`'s contract.
        let size = unsafe { libbpf_sys::ring__size(ring) } as u64;
        format!(
            "wrote {:.1} MB/s, read {:.1} MB/s, drain {:.1} MB/s, peak backlog {:.1} MiB ({:.1}%), busy {:.1}% (max tick {:.2} ms)",
            mb_per_s(produced - self.produced, wall),
            mb_per_s(consumed, wall),
            mb_per_s(consumed, self.busy),
            self.peak_backlog as f64 / (1024.0 * 1024.0),
            self.peak_backlog as f64 * 100.0 / size as f64,
            self.busy.as_secs_f64() * 100.0 / wall.as_secs_f64(),
            self.max_tick.as_secs_f64() * 1e3,
        )
    }
}

fn positions(ring: *const libbpf_sys::ring) -> (u64, u64) {
    // SAFETY: `ring` is valid per `RingStats::new`'s contract.
    unsafe {
        (
            libbpf_sys::ring__producer_pos(ring),
            libbpf_sys::ring__consumer_pos(ring),
        )
    }
}

fn mb_per_s(bytes: u64, over: Duration) -> f64 {
    bytes as f64 / over.as_secs_f64() / 1e6
}
