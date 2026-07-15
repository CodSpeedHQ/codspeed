use std::io::{BufWriter, Write};

use rayon::prelude::*;

use super::MemtrackEvent;
use super::writer::MemtrackWriter;

/// Events per self-contained zstd frame. Larger frames compress better; smaller
/// frames cap the work (and memory) a single worker holds while encoding.
const FRAME_EVENTS: usize = 64 * 1024;
/// Frames compressed in parallel per window. A window is encoded across the
/// worker pool and then written before the next one starts, so this bounds peak
/// memory to roughly `FRAME_EVENTS * WINDOW_FRAMES` events regardless of how long
/// the source runs.
const WINDOW_FRAMES: usize = 16;
/// The source may deliver events out of timestamp order by a bounded amount
/// (per-CPU staging batches flush independently). Events younger than this
/// relative to the newest event seen are held back and merged into the next
/// window, so the output stays timestamp-ordered.
const REORDER_HOLDBACK_NS: u64 = 100_000_000;

/// Encode a stream of events into a single compressed artifact stream,
/// compressing frames in parallel across a Rayon pool of `n_workers` threads.
///
/// Events are grouped into fixed-size frames; each frame is one self-contained
/// zstd frame. Frames are encoded a window at a time: a window is sorted by
/// timestamp and compressed in parallel, then its frames are written before
/// the next window starts, so the output is timestamp-ordered (up to
/// [`REORDER_HOLDBACK_NS`] of source disorder) and peak memory stays bounded.
///
/// Blocks the calling thread until `events` is exhausted. Returns the total
/// number of events written.
pub fn encode_events<S, W>(events: S, out: W, n_workers: usize) -> anyhow::Result<u64>
where
    S: IntoIterator<Item = MemtrackEvent>,
    W: Write,
{
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(n_workers.max(1))
        .build()?;

    let mut out = BufWriter::new(out);
    let mut total = 0u64;
    let mut wrote_any = false;

    let cap = FRAME_EVENTS * WINDOW_FRAMES;
    let mut events = events.into_iter();
    let mut window: Vec<MemtrackEvent> = Vec::with_capacity(cap);
    let mut exhausted = false;
    while !(exhausted && window.is_empty()) {
        let want = cap - window.len();
        let len_before = window.len();
        window.extend(events.by_ref().take(want));
        exhausted = window.len() - len_before < want;

        window.sort_unstable_by_key(|e| e.timestamp);

        // Hold back events too close to the newest one seen: a still-arriving
        // event may sort before them. Once the source is exhausted nothing
        // newer can arrive, so everything is flushed.
        let cut = if exhausted {
            window.len()
        } else {
            let newest = window.last().map(|e| e.timestamp).unwrap_or(0);
            let watermark = newest.saturating_sub(REORDER_HOLDBACK_NS);
            let cut = window.partition_point(|e| e.timestamp <= watermark);
            // The whole window can be younger than the holdback; emit half
            // anyway so the loop always makes progress.
            if cut == 0 { window.len() / 2 } else { cut }
        };
        total += cut as u64;

        let frames: Vec<Vec<u8>> = pool.install(|| {
            window[..cut]
                .par_chunks(FRAME_EVENTS)
                .map(encode_frame)
                .collect::<anyhow::Result<_>>()
        })?;

        if !frames.is_empty() {
            wrote_any = true;
        }
        for frame in frames {
            out.write_all(&frame)?;
        }
        window.drain(..cut);
    }

    // Always emit at least one (possibly empty) frame so the artifact stream is
    // valid and decodable even when no events were recorded.
    if !wrote_any {
        out.write_all(&encode_frame(&[])?)?;
    }

    out.flush()?;
    Ok(total)
}

/// Encode one batch as a single self-contained zstd frame.
fn encode_frame(batch: &[MemtrackEvent]) -> anyhow::Result<Vec<u8>> {
    let mut writer = MemtrackWriter::new(Vec::new())?;
    for event in batch {
        writer.write_event(event)?;
    }
    writer.finish()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::super::{MemtrackArtifact, MemtrackEventKind};
    use super::*;

    fn malloc_events(range: std::ops::Range<u64>) -> Vec<MemtrackEvent> {
        range
            .map(|i| MemtrackEvent {
                pid: 1,
                tid: 1,
                timestamp: i,
                addr: i,
                kind: MemtrackEventKind::Malloc { size: i },
            })
            .collect()
    }

    #[test]
    fn preserves_order_across_parallel_frames() -> anyhow::Result<()> {
        // More events than fit in one frame, so ordering has to hold across the
        // frames the worker pool compresses in parallel.
        let events = malloc_events(0..(FRAME_EVENTS as u64 * 3 + 7));

        let mut out = Vec::new();
        let total = encode_events(events.clone(), &mut out, 4)?;
        assert_eq!(total, events.len() as u64);

        let decoded: Vec<_> = MemtrackArtifact::decode_streamed(Cursor::new(out))?.collect();
        assert_eq!(decoded, events);

        Ok(())
    }

    #[test]
    fn preserves_order_across_window_boundary() -> anyhow::Result<()> {
        let events = malloc_events(0..(FRAME_EVENTS * WINDOW_FRAMES + 1) as u64);

        let mut out = Vec::new();
        let total = encode_events(events.clone(), &mut out, 4)?;
        assert_eq!(total, events.len() as u64);

        let decoded: Vec<_> = MemtrackArtifact::decode_streamed(Cursor::new(out))?.collect();
        assert_eq!(decoded, events);

        Ok(())
    }

    #[test]
    fn sorts_events_with_bounded_disorder() -> anyhow::Result<()> {
        // Interleave two "CPU streams" whose batches arrive offset from each
        // other, like per-CPU staging flushes: timestamps are globally
        // shuffled within a small bound but far apart across windows.
        let mut events = malloc_events(0..(FRAME_EVENTS as u64 * 5));
        for pair in events.chunks_exact_mut(2) {
            pair.swap(0, 1);
        }

        let mut out = Vec::new();
        let total = encode_events(events.clone(), &mut out, 4)?;
        assert_eq!(total, events.len() as u64);

        let decoded: Vec<_> = MemtrackArtifact::decode_streamed(Cursor::new(out))?.collect();
        events.sort_unstable_by_key(|e| e.timestamp);
        assert_eq!(decoded, events);

        Ok(())
    }

    #[test]
    fn empty_source_writes_a_valid_stream() -> anyhow::Result<()> {
        let events: Vec<MemtrackEvent> = Vec::new();

        let mut out = Vec::new();
        let total = encode_events(events, &mut out, 4)?;
        assert_eq!(total, 0);

        assert!(MemtrackArtifact::is_empty(Cursor::new(out)));

        Ok(())
    }
}
