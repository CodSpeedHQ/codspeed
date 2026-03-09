#!/bin/bash
# Minimal reproducer for the PERF_RECORD_COMPRESSED2 size=0 kernel bug.
# Tries increasing mmap sizes to find one that reliably produces a chunk
# whose zstd output lands in [65529, 65535] bytes -> header.size wraps to 0.

set -euo pipefail

PERF=${PERF:-perf}
OUTFILE=/tmp/repro.pipedata
DURATION=5  # seconds per attempt

echo "perf version: $($PERF --version)"
echo "kernel: $(uname -r)"
echo ""

# The workload: a tight loop that generates many samples with deep stack traces.
# Use multiple threads and a high sample rate to maximize data per chunk.
NCPUS=$(nproc)
WORKLOAD="python3 -c \"
import threading, time
def fib(n):
    return n if n < 2 else fib(n-1) + fib(n-2)
def loop():
    end = time.time() + $DURATION
    while time.time() < end:
        fib(28)
threads = [threading.Thread(target=loop) for _ in range($NCPUS)]
[t.start() for t in threads]
[t.join() for t in threads]
\""

# Try different -m values (mmap pages, must be power of 2).
# Larger mmap = larger chunks fed to zstd = higher compressed output size.
for MMAP_PAGES in 128 256 512 1024 2048; do
    echo "=== Trying -m $MMAP_PAGES (chunk size ~$((MMAP_PAGES * 4))KB) ==="

    rm -f "$OUTFILE"
    $PERF record \
        --compression-level=3 \
        -m "$MMAP_PAGES" \
        --freq=9997 \
        -g --call-graph=dwarf,65528 \
        -k CLOCK_MONOTONIC \
        -o - \
        -- bash -c "$WORKLOAD" 2>/dev/null | cat > "$OUTFILE" || true

    if [ ! -f "$OUTFILE" ]; then
        echo "  No output file produced, skipping"
        continue
    fi

    SIZE=$(wc -c < "$OUTFILE")
    echo "  Output file size: $SIZE bytes"

    # Run our diagnostic tool to check for the bug
    RESULT=$(cargo run --release --bin diagnose_perf_file -- "$OUTFILE" 2>/dev/null || true)
    if echo "$RESULT" | grep -q "CORRUPTION FOUND"; then
        echo "  *** BUG TRIGGERED! size=0 record found ***"
        echo "$RESULT" | grep -E "CORRUPTION|type=83|size=0"
        echo ""
        echo "Reproducer: $PERF record --compression-level=3 -m $MMAP_PAGES --freq=9997 -g --call-graph=dwarf -k CLOCK_MONOTONIC -o - -- <workload>"
        exit 0
    else
        echo "  No bug triggered"
    fi
done

echo ""
echo "Bug not triggered with any mmap size. Try a longer duration or different workload."
exit 1
