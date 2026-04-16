use serde::{Deserialize, Serialize};

use crate::pid_t;
use std::io::{BufReader, BufWriter, Read, Write};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemtrackArtifact {
    pub events: Vec<MemtrackEvent>,
}
impl super::ArtifactExt for MemtrackArtifact {
    fn encode_to_writer<W: Write>(&self, writer: W) -> anyhow::Result<()> {
        let mut writer = MemtrackWriter::new(writer)?;
        for event in &self.events {
            writer.write_event(event)?;
        }
        writer.finish()?;
        Ok(())
    }
}

impl MemtrackArtifact {
    pub fn decode_streamed<R: std::io::Read>(
        reader: R,
    ) -> anyhow::Result<MemtrackEventStream<zstd::Decoder<'static, std::io::BufReader<R>>>> {
        let decoder = zstd::Decoder::new(reader)?;
        Ok(MemtrackEventStream {
            deserializer: rmp_serde::Deserializer::new(decoder),
        })
    }

    pub fn is_empty<R: std::io::Read>(reader: R) -> bool {
        let Ok(mut stream) = MemtrackArtifact::decode_streamed(BufReader::new(reader)) else {
            return true;
        };
        stream.next().is_none()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemtrackEvent {
    pub pid: pid_t,
    pub tid: pid_t,
    pub timestamp: u64,
    pub addr: u64,
    #[serde(flatten)]
    pub kind: MemtrackEventKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum MemtrackEventKind {
    Malloc {
        size: u64,
    },
    Free,
    Realloc {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        old_addr: Option<u64>,
        size: u64,
    },
    Calloc {
        size: u64,
    },
    AlignedAlloc {
        size: u64,
    },
    Mmap {
        size: u64,
    },
    Munmap {
        size: u64,
    },
    Brk {
        size: u64,
    },
}

pub struct MemtrackEventStream<R: Read> {
    deserializer: rmp_serde::Deserializer<rmp_serde::decode::ReadReader<R>>,
}

impl<R: Read> Iterator for MemtrackEventStream<R> {
    type Item = MemtrackEvent;

    fn next(&mut self) -> Option<Self::Item> {
        MemtrackEvent::deserialize(&mut self.deserializer).ok()
    }
}

/// Streaming writer for memtrack events with compression
pub struct MemtrackWriter<W: Write> {
    serializer: rmp_serde::Serializer<zstd::Encoder<'static, BufWriter<W>>>,
}

impl<W: Write> MemtrackWriter<W> {
    pub fn new(writer: W) -> anyhow::Result<Self> {
        // We're dealing with a lot of events, so we want to compress as much as possible
        // while not taking too much time to compress.
        const COMPRESSION_LEVEL: i32 = 1;
        const BUFFER_SIZE: usize = 256 * 1024 /* 256 KB */;

        let writer = BufWriter::with_capacity(BUFFER_SIZE, writer);
        let encoder = zstd::Encoder::new(writer, COMPRESSION_LEVEL)?;
        Ok(Self {
            serializer: rmp_serde::Serializer::new(encoder),
        })
    }

    /// Write a single event to the stream
    pub fn write_event(&mut self, event: &MemtrackEvent) -> anyhow::Result<()> {
        event.serialize(&mut self.serializer)?;
        Ok(())
    }

    /// Finish writing and flush the compression stream
    pub fn finish(self) -> anyhow::Result<()> {
        let encoder = self.serializer.into_inner();
        let mut writer = encoder.finish()?;

        // Flush the writer to ensure all data is written to the underlying writer
        writer.flush()?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::artifacts::ArtifactExt;

    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_decode_streamed() -> anyhow::Result<()> {
        let events = vec![
            MemtrackEvent {
                pid: 1,
                tid: 11,
                timestamp: 100,
                addr: 0x10,
                kind: MemtrackEventKind::Malloc { size: 64 },
            },
            MemtrackEvent {
                pid: 1,
                tid: 12,
                timestamp: 200,
                addr: 0x20,
                kind: MemtrackEventKind::Free,
            },
        ];

        let artifact = MemtrackArtifact {
            events: events.clone(),
        };
        let mut buf = Vec::new();
        artifact.encode_to_writer(&mut buf)?;

        let stream = MemtrackArtifact::decode_streamed(Cursor::new(buf))?;
        let collected: Vec<_> = stream.collect();
        assert_eq!(collected, events);

        Ok(())
    }

    #[test]
    fn test_artifact_is_empty() -> anyhow::Result<()> {
        let artifact = MemtrackArtifact { events: vec![] };

        let mut buf = Vec::new();
        artifact.encode_to_writer(&mut buf)?;

        let reader = Cursor::new(buf);
        assert!(MemtrackArtifact::is_empty(reader));

        Ok(())
    }

    #[test]
    fn test_deserialize_realloc_compat() -> anyhow::Result<()> {
        // The file contains a single serialized event using the old format without `old_addr`:
        // MemtrackEventKind::Realloc { size: 42 }
        let buf = include_bytes!("../../testdata/realloc.MemtrackArtifact.msgpack");
        assert_eq!(
            MemtrackArtifact::decode_streamed(Cursor::new(buf))?.count(),
            1
        );

        let event = MemtrackArtifact::decode_streamed(Cursor::new(buf))?
            .next()
            .unwrap();
        assert!(matches!(
            event.kind,
            MemtrackEventKind::Realloc {
                old_addr: None,
                size: 42
            }
        ));

        Ok(())
    }
}
