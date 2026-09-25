use std::collections::VecDeque;
use std::io::{BufWriter, Write};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use super::MemtrackEvent;
use super::writer::MemtrackWriter;

/// Events per self-contained zstd frame. Larger frames compress better; smaller
/// frames cap the work (and memory) a single worker holds while encoding.
const FRAME_EVENTS: usize = 64 * 1024;
/// Frames allowed in flight per worker. At the cap the reader waits for the
/// oldest frame, bounding memory to about `cap + 1` frames.
const MAX_IN_FLIGHT_PER_WORKER: usize = 2;

type FrameResult = Receiver<anyhow::Result<Vec<u8>>>;

/// Encode a stream of events into a single compressed artifact stream,
/// compressing frames in parallel across a Rayon pool of `n_workers` threads.
///
/// Events are grouped into fixed-size frames; each frame is one self-contained
/// zstd frame. Full frames are submitted as soon as they fill, and completed
/// frames are written in input order. At most
/// `MAX_IN_FLIGHT_PER_WORKER * n_workers` frames are in flight; at the cap the
/// reader waits for the oldest frame before pulling more events.
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
    let max_in_flight = MAX_IN_FLIGHT_PER_WORKER * n_workers.max(1);

    let mut out = BufWriter::new(out);
    let mut total = 0u64;
    let mut wrote_any = false;
    let mut in_flight: VecDeque<FrameResult> = VecDeque::with_capacity(max_in_flight);

    let submit = |frame: Vec<MemtrackEvent>, in_flight: &mut VecDeque<FrameResult>| {
        let (tx, rx) = mpsc::sync_channel(1);
        pool.spawn(move || {
            let _ = tx.send(encode_frame(&frame));
        });
        in_flight.push_back(rx);
    };

    let mut frame: Vec<MemtrackEvent> = Vec::with_capacity(FRAME_EVENTS);
    for event in events {
        frame.push(event);
        if frame.len() < FRAME_EVENTS {
            continue;
        }
        total += frame.len() as u64;
        let full = std::mem::replace(&mut frame, Vec::with_capacity(FRAME_EVENTS));
        submit(full, &mut in_flight);

        while let Some(rx) = in_flight.front() {
            match rx.try_recv() {
                Ok(encoded) => {
                    out.write_all(&encoded?)?;
                    wrote_any = true;
                    in_flight.pop_front();
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    anyhow::bail!("frame encoder worker exited without a result")
                }
            }
        }

        if in_flight.len() >= max_in_flight {
            let rx = in_flight.pop_front().expect("in-flight queue is non-empty");
            out.write_all(&recv_frame(&rx)?)?;
            wrote_any = true;
        }
    }

    if !frame.is_empty() {
        total += frame.len() as u64;
        submit(frame, &mut in_flight);
    }
    for rx in in_flight.drain(..) {
        out.write_all(&recv_frame(&rx)?)?;
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

/// Block until the frame behind `rx` is encoded.
fn recv_frame(rx: &FrameResult) -> anyhow::Result<Vec<u8>> {
    rx.recv()
        .map_err(|_| anyhow::anyhow!("frame encoder worker exited without a result"))?
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
    fn preserves_order_beyond_in_flight_cap() -> anyhow::Result<()> {
        let events = malloc_events(0..(FRAME_EVENTS as u64 * 5 + 3));

        let mut out = Vec::new();
        let total = encode_events(events.clone(), &mut out, 1)?;
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
