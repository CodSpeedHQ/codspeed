#define _GNU_SOURCE
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include "rss_report.h"

/* Fault 32 MiB, then force mremap to move it to a reserved destination. The
 * move relocates page tables without rmap remove/add, so no events should
 * fire and the second memset must not refault: a peak of 64 MiB instead of
 * 32 MiB means the move was double-counted or the pages were dropped. */
int main(int argc, char** argv) {
    if (argc != 2) return 1;
    sleep(1);
    const size_t len = 32UL * 1024 * 1024;

    char* src = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (src == MAP_FAILED) return 1;
    memset(src, 0x42, len);

    void* reserved = mmap(NULL, len, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (reserved == MAP_FAILED) return 1;

    char* dst = mremap(src, len, len, MREMAP_MAYMOVE | MREMAP_FIXED, reserved);
    if (dst == MAP_FAILED) return 1;
    memset(dst, 0x43, len);

    int ret = write_rss_report(argv[1]);
    munmap(dst, len);
    return ret;
}
