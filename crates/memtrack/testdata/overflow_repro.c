#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>

/*
 * Reproduces the memtrack ring buffer overflow (COD-3071).
 *
 * Emits a malloc+free pair as fast as possible, so the event production rate
 * far outruns the userspace consumer and the kernel-side BPF ring buffer fills
 * up. Each iteration produces 2 events, so the default 36M pairs generate ~72M
 * events, matching the node_astro_perf_regression case that overflows in CI.
 *
 * Usage: overflow_repro [pairs]   (default: 36000000)
 */
int main(int argc, char** argv) {
    long pairs = argc > 1 ? atol(argv[1]) : 36000000L;

    /* Give the tracker time to attach its uprobes before we start allocating. */
    sleep(1);

    for (long i = 0; i < pairs; i++) {
        void* p = malloc(64);
        if (!p) {
            return 1;
        }
        /* Touch the allocation so the compiler cannot elide the malloc/free. */
        *(volatile uint8_t*)p = (uint8_t)i;
        free(p);
    }

    return 0;
}
