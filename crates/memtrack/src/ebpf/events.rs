use crate::prelude::*;
use libbpf_rs::MapCore;
use runner_shared::artifacts::{MemtrackEvent, MemtrackEventKind, StackRecord};

// Include the bindings for event.h
pub mod bindings {
    #![allow(non_upper_case_globals)]
    #![allow(non_camel_case_types)]
    #![allow(non_snake_case)]
    #![allow(dead_code)]

    include!(concat!(env!("OUT_DIR"), "/event.rs"));
}
use bindings::*;

/// Parse an event from raw bytes into MemtrackEvent
///
/// SAFETY: The data must be a valid `bindings::event`
pub fn parse_event(data: &[u8]) -> Option<MemtrackEvent> {
    if data.len() < std::mem::size_of::<bindings::event>() {
        return None;
    }

    let event = unsafe { &*(data.as_ptr() as *const bindings::event) };

    // Common fields from header
    let pid = event.header.pid as i32;
    let tid = event.header.tid as i32;
    let timestamp = event.header.timestamp;

    // Parse event data based on type
    // SAFETY: The fields must be properly initialized in eBPF
    let (addr, kind) = unsafe {
        match event.header.event_type as u32 {
            EVENT_TYPE_MALLOC => (
                event.data.alloc.addr,
                MemtrackEventKind::Malloc {
                    size: event.data.alloc.size,
                    stack_hash: event.data.alloc.stack_hash,
                },
            ),
            EVENT_TYPE_FREE => (
                event.data.free.addr,
                MemtrackEventKind::Free {
                    stack_hash: event.data.free.stack_hash,
                },
            ),
            EVENT_TYPE_CALLOC => (
                event.data.alloc.addr,
                MemtrackEventKind::Calloc {
                    size: event.data.alloc.size,
                    stack_hash: event.data.alloc.stack_hash,
                },
            ),
            EVENT_TYPE_REALLOC => (
                event.data.realloc.new_addr,
                MemtrackEventKind::Realloc {
                    old_addr: Some(event.data.realloc.old_addr),
                    size: event.data.realloc.size,
                    stack_hash: event.data.realloc.stack_hash,
                },
            ),
            EVENT_TYPE_ALIGNED_ALLOC => (
                event.data.alloc.addr,
                MemtrackEventKind::AlignedAlloc {
                    size: event.data.alloc.size,
                    stack_hash: event.data.alloc.stack_hash,
                },
            ),
            EVENT_TYPE_MMAP => (
                event.data.mmap.addr,
                MemtrackEventKind::Mmap {
                    size: event.data.mmap.size,
                },
            ),
            EVENT_TYPE_MUNMAP => (
                event.data.mmap.addr,
                MemtrackEventKind::Munmap {
                    size: event.data.mmap.size,
                },
            ),
            EVENT_TYPE_BRK => (
                event.data.mmap.addr,
                MemtrackEventKind::Brk {
                    size: event.data.mmap.size,
                },
            ),
            EVENT_TYPE_FORK => (
                0,
                MemtrackEventKind::Fork {
                    parent_pid: event.data.fork.parent_pid as i32,
                },
            ),
            EVENT_TYPE_EXEC => (0, MemtrackEventKind::Exec),
            EVENT_TYPE_EXIT => (0, MemtrackEventKind::Exit),
            EVENT_TYPE_RSS => (
                0,
                MemtrackEventKind::Rss {
                    member: event.data.rss.member,
                    size: event.data.rss.size,
                },
            ),
            EVENT_TYPE_RMAP => (
                event.data.rmap.addr,
                MemtrackEventKind::Rmap {
                    member: event.data.rmap.member,
                    delta: event.data.rmap.delta,
                },
            ),
            unknown => {
                panic!("Unknown event type: {unknown}");
            }
        }
    };

    Some(MemtrackEvent {
        pid,
        tid,
        timestamp,
        addr,
        kind,
    })
}

/// Decode one stack record from the ring buffer, returning it alongside the
/// `bpf_get_stackid()` result its frame-pointer chain is stored under.
pub fn parse_stack(data: &[u8]) -> Option<(MemtrackEvent, i64)> {
    let header_len = std::mem::size_of::<stack_header>();
    // SAFETY: the length is checked below, and the layout is the bindgen-generated C ABI struct.
    let header: stack_header = if data.len() >= header_len {
        unsafe { std::ptr::read_unaligned(data.as_ptr().cast()) }
    } else {
        warn!(
            "malformed stack record: {} bytes, need at least {header_len}",
            data.len()
        );
        return None;
    };

    let record_len = header_len + header.copy_len as usize;
    if data.len() < record_len {
        warn!(
            "malformed stack record: {} bytes, need {record_len}",
            data.len()
        );
        return None;
    }

    let event = MemtrackEvent {
        pid: header.pid as i32,
        tid: header.tid as i32,
        timestamp: header.timestamp,
        addr: 0,
        kind: MemtrackEventKind::Stack {
            record: Box::new(StackRecord {
                hash: header.hash,
                sp: header.sp,
                regs: header.regs.reg.to_vec(),
                bytes: data[header_len..record_len].to_vec(),
                fp_chain: Vec::new(),
                truncated: header.truncated != 0,
            }),
        },
    };

    Some((event, header.stackid))
}

