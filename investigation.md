# Perf File Corruption Investigation

## Test file

`/home/guillaume/cod-2314/profile.pPMUwlf7Pu.out/perf.pipedata`
— 4,445,779,093 bytes, `PERFILE2` pipe mode (little-endian), 16-byte pipe header.

## Confirmed facts

1. **The parser panics at record index 3,423,125** with `InvalidPerfEventSize`. `perf script` also fails on this file. _(Verified via `count_event_records.rs` and instrumented library log.)_

2. **A `PERF_RECORD_COMPRESSED2` record at offset `0x8b14e410` has `size=0`.** Raw bytes: `type=83, misc=0, size=0`. The next 8 bytes (`data_size`) are `0xffef` = 65,519. _(Verified by raw hex dump and independent library instrumentation matching the same offset.)_

3. **All 417,241 valid COMPRESSED2 records satisfy `size == PERF_ALIGN(16 + data_size, 8)`.** The bad record is the only one that breaks this invariant. _(Verified by full-file scan.)_

4. **Kernel source (`tools/perf/builtin-record.c:672`) assigns `PERF_ALIGN(compressed, sizeof(u64))` to `header.size` (`__u16`).** The cap `max_record_size = 65536 - 16 - 1 = 65519` allows `compressed` up to 65,535. `PERF_ALIGN(65535, 8) = 65536`, which truncates to 0 in `__u16`. _(Verified from local kernel source.)_

5. **`PERF_RECORD_COMPRESSED2` was introduced in kernel v6.16** (commit `208c0e168344`). Kernel 6.5 (Ubuntu 22.04) only has `PERF_RECORD_COMPRESSED` (type 81), which doesn't hit this overflow. _(Verified via `git log`/`git show` on kernel tags.)_

6. **No valid perf record header exists at `offset + 65536`** (the expected next record if `data_size` is correct). The bytes there are garbage. _(Verified by hex dump.)_

7. **A workaround exists in `linux-perf-data/src/file_reader.rs:478-516`** that detects `size==0 && type==COMPRESSED2`, reads `data_size`, reconstructs the record body, and passes it to decompression. _(Code exists but has not been validated against the test file — see hypotheses.)_

## Hypotheses (unvalidated)

1. **The workaround actually recovers the file.** The code reads `data_size` bytes of compressed payload and decompresses. But fact #6 shows garbage at the expected next-record offset. Either:
   - The compressed payload decompresses fine but the _following_ record is corrupted too (pipe-mode stream corruption beyond this point), or
   - `data_size=65519` is itself wrong and the read overshoots/undershoots, desynchronizing the stream.
   - **Status: untested.** Need to run the workaround on the test file.

2. **Only `size=0` overflows occur.** Any `compressed` in [65529, 65534] would produce a small nonzero `size` (8–48) that passes `size >= 8` but points to wrong data. We haven't scanned for such records.

## Proposed fixes

**Kernel:** cap `max_record_size` so `PERF_ALIGN(16 + max_record_size, 8) <= 65535`, or add an overflow check after alignment.

**Parser workaround:** detect `size==0` (or `size < 16`) on COMPRESSED2, recover from `data_size`. Already implemented, needs validation.

## Code references

- Parser panic: `src/executor/wall_time/perf/parse_perf_file.rs:69-70`
- Library workaround: `linux-perf-data/src/file_reader.rs:478-516`
- Kernel bug: `tools/perf/builtin-record.c:672` in `~/projects/linux`
- Diagnostics: `src/bin/diagnose_perf_file.rs`, `src/bin/count_event_records.rs`
