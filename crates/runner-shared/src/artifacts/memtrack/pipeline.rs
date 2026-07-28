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

/// Encode a stream of events into a single compressed artifact stream,
/// compressing frames in parallel across a Rayon pool of `n_workers` threads.
///
/// Events are grouped into fixed-size frames; each frame is one self-contained
/// zstd frame. Frames are encoded a window at a time: a window is compressed in
/// parallel, then its frames are written in input order before the next window
/// starts, so the output matches the input order and peak memory stays bounded.
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
    loop {
        window.clear();
        window.extend(events.by_ref().take(cap));
        if window.is_empty() {
            break;
        }
        total += window.len() as u64;

        let frames: Vec<Vec<u8>> = pool.install(|| {
            window
                .par_chunks(FRAME_EVENTS)
                .map(encode_frame)
                .collect::<anyhow::Result<_>>()
        })?;

        for frame in frames {
            out.write_all(&frame)?;
        }
        wrote_any = true;
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
    fn empty_source_writes_a_valid_stream() -> anyhow::Result<()> {
        let events: Vec<MemtrackEvent> = Vec::new();

        let mut out = Vec::new();
        let total = encode_events(events, &mut out, 4)?;
        assert_eq!(total, 0);

        assert!(MemtrackArtifact::is_empty(Cursor::new(out)));

        Ok(())
    }
}
