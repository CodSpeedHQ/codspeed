#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include "rss_report.h"

int main(int argc, char** argv) {
    if (argc != 2) return 1;
    sleep(1);
    const size_t chunk = 16UL * 1024 * 1024;
    void* bufs[4];
    for (int i = 0; i < 4; i++) {
        bufs[i] = mmap(NULL, chunk, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (bufs[i] == MAP_FAILED) return 1;
        memset(bufs[i], 0x42, chunk);
        usleep(50 * 1000);
    }
    int ret = write_rss_report(argv[1]);
    for (int i = 0; i < 4; i++) {
        munmap(bufs[i], chunk);
        usleep(50 * 1000);
    }
    return ret;
}
