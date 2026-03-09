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

6. **The overflow always produces `size=0`, never a small nonzero value.** `output.pos` is capped at `max_record_size` = 65,519, so `compressed` ∈ [16, 65535]. `PERF_ALIGN` maps [65529, 65535] → 65536, and everything ≤ 65528 stays ≤ 65528. `(u16)65536 = 0` is the only possible overflow. _(Verified by C test program exercising all values.)_

7. **The padding write fails silently — no garbage is injected.** Line 673: `padding = header.size - compressed` → `0 - 65535` underflows to `~0`. `record__write` calls `writen(fd, &pad, ~0)` which fails (EFAULT from writing past valid memory). `record__pushfn` returns 1 (via `||`), but `perf_mmap__push` only checks `< 0`, so perf continues normally. The first `record__write(bf, compressed)` wrote exactly 65,535 bytes (header + data_size + compressed payload). _(Verified by code tracing through `record__pushfn` → `perf_mmap__push`.)_

8. **The next valid record is at `offset + 65535` (not `offset + 65536`).** At `0x8b15e40f` = `0x8b14e410 + 65535`: a valid COMPRESSED2 (size=33168), followed by FINISHED_ROUND (size=8), followed by 20+ consecutive valid records. At `0x8b15e410` = `offset + 65536`: garbage (the first byte of that valid record is misaligned by 1). _(Verified by scanning the file and walking the record chain.)_

9. **The on-disk record is NOT 8-byte aligned.** Since the padding write failed, the actual bytes written were `compressed` = 65,535 (not `PERF_ALIGN(65535, 8)` = 65,536). The 1-byte difference means the workaround must skip `8 + data_size` bytes after the header (= 65,527), NOT `aligned_size` (= 65,528). _(Direct consequence of facts #7 and #8.)_

10. **A workaround exists in `linux-perf-data/src/file_reader.rs:478-516`** but it has a bug: it reads `padding` = `aligned_size - total_size` = 1 extra byte, consuming the first byte of the next valid record and desynchronizing the stream. _(Code review: lines 493-506 compute and read alignment padding that was never written to disk.)_

## Hypotheses (unvalidated)

1. **Fixing the workaround to skip 0 padding bytes will recover the file.** The compressed payload (65,519 bytes at `data_size`) should decompress successfully, and after skipping exactly `8 + 8 + data_size` bytes from the header start (no alignment padding), the stream should resynchronize. **Status: untested.**

## Proposed fixes

**Kernel:** cap `max_record_size` so `PERF_ALIGN(16 + max_record_size, 8) <= 65535`, or add an overflow check after alignment.

**Parser workaround:** detect `size==0` (or `size < 16`) on COMPRESSED2, recover from `data_size`. Already implemented, needs validation.

## Code references

- Parser panic: `src/executor/wall_time/perf/parse_perf_file.rs:69-70`
- Library workaround: `linux-perf-data/src/file_reader.rs:478-516`
- Kernel bug: `tools/perf/builtin-record.c:672` in `~/projects/linux`
- Diagnostics: `src/bin/diagnose_perf_file.rs`, `src/bin/count_event_records.rs`
