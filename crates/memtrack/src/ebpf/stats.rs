//! Pipeline samples as JSON lines, enabled with `CODSPEED_MEMTRACK_STATS=<path>`.
//!
//! Only raw counters are recorded; rates and fill levels are derived offline
//! by `scripts/plot_stats.py`. Every `t*` is CLOCK_MONOTONIC ns, the clock of
//! `bpf_ktime_get_ns()` and of the artifact's event timestamps.

use crate::prelude::*;
use libbpf_rs::libbpf_sys;
use parking_lot::Mutex;
use serde::Serialize;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::OnceLock;

/// Emptied on the first write error, so a full disk stops sampling instead of
/// logging on every tick.
static SINK: OnceLock<Mutex<Option<BufWriter<File>>>> = OnceLock::new();

#[derive(Serialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub(crate) enum Record<'a> {
    RingOpen {
        t: u64,
        ring: &'a str,
        size: u64,
    },
    Ring {
        ring: &'a str,
        t0: u64,
        prod0: u64,
        cons0: u64,
        t1: u64,
        prod1: u64,
        cons1: u64,
    },
    Pressure {
        ring: &'a str,
        t: u64,
        pids: &'a [(u32, u64)],
    },
}

pub fn init_from_env() -> Result<()> {
    let Some(path) = std::env::var_os("CODSPEED_MEMTRACK_STATS").map(PathBuf::from) else {
        return Ok(());
    };
    let file = File::create(&path)
        .with_context(|| format!("Failed to create memtrack stats file {}", path.display()))?;
    SINK.set(Mutex::new(Some(BufWriter::new(file))))
        .map_err(|_| anyhow!("memtrack stats already initialized"))?;
    info!("Writing memtrack stats to {}", path.display());
    Ok(())
}

pub fn finish() -> Result<()> {
    let Some(mut out) = SINK.get().and_then(|sink| sink.lock().take()) else {
        return Ok(());
    };
    out.flush().context("Failed to flush memtrack stats")
}

pub(crate) fn enabled() -> bool {
    SINK.get().is_some()
}

pub(crate) fn emit(record: &Record) {
    let Some(sink) = SINK.get() else {
        return;
    };
    let mut sink = sink.lock();
    let Some(out) = sink.as_mut() else {
        return;
    };
    let written = serde_json::to_writer(&mut *out, record)
        .map_err(std::io::Error::from)
        .and_then(|()| out.write_all(b"\n"));
    if let Err(error) = written {
        error!("Stopping memtrack stats after a write error: {error}");
        *sink = None;
    }
}

pub(crate) fn now_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid out-pointer.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Positions of one ring around each poll tick, from libbpf's mmapped
/// producer/consumer pages. The producer position counts every reserved byte,
/// including 8-byte record headers and records BPF later discarded, so it is
/// ring pressure rather than artifact bytes.
pub(crate) struct RingSampler {
    name: String,
    ring: *const libbpf_sys::ring,
    last: (u64, u64),
}

pub(crate) struct Tick {
    t: u64,
    prod: u64,
    cons: u64,
}

impl RingSampler {
    /// `None` unless stats are enabled. `ring` must outlive the sampler.
    pub(crate) fn new(name: String, ring: *const libbpf_sys::ring) -> Option<Self> {
        if !enabled() {
            return None;
        }
        // SAFETY: `ring` is valid per this function's contract.
        let size = unsafe { libbpf_sys::ring__size(ring) } as u64;
        emit(&Record::RingOpen {
            t: now_ns(),
            ring: &name,
            size,
        });
        let mut sampler = Self {
            name,
            ring,
            last: (0, 0),
        };
        sampler.last = sampler.positions();
        Some(sampler)
    }

    pub(crate) fn begin(&self) -> Tick {
        let t = now_ns();
        let (prod, cons) = self.positions();
        Tick { t, prod, cons }
    }

    /// Positions only grow, so equal end positions mean nothing was written or
    /// read since the last emitted tick.
    pub(crate) fn end(&mut self, tick: Tick) {
        let (prod1, cons1) = self.positions();
        if (prod1, cons1) == self.last {
            return;
        }
        self.last = (prod1, cons1);
        emit(&Record::Ring {
            ring: &self.name,
            t0: tick.t,
            prod0: tick.prod,
            cons0: tick.cons,
            t1: now_ns(),
            prod1,
            cons1,
        });
    }

    fn positions(&self) -> (u64, u64) {
        // SAFETY: `ring` is valid per `new`'s contract.
        unsafe {
            (
                libbpf_sys::ring__producer_pos(self.ring),
                libbpf_sys::ring__consumer_pos(self.ring),
            )
        }
    }
}
