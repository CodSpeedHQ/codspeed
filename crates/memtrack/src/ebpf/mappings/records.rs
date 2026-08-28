use crate::ebpf::events::bindings::mapping_record;

/// One executable file mapping as the BPF recorder saw it. The path is resolved
/// separately, per inode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MappingRecord {
    pub pid: u32,
    pub dev: u64,
    pub ino: u64,
    pub file_offset: u64,
    pub start: u64,
    pub end: u64,
    pub timestamp: u64,
}

impl MappingRecord {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < std::mem::size_of::<mapping_record>() {
            return None;
        }

        // SAFETY: the length is checked above, and the layout is the
        // bindgen-generated C ABI struct.
        let record: mapping_record = unsafe { std::ptr::read_unaligned(data.as_ptr().cast()) };
        Some(Self {
            pid: record.pid,
            dev: record.dev,
            ino: record.ino,
            file_offset: record.file_offset,
            start: record.start,
            end: record.end,
            timestamp: record.timestamp,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(record: mapping_record) -> Vec<u8> {
        // SAFETY: reading a plain-data struct as bytes.
        unsafe {
            std::slice::from_raw_parts(
                (&record as *const mapping_record).cast::<u8>(),
                std::mem::size_of::<mapping_record>(),
            )
        }
        .to_vec()
    }

    #[test]
    fn well_formed_record_round_trips_every_field() {
        let bytes = encode(mapping_record {
            dev: 0x1_0002,
            ino: 4242,
            file_offset: 0x2000,
            start: 0x5555_5555_0000,
            end: 0x5555_5556_0000,
            timestamp: 987_654_321,
            pid: 7,
            _pad: 0,
        });

        assert_eq!(
            MappingRecord::parse(&bytes),
            Some(MappingRecord {
                pid: 7,
                dev: 0x1_0002,
                ino: 4242,
                file_offset: 0x2000,
                start: 0x5555_5555_0000,
                end: 0x5555_5556_0000,
                timestamp: 987_654_321,
            })
        );
    }

    #[test]
    fn truncated_buffer_returns_none() {
        assert!(MappingRecord::parse(&[0u8; 8]).is_none());
    }
}
