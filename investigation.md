# Perf File Corruption Investigation

## File
`/home/guillaume/cod-2314/profile.pPMUwlf7Pu.out/perf.pipedata`
- Size: 4,445,779,093 bytes (4.14 GiB)
- Format: `PERFILE2` pipe mode (little-endian)
- Pipe header: 16 bytes (magic=`PERFILE2`, size=16)

## Symptom
```
thread 'main' panicked at src/executor/wall_time/perf/parse_perf_file.rs:70:9:
Failed to read record at index 3423125: The specified size in the perf event header was smaller than the header itself
```
(`perf script` also fails with the same kind of error.)

The `record_index` in the panic (3,423,125) counts **EventRecords successfully returned by the sorter** before the failure. Confirmed via two independent methods:

1. **`src/bin/count_event_records.rs`**: running the actual `linux-perf-data` library and counting EventRecords until the first error yields exactly 3,423,125.
2. **Instrumented library log**: adding an `eprintln!` in `linux-perf-data/src/file_reader.rs` at the `InvalidPerfEventSize` error site reports `read_offset=0x8b14cd00`. This is a relative offset (from after the pipe header + metadata records). Converting to absolute: `16 (pipe header) + 5888 (metadata) + 0x8b14cd00 = 0x8b14e410` — exactly the raw file offset our scanner found. **The bad record is definitively the same one.**

## Root Cause Hypothesis

### Found: A `PERF_RECORD_COMPRESSED2` record with `size=0`

Raw scan of the file (walking header by header) found the first invalid record at:

| Field | Value |
|-------|-------|
| Raw record index | 730,541 |
| Byte offset | `0x8b14e410` (2,333,402,128 bytes into the file) |
| `type` | 83 = `PERF_RECORD_COMPRESSED2` |
| `misc` | 0 |
| `size` (u16) | **0** (invalid: must be ≥ 8) |
| `data_size` (next 8 bytes at `0x8b14e418`) | `0xffef` = **65,519** |

### The Math

`PERF_RECORD_COMPRESSED2` (type 83, Linux 6.12+) format:
```
[perf_event_header: 8 bytes]
  .type  = 83
  .misc  = 0
  .size  = total record size (u16, max 65535)
[data_size: 8 bytes]  ← actual compressed payload length
[compressed data: data_size bytes]
[padding to 8-byte alignment]
```

For this record:
```
total = 8 (header) + 8 (data_size field) + 65519 (data) = 65535 bytes
65535 % 8 = 7  →  needs 1 byte of alignment padding  →  65535 + 1 = 65536 bytes
65536 as u16 = 0  ← WRAPS TO ZERO
```

### Hypothesis: Kernel u16 overflow bug

The `perf_event_header.size` field is a `u16` (max 65,535). When a `COMPRESSED2` record's aligned total size is exactly **65,536 bytes**, the u16 field wraps to **0**.

This is likely a kernel bug introduced with `PERF_RECORD_COMPRESSED2` where the size calculation was not protected against u16 overflow.

## Evidence Supporting This

1. All preceding `COMPRESSED2` records have `size` < 65,535 (sizes seen: 10,328 / 19,976 / 23,112 / 25,464 / 41,200 / 42,272 / 42,704).
2. The exact arithmetic lines up: `data_size = 65519` → unpadded total = 65,535 → padded = 65,536 → u16 = 0.
3. Both `perf script` and the Rust parser fail at the same location — the file content is consistent, the header field is just wrong.
4. The file is likely **not corrupted by truncation or write error** — the data at the record's location looks like a valid continuation of zstd stream data.

## Why Ubuntu 24.04 and Not 22.04

- Ubuntu 22.04 ships kernel **5.15** → uses `PERF_RECORD_COMPRESSED` (type 81, Linux 5.2+), which does NOT have a `data_size` field and has smaller records that never reach the u16 overflow boundary.
- Ubuntu 24.04 (and the ARM64 test machine running kernel **6.12.70**) uses `PERF_RECORD_COMPRESSED2` (type 83, Linux 6.x / May 2025), which adds an 8-byte `data_size` field — making records large enough to trigger the u16 overflow.
- `perf script` also fails on this file for the same reason (not a parser-specific bug).

## Open Questions / Next Steps

1. **Confirm the actual record size is 65535 or 65536** by verifying that `0x8b14e410 + 65535` or `0x8b14e410 + 65536` lands on a valid next record.
   - `+65535 = 0x8b15e40f` → found `type=83 size=33168` there, BUT this address is not 8-byte aligned (ends in `0xf`), which is suspicious.
   - `+65536 = 0x8b15e410` → found `type=0` there, which is invalid.
   - **Neither offset cleanly validates** — needs more investigation.

2. **Is the data_size field itself trustworthy?** Could `size=0` be a different kind of overflow where `data_size` is also wrong?

3. **Does the kernel write padding into `size` or not?** Kernel source for `PERF_RECORD_COMPRESSED2` size calculation needs to be verified. If the kernel does NOT include alignment padding in `size`, then the actual size is 65,535 (which also fits in u16 as `0xffff`, not 0). This means the actual `data_size` must be larger than 65,519 for `size` to overflow to 0.

4. **Scan for more `size=0` records** to see if this happens multiple times in the file.

5. **Workaround for the parser**: Handle `size=0` on `PERF_RECORD_COMPRESSED2` records by reading the `data_size` field from the next 8 bytes to reconstruct the actual record size.

## Related Code

- Parser: `src/executor/wall_time/perf/parse_perf_file.rs:69-70` — panics on `InvalidPerfEventSize`
- Library error: `linux-perf-data/src/file_reader.rs:467-468` — returns `Error::InvalidPerfEventSize` when `size < 8`
- Decompressor: `linux-perf-data/src/decompression.rs` — uses stateful zstd streaming context across chunks

## Diagnostic Tool

`src/bin/diagnose_perf_file.rs` — raw perf file walker that finds and dumps context around the first invalid record.
