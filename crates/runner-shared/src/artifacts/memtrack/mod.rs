use libc::pid_t;
use serde::{Deserialize, Serialize};
use std::io::{BufReader, Read, Write};

mod pipeline;
mod writer;

pub use pipeline::*;
pub use writer::*;

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
    fn manual_serialize_is_byte_identical_to_derive() {
        #[derive(serde::Serialize)]
        struct Shadow {
            pid: libc::pid_t,
            tid: libc::pid_t,
            timestamp: u64,
            addr: u64,
            #[serde(flatten)]
            kind: MemtrackEventKind,
        }

        let kinds = [
            MemtrackEventKind::Malloc { size: 7 },
            MemtrackEventKind::Free,
            MemtrackEventKind::Realloc {
                old_addr: Some(0x1000),
                size: 42,
            },
            MemtrackEventKind::Realloc {
                old_addr: None,
                size: 42,
            },
            MemtrackEventKind::Calloc { size: 9 },
            MemtrackEventKind::AlignedAlloc { size: 9 },
            MemtrackEventKind::Mmap { size: 9 },
            MemtrackEventKind::Munmap { size: 9 },
            MemtrackEventKind::Brk { size: 9 },
        ];

        for kind in kinds {
            let event = MemtrackEvent {
                pid: -7,
                tid: 42,
                timestamp: 0xDEAD,
                addr: 0xBEEF,
                kind,
            };
            let shadow = Shadow {
                pid: -7,
                tid: 42,
                timestamp: 0xDEAD,
                addr: 0xBEEF,
                kind,
            };

            assert_eq!(
                rmp_serde::to_vec(&event).unwrap(),
                rmp_serde::to_vec(&shadow).unwrap()
            );
        }
    }

    #[test]
    fn concatenated_frames_decode_in_order() -> anyhow::Result<()> {
        let events: Vec<_> = (0..2500)
            .map(|i| MemtrackEvent {
                pid: 1,
                tid: 1,
                timestamp: i,
                addr: i,
                kind: MemtrackEventKind::Malloc { size: i },
            })
            .collect();

        let mut file = Vec::new();
        for batch in events.chunks(1000) {
            let mut writer = MemtrackWriter::new(Vec::<u8>::new())?;
            for event in batch {
                writer.write_event(event)?;
            }
            let frame = writer.finish()?;
            file.extend_from_slice(&frame);
        }

        let decoded: Vec<_> = MemtrackArtifact::decode_streamed(Cursor::new(file))?.collect();
        assert_eq!(decoded, events);

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
        let buf = include_bytes!("../../../testdata/realloc.MemtrackArtifact.msgpack");
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
