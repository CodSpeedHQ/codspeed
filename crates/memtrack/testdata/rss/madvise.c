#include <stdint.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include "rss_report.h"

/* AnonHugePages for the whole process, from smaps_rollup (not in /proc/status). */
static long anon_huge_pages_kb(void) {
    FILE* rollup = fopen("/proc/self/smaps_rollup", "r");
    if (!rollup) return 0;
    char line[256];
    long kb = 0;
    while (fgets(line, sizeof(line), rollup)) {
        if (sscanf(line, "AnonHugePages: %ld", &kb) == 1) break;
    }
    fclose(rollup);
    return kb;
}

/* Two 32 MiB anon regions: one advised MADV_HUGEPAGE (2 MiB-aligned so the
 * kernel *may* fault it as PMD folios), one MADV_NOHUGEPAGE (guaranteed pte
 * path). MADV_HUGEPAGE is only advisory, so nothing here depends on THP
 * actually materializing: the accounted totals are identical either way.
 *
 * Both regions are dropped with MADV_DONTNEED and faulted a second time. If
 * the in-context removes were missed, the reconstructed running total reaches
 * 128 MiB instead of 64 MiB, so the snapshot peak is the assertion. The report
 * also carries the observed AnonHugePages (hex, so it stays out of the
 * snapshot) letting the test require PMD-sized rmap deltas exactly when THP
 * actually materialized. */
int main(int argc, char** argv) {
    if (argc != 2) return 1;
    sleep(1);
    const size_t len = 32UL * 1024 * 1024;
    const size_t align = 2UL * 1024 * 1024;

    void* raw = mmap(NULL, len + align, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (raw == MAP_FAILED) return 1;
    char* huge = (char*)(((uintptr_t)raw + align - 1) & ~(uintptr_t)(align - 1));
    if (madvise(huge, len, MADV_HUGEPAGE) != 0) return 1;

    char* base = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (base == MAP_FAILED) return 1;
    if (madvise(base, len, MADV_NOHUGEPAGE) != 0) return 1;

    memset(huge, 0x42, len);
    memset(base, 0x42, len);

    if (madvise(huge, len, MADV_DONTNEED) != 0) return 1;
    if (madvise(base, len, MADV_DONTNEED) != 0) return 1;

    memset(huge, 0x43, len);
    memset(base, 0x43, len);

    int ret = write_rss_report(argv[1]);
    FILE* report = fopen(argv[1], "a");
    if (!report) return 1;
    fprintf(report, "ThpKb: 0x%lx\n", anon_huge_pages_kb());
    fclose(report);
    munmap(raw, len + align);
    munmap(base, len);
    return ret;
}
