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
    /// The msgpack decoder reads a few bytes per call, so the decompressed
    /// stream is buffered rather than read straight from the zstd decoder.
    #[allow(clippy::type_complexity)]
    pub fn decode_streamed<R: std::io::Read>(
        reader: R,
    ) -> anyhow::Result<MemtrackEventStream<BufReader<zstd::Decoder<'static, BufReader<R>>>>> {
        let decoder = zstd::Decoder::new(reader)?;
        Ok(MemtrackEventStream {
            deserializer: rmp_serde::Deserializer::new(BufReader::new(decoder)),
        })
    }

    /// Find the events that place modules in processes: executable mappings,
    /// and the forks and execs that inherit or drop them. Offline stack
    /// attribution needs these few events out of the whole artifact.
    ///
    /// Every frame the encoder writes is a self-contained zstd frame, so frames
    /// are decoded in parallel; events keep their artifact order. A truncated
    /// last frame cannot be split off and is streamed instead, so the events
    /// before the cut are still found.
    pub fn decode_module_events(artifact: &[u8]) -> anyhow::Result<Vec<MemtrackEvent>> {
        use rayon::prelude::*;

        let (frames, tail) = split_zstd_frames(artifact);
        let mut events = frames
            .par_iter()
            .map_init(Vec::new, |msgpack, frame| {
                module_events_in_frame(frame, msgpack)
            })
            .collect::<anyhow::Result<Vec<_>>>()?
            .concat();
        events.extend(Self::decode_streamed(tail)?.filter(|event| event.kind.is_module_event()));
        Ok(events)
    }

    pub fn is_empty<R: std::io::Read>(reader: R) -> bool {
        let Ok(mut stream) = MemtrackArtifact::decode_streamed(BufReader::new(reader)) else {
            return true;
        };
        stream.next().is_none()
    }
}

/// Split an artifact into its complete zstd frames, plus the unsplittable rest.
fn split_zstd_frames(mut artifact: &[u8]) -> (Vec<&[u8]>, &[u8]) {
    let mut frames = Vec::new();
    while let Ok(len) = zstd::zstd_safe::find_frame_compressed_size(artifact) {
        let (frame, rest) = artifact.split_at(len);
        frames.push(frame);
        artifact = rest;
    }
    (frames, artifact)
}

