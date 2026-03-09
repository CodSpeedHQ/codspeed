/// Walk the perf pipedata file raw, decompress COMPRESSED2 blocks,
/// count EventRecords, and report which raw file offset contains the
/// record that causes the library to hit EventRecord index 3,423,125.
use std::io::{BufReader, Read, Seek, SeekFrom};

const PERF_EVENT_HEADER_SIZE: usize = 8;
const PIPE_HEADER_SIZE: usize = 16;
const TARGET_EVENT_RECORD_INDEX: u64 = 3_423_125;

// Record types
const PERF_RECORD_COMPRESSED2: u32 = 83;
const PERF_RECORD_FINISHED_ROUND: u32 = 68;

fn main() {
    let perf_file_path = "/home/guillaume/cod-2314/profile.pPMUwlf7Pu.out/perf.pipedata";

    let file = std::fs::File::open(perf_file_path).expect("Failed to open perf file");
    let mut reader = BufReader::with_capacity(4 * 1024 * 1024, file);

    // Skip pipe header
    let mut pipe_header = [0u8; PIPE_HEADER_SIZE];
    reader.read_exact(&mut pipe_header).unwrap();
    let pipe_size = u64::from_le_bytes(pipe_header[8..16].try_into().unwrap());
    if pipe_size > PIPE_HEADER_SIZE as u64 {
        let mut skip = vec![0u8; (pipe_size - PIPE_HEADER_SIZE as u64) as usize];
        reader.read_exact(&mut skip).unwrap();
    }

    let mut byte_offset: u64 = pipe_size;
    let mut event_record_count: u64 = 0;

    let dctx = zstd::bulk::Decompressor::new().unwrap();
    let _ = dctx; // we'll use streaming

    loop {
        let record_start = byte_offset;

        let mut hdr = [0u8; PERF_EVENT_HEADER_SIZE];
        match reader.read_exact(&mut hdr) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                println!("EOF at offset {:#x} after {} EventRecords", byte_offset, event_record_count);
                break;
            }
            Err(e) => { eprintln!("Read error: {e}"); break; }
        }

        let record_type = u32::from_le_bytes(hdr[0..4].try_into().unwrap());
        let size_u16 = u16::from_le_bytes(hdr[6..8].try_into().unwrap());

        if size_u16 == 0 && record_type == PERF_RECORD_COMPRESSED2 {
            // Read data_size to recover actual record size
            let mut ds_bytes = [0u8; 8];
            reader.read_exact(&mut ds_bytes).unwrap();
            let data_size = u64::from_le_bytes(ds_bytes);
            let total_unpadded = PERF_EVENT_HEADER_SIZE as u64 + 8 + data_size;
            let total_padded = (total_unpadded + 7) & !7;
            println!(
                "Found size=0 COMPRESSED2 at raw offset {:#x}: data_size={} total_padded={}",
                record_start, data_size, total_padded
            );
            println!(
                "EventRecord count at this point: {} (target: {})",
                event_record_count, TARGET_EVENT_RECORD_INDEX
            );
            // Read and decompress the payload
            let mut compressed = vec![0u8; data_size as usize];
            reader.read_exact(&mut compressed).unwrap();
            // skip padding
            let padding = (total_padded - total_unpadded) as usize;
            if padding > 0 {
                let mut pad = vec![0u8; padding];
                reader.read_exact(&mut pad).unwrap();
            }
            // Count event records inside
            let records_in_block = count_records_in_compressed(&compressed);
            println!(
                "  EventRecords inside this block: {} (would bring total to {})",
                records_in_block,
                event_record_count + records_in_block
            );
            byte_offset += total_padded;
            event_record_count += records_in_block;
            continue;
        }

        if size_u16 < PERF_EVENT_HEADER_SIZE as u16 {
            println!(
                "Invalid size={} at offset {:#x} type={} after {} EventRecords",
                size_u16, record_start, record_type, event_record_count
            );
            break;
        }

        let body_len = (size_u16 as usize) - PERF_EVENT_HEADER_SIZE;
        let mut body = vec![0u8; body_len];
        reader.read_exact(&mut body).unwrap();
        byte_offset += size_u16 as u64;

        if record_type == PERF_RECORD_COMPRESSED2 {
            // Normal (non-overflow) COMPRESSED2: data_size is first 8 bytes of body
            let data_size = u64::from_le_bytes(body[0..8].try_into().unwrap());
            let compressed = &body[8..8 + data_size as usize];
            let records_in_block = count_records_in_compressed(compressed);
            event_record_count += records_in_block;

            if event_record_count >= TARGET_EVENT_RECORD_INDEX {
                println!(
                    "Target EventRecord index {} crossed at raw offset {:#x} (COMPRESSED2 block, data_size={})",
                    TARGET_EVENT_RECORD_INDEX, record_start, data_size
                );
            }
        } else if record_type != PERF_RECORD_FINISHED_ROUND {
            // Direct EventRecord (non-compressed)
            event_record_count += 1;
            if event_record_count == TARGET_EVENT_RECORD_INDEX {
                println!(
                    "Target EventRecord index {} is at raw offset {:#x} type={}",
                    TARGET_EVENT_RECORD_INDEX, record_start, record_type
                );
            }
        }

        if event_record_count % 500_000 == 0 && event_record_count > 0 {
            // avoid spamming, only print on exact multiples (will repeat in tight loops, use a flag in real code)
        }
    }
}

/// Decompress a zstd-compressed block and count the perf EventRecords inside it.
/// Returns the number of non-user-type records (type < 64).
fn count_records_in_compressed(compressed: &[u8]) -> u64 {
    let decompressed = match zstd::decode_all(compressed) {
        Ok(d) => d,
        Err(_) => return 0,
    };

    let mut count = 0u64;
    let mut pos = 0usize;
    while pos + PERF_EVENT_HEADER_SIZE <= decompressed.len() {
        let record_type = u32::from_le_bytes(decompressed[pos..pos+4].try_into().unwrap());
        let size = u16::from_le_bytes(decompressed[pos+6..pos+8].try_into().unwrap()) as usize;
        if size < PERF_EVENT_HEADER_SIZE {
            break;
        }
        // builtin types are < 64; user types (FINISHED_ROUND etc) are >= 64
        if record_type < 64 {
            count += 1;
        }
        pos += size;
    }
    count
}
