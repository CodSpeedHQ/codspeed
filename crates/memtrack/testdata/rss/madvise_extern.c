#define _GNU_SOURCE
#include <unistd.h>

#include "file_region.h"

int main(int argc, char** argv) {
    if (argc != 2) return 1;
    sleep(1); /* let the tracker attach + enable + add root pid */

    size_t len = 64UL * 1024 * 1024;
    void* mem = map_and_fault_file(argv[1], len);
    if (!mem) return 1;

    if (reclaim_from_child(mem, len) != 0) return 1;

    sleep(1); /* let the external decrement flush to the ring buffer */
    /* No munmap: an in-context decrement would mask the external-path signal. */
    return 0;
}
