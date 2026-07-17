#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include "rss_report.h"

/* Two-phase fixture that faults anonymous memory before and after a handshake:
 *
 *   1. Fault a baseline region, then signal `ready` and block on `go`.
 *   2. After `go` appears, fault a second region of the same (anon) member.
 *
 * The handshake lets the caller act between the two phases (e.g. start
 * observing only phase 2). Both regions touch MM_ANONPAGES, so an absolute
 * counter read after phase 2 covers baseline + growth, while a delta observed
 * only from phase 2 covers growth alone.
 *
 * argv: [1]=report path, [2]=ready path, [3]=go path. */
static void touch(const char* path) {
    FILE* f = fopen(path, "w");
    if (f) {
        fclose(f);
    }
}

int main(int argc, char** argv) {
    if (argc != 4) {
        return 1;
    }
    const char* report_path = argv[1];
    const char* ready_path = argv[2];
    const char* go_path = argv[3];

    size_t region = 64UL * 1024 * 1024;

    void* baseline = mmap(NULL, region, PROT_READ | PROT_WRITE,
                          MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (baseline == MAP_FAILED) {
        return 1;
    }
    memset(baseline, 0x42, region);

    touch(ready_path);
    while (access(go_path, F_OK) != 0) {
        usleep(1000);
    }

    void* growth = mmap(NULL, region, PROT_READ | PROT_WRITE,
                        MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (growth == MAP_FAILED) {
        return 1;
    }
    memset(growth, 0x42, region);

    int ret = write_rss_report(report_path);
    munmap(baseline, region);
    munmap(growth, region);
    return ret;
}