/// The frame-pointer walk recorded under `stackid`, innermost frame first.
/// Best effort: a missing chain costs the fallback for one stack, not the run.
pub fn fp_chain(stack_traces: &impl MapCore, stackid: i64) -> Vec<u64> {
    let Ok(key) = u32::try_from(stackid) else {
        return Vec::new();
    };

    let value = match stack_traces.lookup(&key.to_ne_bytes(), libbpf_rs::MapFlags::ANY) {
        Ok(Some(value)) => value,
        Ok(None) => return Vec::new(),
        Err(error) => {
            warn!("Failed to read frame-pointer chain for stackid {stackid}: {error}");
            return Vec::new();
        }
    };

    // The map value is a fixed-depth array zero-padded past the last frame.
    value
        .chunks_exact(8)
        .map(|word| u64::from_ne_bytes(word.try_into().expect("chunks_exact yields 8 bytes")))
        .take_while(|&address| address != 0)
        .collect()
}

/// A request from the exec-mapping watcher to attach allocator probes.
#[derive(Debug, Clone, Copy)]
pub struct AttachRequest {
    pub pid: u32,
    pub dev: u64,
    pub ino: u64,
}
impl AttachRequest {
    /// Parse an attach request from raw ring buffer bytes.
    ///
    /// SAFETY: The data must be a valid `bindings::attach_request`
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < std::mem::size_of::<bindings::attach_request>() {
            return None;
        }

        let req = unsafe { &*(data.as_ptr() as *const bindings::attach_request) };
        Some(AttachRequest {
            pid: req.pid,
            dev: req.dev,
            ino: req.ino,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event_bytes(event: &bindings::event) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(event as *const _ as *const u8, std::mem::size_of_val(event))
        }
    }

    #[test]
    fn test_parse_realloc_event() {
        // Create a mock event with realloc data
        let mut event: bindings::event = unsafe { std::mem::zeroed() };
        event.header.event_type = bindings::EVENT_TYPE_REALLOC as u8;
        event.header.timestamp = 12345678;
        event.header.pid = 1000;
        event.header.tid = 2000;
        event.data.realloc.old_addr = 0x1000;
        event.data.realloc.new_addr = 0x2000;
        event.data.realloc.size = 256;
        event.data.realloc.stack_hash = 0xbeef;

        let bytes = event_bytes(&event);

        // Parse and validate:
        let parsed = parse_event(bytes).unwrap();
        assert_eq!(parsed.pid, 1000);
        assert_eq!(parsed.tid, 2000);
        assert_eq!(parsed.timestamp, 12345678);
        assert_eq!(parsed.addr, 0x2000);

        match parsed.kind {
            MemtrackEventKind::Realloc {
                old_addr,
                size,
                stack_hash,
            } => {
                assert_eq!(old_addr, Some(0x1000));
                assert_eq!(size, 256);
                assert_eq!(stack_hash, 0xbeef);
            }
            _ => panic!("Expected Realloc event kind"),
        }
    }

    #[test]
    fn test_parse_malloc_event() {
        // Create a mock event with malloc data
        let mut event: bindings::event = unsafe { std::mem::zeroed() };
        event.header.event_type = bindings::EVENT_TYPE_MALLOC as u8;
        event.header.timestamp = 12345678;
        event.header.pid = 1000;
        event.header.tid = 2000;
        event.data.alloc.addr = 0x1000;
        event.data.alloc.size = 128;
        event.data.alloc.stack_hash = 0x1234;

        let bytes = event_bytes(&event);

        // Parse and validate:
        let parsed = parse_event(bytes).unwrap();
        assert_eq!(parsed.pid, 1000);
        assert_eq!(parsed.tid, 2000);
        assert_eq!(parsed.timestamp, 12345678);
        assert_eq!(parsed.addr, 0x1000);

        match parsed.kind {
            MemtrackEventKind::Malloc { size, stack_hash } => {
                assert_eq!(size, 128);
                assert_eq!(stack_hash, 0x1234);
            }
            _ => panic!("Expected Malloc event kind"),
        }
    }

