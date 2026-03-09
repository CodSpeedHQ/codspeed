/// Diagnostic tool to find the byte offset of the corrupted record in a perf pipedata file.
/// Manually walks the file header-by-header to pinpoint where corruption starts.
use std::io::{BufReader, Read};

const PERF_EVENT_HEADER_SIZE: usize = 8; // type(4) + misc(2) + size(2)
const PIPE_HEADER_SIZE: usize = 16; // magic(8) + size(8)

fn main() {
    let perf_file_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/home/guillaume/cod-2314/profile.pPMUwlf7Pu.out/perf.pipedata".to_string());
    let target_record_index: u64 = 3_423_125;

    let file = std::fs::File::open(&perf_file_path).expect("Failed to open perf file");
    let file_size = file.metadata().unwrap().len();
    println!("File size: {} bytes ({:.2} GiB)", file_size, file_size as f64 / (1024.0 * 1024.0 * 1024.0));

    let mut reader = BufReader::with_capacity(1024 * 1024, file);

    // Read and validate pipe header
    let mut pipe_header = [0u8; PIPE_HEADER_SIZE];
    reader.read_exact(&mut pipe_header).expect("Failed to read pipe header");
    println!("Pipe header magic: {:?}", std::str::from_utf8(&pipe_header[0..8]).unwrap_or("invalid utf8"));
    let pipe_header_size = u64::from_le_bytes(pipe_header[8..16].try_into().unwrap());
    println!("Pipe header declared size: {}", pipe_header_size);

    let mut byte_offset: u64 = pipe_header_size; // start after the pipe header
    // If there are extra header bytes, skip them
    if pipe_header_size > PIPE_HEADER_SIZE as u64 {
        let extra = pipe_header_size - PIPE_HEADER_SIZE as u64;
        let mut skip = vec![0u8; extra as usize];
        reader.read_exact(&mut skip).expect("Failed to skip extra pipe header bytes");
    }

    let mut record_index: u64 = 0;
    let dump_around = 5; // dump N records before and after the bad one

    // We'll store the last N records' offsets to show context
    let mut recent_offsets: std::collections::VecDeque<(u64, u32, u16, u16)> = std::collections::VecDeque::new();

    loop {
        let record_start = byte_offset;

        let mut header_bytes = [0u8; PERF_EVENT_HEADER_SIZE];
        match reader.read_exact(&mut header_bytes) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                println!("\n[EOF] Reached end of file at byte offset {:#x} ({}) after {} records", byte_offset, byte_offset, record_index);
                break;
            }
            Err(e) => {
                println!("\n[ERROR] Failed to read header at offset {:#x}: {}", byte_offset, e);
                break;
            }
        }

        let record_type = u32::from_le_bytes(header_bytes[0..4].try_into().unwrap());
        let misc = u16::from_le_bytes(header_bytes[4..6].try_into().unwrap());
        let size = u16::from_le_bytes(header_bytes[6..8].try_into().unwrap());

        recent_offsets.push_back((record_start, record_type, misc, size));
        if recent_offsets.len() > (dump_around * 2 + 1) as usize {
            recent_offsets.pop_front();
        }

        if size < PERF_EVENT_HEADER_SIZE as u16 {
            println!(
                "\n[CORRUPTION FOUND] Record #{} at byte offset {:#x} ({}):",
                record_index, record_start, record_start
            );
            println!(
                "  type={} misc={:#06x} size={} (INVALID: must be >= {})",
                record_type, misc, size, PERF_EVENT_HEADER_SIZE
            );
            println!("\nRaw bytes at offset {:#x}:", record_start);
            // Re-read context: go back and show 64 bytes before + 64 bytes after
            let ctx_before = 64usize;
            let ctx_after = 64usize;
            let ctx_start = record_start.saturating_sub(ctx_before as u64);
            let ctx_len = ctx_before + ctx_after;

            // We need to seek; use a fresh file handle for this
            use std::io::{Seek, SeekFrom};
            let ctx_file = std::fs::File::open(perf_file_path).unwrap();
            let mut ctx_reader = BufReader::new(ctx_file);
            ctx_reader.seek(SeekFrom::Start(ctx_start)).unwrap();
            let mut ctx_bytes = vec![0u8; ctx_len];
            let n = ctx_reader.read(&mut ctx_bytes).unwrap();
            ctx_bytes.truncate(n);

            println!("Context (from offset {:#x}):", ctx_start);
            for (i, chunk) in ctx_bytes.chunks(16).enumerate() {
                let offset = ctx_start + (i * 16) as u64;
                let marker = if offset == record_start { " <-- BAD HEADER" } else { "" };
                let hex: Vec<String> = chunk.iter().map(|b| format!("{:02x}", b)).collect();
                let ascii: String = chunk.iter().map(|b| if b.is_ascii_graphic() || *b == b' ' { *b as char } else { '.' }).collect();
                println!("  {:#010x}: {:47}  |{}|{}", offset, hex.join(" "), ascii, marker);
            }

            println!("\nLast {} records before the bad one:", recent_offsets.len().saturating_sub(1));
            for (off, rtype, rmis, rsz) in recent_offsets.iter() {
                let marker = if *off == record_start { " <-- BAD" } else { "" };
                println!("  offset={:#x} type={} misc={:#06x} size={}{}", off, rtype, rmis, rsz, marker);
            }
            break;
        }

        // Skip the body of this record
        let body_len = (size as usize) - PERF_EVENT_HEADER_SIZE;
        if body_len > 0 {
            let mut body = vec![0u8; body_len];
            match reader.read_exact(&mut body) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    println!(
                        "\n[TRUNCATED] Record #{} at offset {:#x}: header says size={} but file ended reading body. type={}",
                        record_index, record_start, size, record_type
                    );
                    println!("  This is a truncated/incomplete record at the end of the file.");
                    break;
                }
                Err(e) => {
                    println!("\n[READ ERROR] Record #{} at offset {:#x}: {}", record_index, record_start, e);
                    break;
                }
            }
        }

        byte_offset += size as u64;
        record_index += 1;

        if record_index % 500_000 == 0 {
            println!(
                "Progress: {} records, offset={:#x} ({:.1}% of file)",
                record_index,
                byte_offset,
                byte_offset as f64 / file_size as f64 * 100.0
            );
        }

        // Extra check near the target record: print nearby records
        if record_index >= target_record_index.saturating_sub(dump_around)
            && record_index < target_record_index + dump_around
        {
            println!(
                "  [near target] Record #{} offset={:#x} type={} misc={:#06x} size={}",
                record_index, record_start, record_type, misc, size
            );
        }
    }
}
// placeholder - checking existing content
