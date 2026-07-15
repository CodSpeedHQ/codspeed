use runner_shared::artifacts::{MemtrackEvent, MemtrackEventKind};

// Include the bindings for event.h
pub mod bindings {
    #![allow(non_upper_case_globals)]
    #![allow(non_camel_case_types)]
    #![allow(non_snake_case)]
    #![allow(dead_code)]

    include!(concat!(env!("OUT_DIR"), "/event.rs"));
}
use bindings::*;

/// Parse a batch record from raw bytes, emitting each valid event.
///
/// SAFETY: The data must be a valid `bindings::event_batch`
pub fn parse_batch(data: &[u8], mut emit: impl FnMut(MemtrackEvent)) {
    if data.len() < std::mem::size_of::<bindings::event_batch>() {
        return;
    }

    // The bytes may come from a buffer without the struct's alignment
    // (e.g. a per-CPU map lookup), so copy the batch out before reading fields.
    let batch = unsafe { std::ptr::read_unaligned(data.as_ptr() as *const bindings::event_batch) };
    let count = (batch.count as usize).min(batch.events.len());
    for event in &batch.events[..count] {
        emit(parse_one(event));
    }
}

fn parse_one(event: &bindings::event) -> MemtrackEvent {
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
                },
            ),
            EVENT_TYPE_FREE => (event.data.free.addr, MemtrackEventKind::Free),
            EVENT_TYPE_CALLOC => (
                event.data.alloc.addr,
                MemtrackEventKind::Calloc {
                    size: event.data.alloc.size,
                },
            ),
            EVENT_TYPE_REALLOC => (
                event.data.realloc.new_addr,
                MemtrackEventKind::Realloc {
                    old_addr: Some(event.data.realloc.old_addr),
                    size: event.data.realloc.size,
                },
            ),
            EVENT_TYPE_ALIGNED_ALLOC => (
                event.data.alloc.addr,
                MemtrackEventKind::AlignedAlloc {
                    size: event.data.alloc.size,
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
            unknown => {
                panic!("Unknown event type: {unknown}");
            }
        }
    };

    MemtrackEvent {
        pid,
        tid,
        timestamp,
        addr,
        kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_single(event: bindings::event) -> MemtrackEvent {
        let mut batch: bindings::event_batch = unsafe { std::mem::zeroed() };
        batch.count = 1;
        batch.events[0] = event;

        let bytes = unsafe {
            std::slice::from_raw_parts(
                &batch as *const _ as *const u8,
                std::mem::size_of_val(&batch),
            )
        };

        let mut parsed = Vec::new();
        parse_batch(bytes, |event| parsed.push(event));
        assert_eq!(parsed.len(), 1);
        parsed.pop().unwrap()
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

        let parsed = parse_single(event);
        assert_eq!(parsed.pid, 1000);
        assert_eq!(parsed.tid, 2000);
        assert_eq!(parsed.timestamp, 12345678);
        assert_eq!(parsed.addr, 0x2000);

        match parsed.kind {
            MemtrackEventKind::Realloc { old_addr, size } => {
                assert_eq!(old_addr, Some(0x1000));
                assert_eq!(size, 256);
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

        let parsed = parse_single(event);
        assert_eq!(parsed.pid, 1000);
        assert_eq!(parsed.tid, 2000);
        assert_eq!(parsed.timestamp, 12345678);
        assert_eq!(parsed.addr, 0x1000);

        match parsed.kind {
            MemtrackEventKind::Malloc { size } => {
                assert_eq!(size, 128);
            }
            _ => panic!("Expected Malloc event kind"),
        }
    }
}
