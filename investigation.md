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

## Root Cause

### Found: A `PERF_RECORD_COMPRESSED2` record with `size=0`

Raw scan of the file (walking header by header) found the first invalid record at:

| Field                                      | Value                                            |
| ------------------------------------------ | ------------------------------------------------ |
| Raw record index                           | 730,541                                          |
| Byte offset                                | `0x8b14e410` (2,333,402,128 bytes into the file) |
| `type`                                     | 83 = `PERF_RECORD_COMPRESSED2`                   |
| `misc`                                     | 0                                                |
| `size` (u16)                               | **0** (invalid: must be ≥ 8)                     |
| `data_size` (next 8 bytes at `0x8b14e418`) | `0xffef` = **65,519**                            |

### Kernel Bug: u16 overflow in `builtin-record.c`

The bug is in `tools/perf/builtin-record.c:672`:

```c
event->data_size = compressed - sizeof(struct perf_record_compressed2);
event->header.size = PERF_ALIGN(compressed, sizeof(u64));  // BUG: u16 overflow
```

`compressed` is the total byte count returned by `zstd_compress_stream_to_records`, which includes the 16-byte struct header plus the zstd output. `PERF_ALIGN` rounds it up to 8-byte alignment and the result is assigned to `header.size`, which is a `__u16` (max 65,535).

`max_record_size` is computed as:

```c
// tools/perf/util/event.h
#define PERF_SAMPLE_MAX_SIZE (1 << 16)  // = 65536

// builtin-record.c
size_t max_record_size = PERF_SAMPLE_MAX_SIZE - sizeof(struct perf_record_compressed2) - 1;
//                     = 65536 - 16 - 1 = 65519
```

The `-1` was intended to prevent overflow, but it only caps the zstd output. The alignment step happens **after**:

```
compressed = 16 (struct header) + output.pos (zstd bytes, max 65519)
           = max 65535

PERF_ALIGN(65535, 8) = 65536

(u16)65536 = 0  ← BUG
```

Any `compressed` value in `[65529, 65535]` triggers this: `PERF_ALIGN` rounds up to 65536, which wraps to 0 in `__u16`.

For our specific record: `compressed = 65535` → `PERF_ALIGN = 65536` → `header.size = 0`. The `data_size` field (`65519`) is computed correctly and is trustworthy — it's just `compressed - 16`.

### Why Ubuntu 24.04 / kernel 6.17 and not 22.04 / kernel 6.5

- Kernel **6.5** (Ubuntu 22.04 AWS) predates `PERF_RECORD_COMPRESSED2` → uses only `PERF_RECORD_COMPRESSED` (type 81), which has no `data_size` field and a different size calculation that doesn't hit this overflow → works fine.
- Kernel **6.17** (Ubuntu 24.04 AWS) introduced `PERF_RECORD_COMPRESSED2` (type 83) with the buggy alignment assignment → triggers the u16 overflow when compressed output is large enough.

## Reproducibility

The bug is **probabilistic** — it triggers when the zstd output for a single flush lands in the 7-byte window `[65529, 65535]` out of 65,535 possible sizes. In the original file it happened once out of 417,241 `COMPRESSED2` records in a 4GB file.

Attempts to reproduce via `perf record --compression-level=3` with various `-m` (mmap pages) values and a heavy multi-threaded Python workload at 9997Hz sampling did not trigger the bug on this machine (kernel 6.12.70). The original file's max `COMPRESSED2` size before the bad record was 61,432 bytes — so even with an identical workload, hitting the exact overflow window is not guaranteed.

A deterministic repro via `perf record` is not practical without either:

1. Running the exact same workload that produced the original file, or
2. Patching perf/kernel to lower `PERF_SAMPLE_MAX_SIZE` to make the window easier to hit, or
3. Writing a unit test that directly calls `zstd_compress_stream_to_records` with crafted input producing exactly 65529–65535 bytes of output.

The original file `/home/guillaume/cod-2314/profile.pPMUwlf7Pu.out/perf.pipedata` serves as the concrete test case.

## Fix / Workaround

### Kernel fix

In `tools/perf/builtin-record.c:672`, cast to a wider type before aligning:

```c
event->header.size = (u16)PERF_ALIGN(compressed, sizeof(u64));
// Should assert or clamp: aligned value must fit in u16
```

Or cap `max_record_size` to ensure `PERF_ALIGN(16 + max_record_size, 8) <= 65535`.

### Parser workaround (in `linux-perf-data`)

When a `PERF_RECORD_COMPRESSED2` record has `header.size < 8`, recover the actual size from the `data_size` field (next 8 bytes):

```
actual_size = PERF_ALIGN(sizeof(perf_record_compressed2) + data_size, 8)
            = PERF_ALIGN(16 + data_size, 8)
```

Since `data_size` is set correctly (`compressed - 16`), this gives the right number of bytes to consume.

## Related Code

- Parser panic: `src/executor/wall_time/perf/parse_perf_file.rs:69-70`
- Library error: `linux-perf-data/src/file_reader.rs` — `read_next_round_impl`, returns `Error::InvalidPerfEventSize` when `size < 8`
- Kernel bug: `tools/perf/builtin-record.c:672` in `~/projects/linux`
- Record struct: `tools/lib/perf/include/perf/event.h:477` — `struct perf_record_compressed2`

## Diagnostic Tools

- `src/bin/diagnose_perf_file.rs` — raw perf file walker, finds and dumps context around the first invalid record
- `src/bin/count_event_records.rs` — runs the library and counts EventRecords until failure
