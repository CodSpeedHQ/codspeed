use serde::Serialize;
use std::io::{BufWriter, Write};

use super::MemtrackEvent;

/// Streaming writer for memtrack events, serializing into a zstd-compressed sink.
pub struct MemtrackWriter<B: Write> {
    serializer: rmp_serde::Serializer<ByteCounter<B>>,
}

/// Counts the uncompressed msgpack bytes handed to the compressor.
pub struct ByteCounter<W> {
    inner: W,
    bytes: u64,
}

impl<W: Write> Write for ByteCounter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.bytes += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write> MemtrackWriter<BufWriter<zstd::Encoder<'static, W>>> {
    pub fn new(writer: W) -> anyhow::Result<Self> {
        // We're dealing with a lot of events, so we want to compress as much as possible
        // while not taking too much time to compress.
        const COMPRESSION_LEVEL: i32 = -5;
        const BUFFER_SIZE: usize = 256 * 1024 /* 256 KB */;

        let encoder = zstd::Encoder::new(writer, COMPRESSION_LEVEL)?;
        let writer = BufWriter::with_capacity(BUFFER_SIZE, encoder);
        Ok(Self {
            serializer: rmp_serde::Serializer::new(ByteCounter {
                inner: writer,
                bytes: 0,
            }),
        })
    }

    /// Finish writing, flush the compression stream, and return the sink
    pub fn finish(self) -> anyhow::Result<W> {
        let buffered = self.serializer.into_inner().inner;
        let encoder = buffered.into_inner().map_err(|e| e.into_error())?;
        let mut writer = encoder.finish()?;
        writer.flush()?;
        Ok(writer)
    }
}

impl<B: Write> MemtrackWriter<B> {
    /// Write a single event to the stream
    pub fn write_event(&mut self, event: &MemtrackEvent) -> anyhow::Result<()> {
        event.serialize(&mut self.serializer)?;
        Ok(())
    }

    /// Uncompressed bytes serialized so far.
    pub fn uncompressed_bytes(&self) -> u64 {
        self.serializer.get_ref().bytes
    }
}