/// Decompress one frame into `msgpack` and return its module events. Decoding
/// from the buffer lets strings and stack payloads be read in place instead of
/// copied out of a stream first. Like [`MemtrackEventStream`], reading stops at
/// the first event that fails to decode.
fn module_events_in_frame(
    frame: &[u8],
    msgpack: &mut Vec<u8>,
) -> anyhow::Result<Vec<MemtrackEvent>> {
    msgpack.clear();
    zstd::stream::copy_decode(frame, &mut *msgpack)?;

    let mut deserializer = rmp_serde::Deserializer::from_read_ref(msgpack.as_slice());
    let mut events = Vec::new();
    while let Ok(event) = MemtrackEvent::deserialize(&mut deserializer) {
        if event.kind.is_module_event() {
            events.push(event);
        }
    }
    Ok(events)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemtrackEvent {
    pub pid: pid_t,
    pub tid: pid_t,
    pub timestamp: u64,
    pub addr: u64,
    #[serde(flatten)]
    pub kind: MemtrackEventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum MemtrackEventKind {
    Malloc {
        size: u64,
        #[serde(default, skip_serializing_if = "is_zero")]
        stack_hash: u64,
    },
    Free,
    Realloc {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        old_addr: Option<u64>,
        size: u64,
        #[serde(default, skip_serializing_if = "is_zero")]
        stack_hash: u64,
    },
    Calloc {
        size: u64,
        #[serde(default, skip_serializing_if = "is_zero")]
        stack_hash: u64,
    },
    AlignedAlloc {
        size: u64,
        #[serde(default, skip_serializing_if = "is_zero")]
        stack_hash: u64,
    },
    Fork {
        parent_pid: pid_t,
    },
    Exec,
    Exit,
    Rss {
        member: i32,
        size: u64,
    },
    Rmap {
        member: i32,
        delta: i64,
    },
    /// No longer emitted, kept so artifacts written by older memtrack versions still decode.
    #[deprecated(note = "mapping events are not emitted anymore")]
    Mmap {
        size: u64,
    },
    /// No longer emitted, kept so artifacts written by older memtrack versions still decode.
    #[deprecated(note = "mapping events are not emitted anymore")]
    Munmap {
        size: u64,
    },
    /// No longer emitted, kept so artifacts written by older memtrack versions still decode.
    #[deprecated(note = "mapping events are not emitted anymore")]
    Brk {
        size: u64,
    },
    /// One executable file mapping from a native PERF_RECORD_MMAP2 record.
    /// The common event header carries its address, process, and timestamp.
    Mapping {
        path: String,
        dev: u64,
        ino: u64,
        file_offset: u64,
        len: u64,
    },

    Stack {
        // Box keeps the MemtrackEventKind enum small across millions of events.
        #[serde(flatten)]
        record: Box<StackRecord>,
    },
}

impl MemtrackEventKind {
    /// Whether the event places modules in processes: an executable mapping,
    /// or a fork or exec that inherits or drops them.
    pub fn is_module_event(&self) -> bool {
        matches!(self, Self::Mapping { .. } | Self::Fork { .. } | Self::Exec)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StackRecord {
    pub hash: u64,
    /// User stack pointer the copy starts at.
    pub sp: u64,
    /// Registers by DWARF number for the capturing architecture; 33 entries on x86_64.
    pub regs: Vec<u64>,
    /// Raw stack bytes read upward from `sp`.
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
    /// In-kernel frame-pointer walk, innermost first; empty when unavailable.
    pub fp_chain: Vec<u64>,
    /// The copy filled its budget, so stack above it was not captured.
    pub truncated: bool,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
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
                kind: MemtrackEventKind::Malloc {
                    size: 64,
                    stack_hash: 0,
                },
            },
            MemtrackEvent {
                pid: 1,
                tid: 12,
                timestamp: 200,
                addr: 0x20,
                kind: MemtrackEventKind::Free,
            },
            MemtrackEvent {
                pid: 1,
                tid: 11,
                timestamp: 300,
                addr: 0,
                kind: MemtrackEventKind::Rss {
                    member: 1,
                    size: 40960,
                },
            },
            MemtrackEvent {
                pid: 1,
                tid: 11,
                timestamp: 400,
                addr: 0x400000,
                kind: MemtrackEventKind::Mapping {
                    path: "/usr/lib/libexample.so".into(),
                    dev: 0x0801,
                    ino: 0x1234,
                    file_offset: 0x1000,
                    len: 0x2000,
                },
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
            MemtrackEventKind::Malloc {
                size: 7,
                stack_hash: 0,
            },
            MemtrackEventKind::Malloc {
                size: 7,
                stack_hash: 0xCAFE_BABE,
            },
            MemtrackEventKind::Free,
            MemtrackEventKind::Realloc {
                old_addr: Some(0x1000),
                size: 42,
                stack_hash: 0,
            },
            MemtrackEventKind::Realloc {
                old_addr: None,
                size: 42,
                stack_hash: 0x1234,
            },
            MemtrackEventKind::Calloc {
                size: 9,
                stack_hash: 0,
            },
            MemtrackEventKind::AlignedAlloc {
                size: 9,
                stack_hash: 0,
            },
            MemtrackEventKind::Mapping {
                path: "/usr/lib/libexample.so".into(),
                dev: 0x0801,
                ino: 0x1234,
                file_offset: 0x1000,
                len: 0x2000,
            },
            MemtrackEventKind::Stack {
                record: Box::new(StackRecord {
                    hash: 0xDEAD_BEEF,
                    sp: 0x7FFF_0000,
                    regs: vec![0; 33],
                    bytes: vec![1, 2, 3, 4],
                    fp_chain: vec![0x1000, 0x2000],
                    truncated: false,
                }),
            },
        ];

        for kind in kinds {
            let event = MemtrackEvent {
                pid: -7,
                tid: 42,
                timestamp: 0xDEAD,
                addr: 0xBEEF,
                kind: kind.clone(),
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
                kind: MemtrackEventKind::Malloc {
                    size: i,
                    stack_hash: 0,
                },
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

    /// Every event kind, with stack payloads, across several frames and a
    /// last frame cut short: the parallel frame decoder must find exactly what
    /// a full streamed decode filtered to module events finds.
    #[test]
    fn module_events_match_a_filtered_full_decode() -> anyhow::Result<()> {
        let kinds = [
            MemtrackEventKind::Malloc {
                size: 64,
                stack_hash: 7,
            },
            MemtrackEventKind::Free,
            MemtrackEventKind::Realloc {
                old_addr: Some(0x10),
                size: 128,
                stack_hash: 0,
            },
            MemtrackEventKind::Stack {
                record: Box::new(StackRecord {
                    hash: 7,
                    sp: 0x7fff_0000,
                    regs: vec![1; 33],
                    bytes: vec![0xab; 4096],
                    fp_chain: vec![0x5555_0000, 0x5555_0010],
                    truncated: true,
                }),
            },
            MemtrackEventKind::Fork { parent_pid: 1 },
            MemtrackEventKind::Exec,
            MemtrackEventKind::Rss {
                member: 1,
                size: 4096,
            },
            MemtrackEventKind::Mapping {
                path: "/usr/lib/libexample.so".into(),
                dev: 0x0800_0001,
                ino: 42,
                file_offset: 0x1000,
                len: 0x2000,
            },
            MemtrackEventKind::Exit,
            MemtrackEventKind::Rmap {
                member: 1,
                delta: -70_000,
            },
        ];
        let events: Vec<_> = (0..3000u64)
            .map(|i| MemtrackEvent {
                pid: 2,
                tid: 3,
                timestamp: i,
                addr: i * 16,
                kind: kinds[i as usize % kinds.len()].clone(),
            })
            .collect();

        let mut artifact = Vec::new();
        for batch in events.chunks(1000) {
            let mut writer = MemtrackWriter::new(Vec::<u8>::new())?;
            for event in batch {
                writer.write_event(event)?;
            }
            artifact.extend_from_slice(&writer.finish()?);
        }
        let truncated = &artifact[..artifact.len() - 64];

        for artifact in [&artifact[..], truncated] {
            let expected: Vec<_> = MemtrackArtifact::decode_streamed(artifact)?
                .filter(|event| event.kind.is_module_event())
                .collect();
            assert!(!expected.is_empty());
            assert_eq!(MemtrackArtifact::decode_module_events(artifact)?, expected);
        }
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
                size: 42,
                stack_hash: 0,
            }
        ));

        Ok(())
    }

    #[test]
    #[allow(deprecated)]
    fn test_deserialize_mapping_events_compat() -> anyhow::Result<()> {
        // Artifact written by a memtrack version that still emitted mmap/munmap/brk events,
        // followed by a malloc: an unknown variant would end the stream and drop the tail.
        let buf = include_bytes!("../../../testdata/mappings.MemtrackArtifact.msgpack");
        let kinds: Vec<_> = MemtrackArtifact::decode_streamed(Cursor::new(buf))?
            .map(|event| event.kind)
            .collect();

        assert_eq!(
            kinds,
            vec![
                MemtrackEventKind::Mmap { size: 4096 },
                MemtrackEventKind::Munmap { size: 4096 },
                MemtrackEventKind::Brk { size: 8192 },
                MemtrackEventKind::Malloc {
                    size: 64,
                    stack_hash: 0,
                },
            ]
        );

        Ok(())
    }
}
