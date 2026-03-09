# PERF_RECORD_COMPRESSED2 header.size overflow workaround

## The bug

The perf tool (not kernel proper) has a u16 overflow in `tools/perf/builtin-record.c:record__pushfn()`:

```c
event->data_size = compressed - sizeof(struct perf_record_compressed2);
event->header.size = PERF_ALIGN(compressed, sizeof(u64));  // u16 overflow
padding = event->header.size - compressed;                  // size_t underflow
return record__write(rec, map, bf, compressed) ||
       record__write(rec, map, &pad, padding);
```

`compressed` is the total byte count returned by `zstd_compress_stream_to_records()`, which includes the 16-byte struct header(s) plus zstd output. `PERF_ALIGN` rounds up to 8-byte alignment and the result is assigned to `header.size`, which is `__u16` (max 65,535).

The zstd output per sub-record is capped at `max_record_size = PERF_SAMPLE_MAX_SIZE - sizeof(struct perf_record_compressed2) - 1 = 65,519`, but:

- **Single-record case:** `compressed` can reach 65,535 (= 16 + 65,519). `PERF_ALIGN(65,535, 8) = 65,536` → `(u16)65,536 = 0`.
- **Multi-record case:** when `zstd_compress_stream_to_records()` loops multiple times, `compressed` is the **total** across all sub-records and can exceed 65,535. The truncated `(u16)` value wraps modulo 65,536 to a small but plausible size.

In both cases, the subsequent padding write (`record__write(&pad, padding)`) also fails silently: `padding = header.size - compressed` underflows to ~18 exabytes, `writen()` gets EFAULT, but `record__pushfn` returns 1 (via `||`), and `perf_mmap__push` only checks `< 0`, so perf continues normally. This means:

- The on-disk record is exactly `compressed` bytes (no alignment padding).
- The stream is NOT corrupted beyond the affected record — the next record follows immediately.

Introduced in kernel v6.16 (commit `208c0e168344`, "perf record: Add 8-byte aligned event type PERF_RECORD_COMPRESSED2"). Does not affect `PERF_RECORD_COMPRESSED` (type 81) used in earlier kernels.

## On-disk layouts

### Single-record overflow (compressed ∈ [65,529, 65,535])

The zstd output fit in one sub-record but `PERF_ALIGN` overflowed. `header.size = 0`, `data_size` is correct.

```
[header: type=83, misc=0, size=0 (8 bytes)]
[data_size: correct value ≤ 65,519 (8 bytes)]
[zstd compressed data (data_size bytes)]
-- no padding --
[next valid record]
```

### Multi-record overflow (compressed > 65,535)

`zstd_compress_stream_to_records()` produced multiple sub-records. `record__pushfn` overwrote the first sub-record's `header.size` and `data_size` with values based on the total. Sub-records 2+ have intact headers (set by `process_comp_header`) but uninitialized `data_size` fields.

```
[header1: type=83, misc=0, size=TRUNCATED (8 bytes)]  ← corrupted
[data_size1: total_compressed - 16 (8 bytes)]          ← corrupted (total, not per-record)
[zstd1 compressed data (up to 65,519 bytes)]
[header2: type=83, misc=0, size=CORRECT (8 bytes)]    ← intact
[data_size2: UNINITIALIZED (8 bytes)]                  ← garbage
[zstd2 compressed data (header2.size - 16 bytes)]
... more sub-records possible ...
-- no padding --
[next valid record]
```

## Detection

For any COMPRESSED2 record, after reading `data_size` from the body:

```
expected_aligned = ALIGN(header_size + data_size_field_size + data_size, 8)
                 = (8 + 8 + data_size + 7) & !7
```

If `expected_aligned > header.size`, the header overflowed. The special case `header.size = 0` is caught as well since `expected_aligned` is always ≥ 24.

## Recovery

1. Read the full on-disk body: `8 + data_size` bytes total (data_size field + payload), reading additional bytes from the stream beyond what `header.size` indicated.
2. Advance `read_offset` by `header_size + 8 + data_size` (no padding).
3. If `data_size ≤ 65,519` (single sub-record): decompress the payload as one zstd stream.
4. If `data_size > 65,519` (multi-record): scan the payload for sub-record boundaries using intact COMPRESSED2 headers of records 2+. Each sub-record's zstd data size is `sub_header.size - 16`. Decompress each sub-record's zstd stream independently.

## Implementation

`file_reader.rs`, in `read_next_round_impl()`: the COMPRESSED2 handling block detects overflow by comparing `data_size` against `header.size`, reads the full payload, and dispatches to single-record or multi-record decompression.

`find_tail_sub_records()`: scans a payload buffer for type=83/misc=0 headers that chain exactly to the end of the buffer, returning the offset and size of each tail sub-record.
