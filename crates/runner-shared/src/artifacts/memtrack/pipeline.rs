use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{BufWriter, Write};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use serde::Serialize;

use super::MemtrackEvent;
use super::writer::COMPRESSION_LEVEL;

/// Events per self-contained zstd frame. Larger frames compress better; smaller
/// frames cap the work (and memory) a single worker holds while encoding.
const FRAME_EVENTS: usize = 64 * 1024;
/// Frames allowed in flight per worker. At the cap the reader waits for the
/// oldest frame, bounding memory to about `cap + 1` frames.
const MAX_IN_FLIGHT_PER_WORKER: usize = 2;

/// An encoded frame, plus the event buffer it was built from so the reader can
/// reuse it for the next frame instead of allocating a new one.
type EncodedFrame = (anyhow::Result<Vec<u8>>, Vec<MemtrackEvent>);

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
    let mut in_flight: VecDeque<Receiver<EncodedFrame>> = VecDeque::with_capacity(max_in_flight);
    // Event buffers handed back by workers, reused for the next frames.
    let mut spare: Vec<Vec<MemtrackEvent>> = Vec::new();

    let submit = |frame: Vec<MemtrackEvent>| {
        let (tx, rx) = mpsc::sync_channel(1);
        pool.spawn(move || {
            let encoded = encode_frame(&frame);
            let _ = tx.send((encoded, frame));
        });
        rx
    };
    let mut collect = |(encoded, mut frame): EncodedFrame, spare: &mut Vec<Vec<MemtrackEvent>>| {
        out.write_all(&encoded?)?;
        wrote_any = true;
        frame.clear();
        spare.push(frame);
        anyhow::Ok(())
    };

    let mut frame: Vec<MemtrackEvent> = Vec::with_capacity(FRAME_EVENTS);
    for event in events {
        frame.push(event);
        if frame.len() < FRAME_EVENTS {
            continue;
        }
        total += frame.len() as u64;
        let next = spare
            .pop()
            .unwrap_or_else(|| Vec::with_capacity(FRAME_EVENTS));
        in_flight.push_back(submit(std::mem::replace(&mut frame, next)));

        // Write finished frames in input order. Wait for the oldest frame only
        // while the queue is at the cap.
        while let Some(rx) = in_flight.front() {
            let at_cap = in_flight.len() >= max_in_flight;
            let result = match rx.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) if !at_cap => break,
                // Blocks at the cap; fails at once if the worker is gone.
                Err(_) => recv_frame(rx)?,
            };
            in_flight.pop_front();
            collect(result, &mut spare)?;
        }
    }

    if !frame.is_empty() {
        total += frame.len() as u64;
        in_flight.push_back(submit(frame));
    }
    for rx in in_flight.drain(..) {
        collect(recv_frame(&rx)?, &mut spare)?;
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
fn recv_frame(rx: &Receiver<EncodedFrame>) -> anyhow::Result<EncodedFrame> {
    rx.recv()
        .map_err(|_| anyhow::anyhow!("frame encoder worker exited without a result"))
}

/// Upper estimate of the msgpack size of one event, used to size the frame
/// buffer up front. Growing it by doubling instead would leave each worker
/// pinning about twice a frame's msgpack size.
const MSGPACK_BYTES_PER_EVENT: usize = 80;

/// Per-thread scratch state reused across frames: the msgpack buffer and the
/// zstd compression context.
///
/// Each worker keeps its buffer (about one frame's msgpack, ~5 MiB) until the
/// pool exits, even when idle. Peak usage needs it anyway, since every busy
/// worker holds one, and allocating it per frame measured about 9% slower with
/// a single worker.
struct FrameEncoder {
    msgpack: Vec<u8>,
    compressor: zstd::bulk::Compressor<'static>,
}

thread_local! {
    static FRAME_ENCODER: RefCell<Option<FrameEncoder>> = const { RefCell::new(None) };
}

/// Encode one batch as a single self-contained zstd frame.
///
/// The batch is serialized with `rmp_serde` into a reused buffer, then
/// compressed in one shot with a reused zstd context. The decoded bytes are the
/// same msgpack stream `MemtrackWriter` produces.
fn encode_frame(batch: &[MemtrackEvent]) -> anyhow::Result<Vec<u8>> {
    FRAME_ENCODER.with(|cell| {
        let mut slot = cell.borrow_mut();
        let enc = match slot.as_mut() {
            Some(enc) => enc,
            None => slot.insert(FrameEncoder {
                msgpack: Vec::new(),
                compressor: zstd::bulk::Compressor::new(COMPRESSION_LEVEL)?,
            }),
        };

        enc.msgpack.clear();
        enc.msgpack
            .reserve_exact(batch.len() * MSGPACK_BYTES_PER_EVENT);
        let mut serializer = rmp_serde::Serializer::new(&mut enc.msgpack);
        for event in batch {
            event.serialize(&mut serializer)?;
        }

        let mut compressed = enc.compressor.compress(&enc.msgpack)?;
        // Trim the worst-case compression bound so in-flight frames only hold
        // their actual size.
        compressed.shrink_to_fit();
        Ok(compressed)
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::super::{MemtrackArtifact, MemtrackEventKind, MemtrackWriter};
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
    fn frame_payload_matches_memtrack_writer() -> anyhow::Result<()> {
        let events = malloc_events(0..10_000);

        let mut reference = MemtrackWriter::new(Vec::new())?;
        for event in &events {
            reference.write_event(event)?;
        }
        let reference = zstd::decode_all(Cursor::new(reference.finish()?))?;

        // Encode twice to also exercise the reused per-thread buffers.
        for _ in 0..2 {
            let frame = zstd::decode_all(Cursor::new(encode_frame(&events)?))?;
            assert_eq!(frame, reference);
        }

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
