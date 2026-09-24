use std::fs::File;
use std::io::{BufReader, Read};
use std::path::PathBuf;

use clap::Parser;
use memtrack::stack_codec::{RawStack, StackDecoder, StackEncoder, fnv_stack_hash};

#[derive(Parser, Debug)]
#[command(name = "stack_codec_ratio")]
struct Args {
    #[arg(long, default_value_t = 100_000)]
    limit: usize,

    #[arg(required = true)]
    dumps: Vec<PathBuf>,
}

struct DumpRecord {
    hash: u64,
    timestamp: u64,
    pid: u32,
    tid: u32,
    stack: RawStack,
}

// Format defined in .agents/scripts/stackdump.py:
// struct.pack("<QQIIQIBBH", hash, timestamp, pid, tid, sp, copy_len, truncated, nregs, nfp)
// followed by nregs * u64, nfp * u64, and copy_len * u8.
fn read_record(r: &mut impl Read) -> std::io::Result<Option<DumpRecord>> {
    let mut hdr = [0u8; 8 + 8 + 4 + 4 + 8 + 4 + 1 + 1 + 2];
    match r.read_exact(&mut hdr) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }

    let expected_hash = u64::from_le_bytes(hdr[0..8].try_into().unwrap());
    let timestamp = u64::from_le_bytes(hdr[8..16].try_into().unwrap());
    let pid = u32::from_le_bytes(hdr[16..20].try_into().unwrap());
    let tid = u32::from_le_bytes(hdr[20..24].try_into().unwrap());
    let sp = u64::from_le_bytes(hdr[24..32].try_into().unwrap());
    let copy_len = u32::from_le_bytes(hdr[32..36].try_into().unwrap()) as usize;
    let truncated = hdr[36] != 0;
    let nregs = hdr[37] as usize;
    let nfp = u16::from_le_bytes(hdr[38..40].try_into().unwrap()) as usize;

    let mut reg_bytes = vec![0u8; nregs * 8];
    r.read_exact(&mut reg_bytes)?;
    let mut fp_bytes = vec![0u8; nfp * 8];
    r.read_exact(&mut fp_bytes)?;
    let mut bytes = vec![0u8; copy_len];
    r.read_exact(&mut bytes)?;

    let mut regs = [0u64; 33];
    for (i, chunk) in reg_bytes.chunks_exact(8).enumerate() {
        if i < 33 {
            regs[i] = u64::from_le_bytes(chunk.try_into().unwrap());
        }
    }

    Ok(Some(DumpRecord {
        hash: expected_hash,
        timestamp,
        pid,
        tid,
        stack: RawStack {
            sp,
            regs,
            bytes,
            truncated,
        },
    }))
}

fn process_dump(path: &PathBuf, limit: usize) -> anyhow::Result<()> {
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(1 << 20, file);

    let mut encoder = StackEncoder::default();
    let mut decoder = StackDecoder::default();

    let mut record_count = 0usize;
    let mut total_raw_bytes = 0u64;
    let mut total_encoded_bytes = 0u64;

    while record_count < limit {
        let Some(DumpRecord {
            hash: expected_hash,
            timestamp,
            pid,
            tid,
            stack,
        }) = read_record(&mut reader)?
        else {
            break;
        };

        // Validate FNV implementation against the recorded capture hash
        let computed_hash = fnv_stack_hash(&stack.bytes);
        assert_eq!(
            computed_hash,
            expected_hash,
            "FNV hash mismatch in {}: record {record_count}, expected {expected_hash:#x}, got {computed_hash:#x}",
            path.display()
        );

        let raw_record_size = (312 + stack.bytes.len()) as u64; // raw record = 312 B header + copy_len
        total_raw_bytes += raw_record_size;

        let encoded = encoder.encode(pid, tid, timestamp, 0, &stack);
        total_encoded_bytes += encoded.len() as u64;

        let (event, _) = decoder.decode(&encoded).expect("decode failed");
        if let runner_shared::artifacts::MemtrackEventKind::Stack { record } = event.kind {
            assert_eq!(record.hash, expected_hash);
            assert_eq!(record.bytes, stack.bytes);
            assert_eq!(record.sp, stack.sp);
            assert_eq!(&record.regs[..], &stack.regs[..]);
            assert_eq!(record.truncated, stack.truncated);
        } else {
            panic!("expected Stack event");
        }

        record_count += 1;
    }

    let ratio = total_raw_bytes as f64 / total_encoded_bytes as f64;
    let bytes_per_record = total_encoded_bytes as f64 / record_count as f64;

    println!(
        "{}: records={}, raw_bytes={}, encoded_bytes={}, ratio={:.2}x, bytes/record={:.1}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        record_count,
        total_raw_bytes,
        total_encoded_bytes,
        ratio,
        bytes_per_record
    );

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    for dump in &args.dumps {
        process_dump(dump, args.limit)?;
    }
    Ok(())
}