    #[test]
    fn test_parse_rss_event() {
        let mut event: bindings::event = unsafe { std::mem::zeroed() };
        event.header.event_type = bindings::EVENT_TYPE_RSS as u8;
        event.header.timestamp = 12345678;
        event.header.pid = 1000;
        event.header.tid = 2000;
        event.data.rss.member = 1;
        event.data.rss.size = 4096 * 10;

        let bytes = event_bytes(&event);

        let parsed = parse_event(bytes).unwrap();
        assert_eq!(parsed.pid, 1000);
        assert_eq!(parsed.addr, 0);

        match parsed.kind {
            MemtrackEventKind::Rss { member, size } => {
                assert_eq!(member, 1);
                assert_eq!(size, 4096 * 10);
            }
            _ => panic!("Expected Rss event kind"),
        }
    }

    #[test]
    fn test_parse_rmap_event() {
        let mut event: bindings::event = unsafe { std::mem::zeroed() };
        event.header.event_type = bindings::EVENT_TYPE_RMAP as u8;
        event.header.timestamp = 12345678;
        event.header.pid = 1000;
        event.header.tid = 2000;
        event.data.rmap.member = 3;
        event.data.rmap.delta = 8;
        event.data.rmap.addr = 0x7f00;

        let bytes = event_bytes(&event);

        let parsed = parse_event(bytes).unwrap();
        assert_eq!(parsed.pid, 1000);
        assert_eq!(parsed.addr, 0x7f00);

        match parsed.kind {
            MemtrackEventKind::Rmap { member, delta } => {
                assert_eq!(member, 3);
                assert_eq!(delta, 8);
            }
            _ => panic!("Expected Rmap event kind"),
        }
    }

    #[test]
    fn test_parse_fork_event() {
        let mut event: bindings::event = unsafe { std::mem::zeroed() };
        event.header.event_type = bindings::EVENT_TYPE_FORK as u8;
        event.header.timestamp = 12345678;
        event.header.pid = 1001;
        event.header.tid = 2000;
        event.data.fork.parent_pid = 1000;

        let bytes = event_bytes(&event);

        let parsed = parse_event(bytes).unwrap();
        assert_eq!(parsed.pid, 1001);

        match parsed.kind {
            MemtrackEventKind::Fork { parent_pid } => {
                assert_eq!(parent_pid, 1000);
            }
            _ => panic!("Expected Fork event kind"),
        }
    }
}

#[cfg(test)]
mod stack_tests {
    use super::*;
    use crate::ebpf::events::bindings::stack_regs;

    fn encode(header: stack_header, payload: &[u8]) -> Vec<u8> {
        // SAFETY: The bindgen-generated C ABI struct is copied as bytes for a test fixture.
        let header_bytes = unsafe {
            std::slice::from_raw_parts(
                (&header as *const stack_header).cast::<u8>(),
                std::mem::size_of::<stack_header>(),
            )
        };
        let mut data = header_bytes.to_vec();
        data.extend_from_slice(payload);
        data
    }

    fn header(copy_len: u32) -> stack_header {
        stack_header {
            hash: 0x0123_4567_89ab_cdef,
            timestamp: 987_654_321,
            stackid: -17,
            sp: 0x7fff_1234_5000,
            pid: 41,
            tid: 42,
            copy_len,
            truncated: 1,
            _pad: [0; 3],
            regs: stack_regs {
                reg: std::array::from_fn(|index| 0x1000 + index as u64),
            },
        }
    }

    #[test]
    fn well_formed_record_round_trips_every_field() {
        let header = header(5);
        let payload = [1, 2, 3, 4, 5];

        let (event, stackid) = parse_stack(&encode(header, &payload)).unwrap();
        assert_eq!(event.pid, 41);
        assert_eq!(event.tid, 42);
        assert_eq!(event.timestamp, 987_654_321);
        assert_eq!(event.addr, 0);
        assert_eq!(stackid, -17);

        let MemtrackEventKind::Stack { record } = event.kind else {
            panic!("expected Stack event");
        };

        assert_eq!(record.hash, header.hash);
        assert_eq!(record.sp, header.sp);
        assert_eq!(record.regs, header.regs.reg.to_vec());
        assert_eq!(record.bytes, payload);
        assert!(record.fp_chain.is_empty());
        assert!(record.truncated);
    }

    #[test]
    fn truncated_buffer_returns_none() {
        let data = vec![0; std::mem::size_of::<stack_header>() - 1];
        assert!(parse_stack(&data).is_none());
    }

    #[test]
    fn missing_payload_returns_none() {
        assert!(parse_stack(&encode(header(4), &[1, 2, 3])).is_none());
    }
}
