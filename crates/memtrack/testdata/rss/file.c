#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include "rss_report.h"

int main(int argc, char** argv) {
    if (argc != 2) return 1;
    sleep(1);
    size_t len = 64UL * 1024 * 1024;
    /* Keep the data file next to the report: /tmp may be tmpfs (Ubuntu >= 25.04),
       which accounts mapped file pages as shmem instead of file. */
    char template[4096];
    snprintf(template, sizeof(template), "%s.data-XXXXXX", argv[1]);
    int fd = mkstemp(template);
    if (fd < 0) return 1;
    unlink(template);
    char chunk[65536];
    memset(chunk, 0x42, sizeof(chunk));
    for (size_t off = 0; off < len; off += sizeof(chunk)) {
        if (write(fd, chunk, sizeof(chunk)) != (ssize_t)sizeof(chunk)) return 1;
    }
    void* mem = mmap(NULL, len, PROT_READ, MAP_PRIVATE, fd, 0);
    if (mem == MAP_FAILED) return 1;
    volatile char sink = 0;
    for (size_t i = 0; i < len; i += 4096) sink ^= ((volatile char*)mem)[i];
    int ret = write_rss_report(argv[1]);
    munmap(mem, len);
    close(fd);
    return ret + (sink & 0);
}
