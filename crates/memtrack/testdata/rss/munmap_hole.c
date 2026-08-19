#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include "rss_report.h"

/* Fault 64 MiB, punch a 16 MiB hole with munmap, then map and fault the hole
 * again. If the partial-unmap removes were missed (or covered the wrong
 * range), the reconstructed running total peaks at 80 MiB instead of 64 MiB.
 * MADV_NOHUGEPAGE keeps every folio a single page: the region is only
 * page-aligned, so a straddling PMD folio would blur the hole boundaries. */
int main(int argc, char** argv) {
    if (argc != 2) return 1;
    sleep(1);
    const size_t len = 64UL * 1024 * 1024;
    const size_t hole_off = 24UL * 1024 * 1024;
    const size_t hole_len = 16UL * 1024 * 1024;

    char* mem = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (mem == MAP_FAILED) return 1;
    if (madvise(mem, len, MADV_NOHUGEPAGE) != 0) return 1;
    memset(mem, 0x42, len);

    if (munmap(mem + hole_off, hole_len) != 0) return 1;

    void* refill = mmap(mem + hole_off, hole_len, PROT_READ | PROT_WRITE,
                        MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
    if (refill == MAP_FAILED) return 1;
    if (madvise(refill, hole_len, MADV_NOHUGEPAGE) != 0) return 1;
    memset(refill, 0x43, hole_len);

    int ret = write_rss_report(argv[1]);
    FILE* report = fopen(argv[1], "a");
    if (!report) return 1;
    fprintf(report, "Layout: 0x%lx 0x%lx 0x%lx 0x%lx\n", (unsigned long)mem, hole_off, hole_len,
            len);
    fclose(report);
    return ret;
}
