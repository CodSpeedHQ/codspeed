#define _GNU_SOURCE
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include "rss_report.h"

int main(int argc, char** argv) {
    if (argc != 2) return 1;
    sleep(1);
    size_t len = 64UL * 1024 * 1024;
    int fd = memfd_create("memtrack-rss-shmem", 0);
    if (fd < 0) return 1;
    if (ftruncate(fd, len) != 0) return 1;
    void* mem = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (mem == MAP_FAILED) return 1;
    memset(mem, 0x42, len);
    int ret = write_rss_report(argv[1]);
    munmap(mem, len);
    close(fd);
    return ret;
}
