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

    Some(MemtrackEvent {
        pid,
        tid,
        timestamp,
        addr,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let bytes = unsafe {
            std::slice::from_raw_parts(
                &event as *const _ as *const u8,
                std::mem::size_of_val(&event),
            )
        };

        // Parse and validate:
        let parsed = parse_event(bytes).unwrap();
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

        let bytes = unsafe {
            std::slice::from_raw_parts(
                &event as *const _ as *const u8,
                std::mem::size_of_val(&event),
            )
        };

        // Parse and validate:
        let parsed = parse_event(bytes).unwrap();
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
