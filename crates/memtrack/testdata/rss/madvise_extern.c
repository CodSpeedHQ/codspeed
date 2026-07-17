#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <sys/wait.h>
#include <unistd.h>

int main(int argc, char** argv) {
    if (argc != 2) return 1;
    sleep(1); /* let the tracker attach + enable + add root pid */

    size_t len = 64UL * 1024 * 1024;

    /* Keep the data file next to the report: /tmp may be tmpfs (Ubuntu >= 25.04),
       which accounts mapped file pages as shmem instead of file. */
    char path[4096];
    snprintf(path, sizeof(path), "%s.data-XXXXXX", argv[1]);
    int fd = mkstemp(path);
    if (fd < 0) return 1;
    unlink(path);
    if (ftruncate(fd, len) != 0) return 1;

    void* mem = mmap(NULL, len, PROT_READ, MAP_PRIVATE, fd, 0);
    if (mem == MAP_FAILED) return 1;

    /* Fault the file pages into A's RSS (MM_FILEPAGES), in-context -> seeds ownership. */
    volatile char sink = 0;
    for (size_t i = 0; i < len; i += 4096) sink ^= ((volatile char*)mem)[i];
    (void)sink;

    pid_t pid = fork();
    if (pid < 0) return 1;
    if (pid == 0) {
        /* B: page out A's (parent's) region from B's context. */
        int pidfd = syscall(SYS_pidfd_open, getppid(), 0);
        if (pidfd < 0) _exit(1);
        struct iovec iov = {.iov_base = mem, .iov_len = len};
        syscall(SYS_process_madvise, pidfd, &iov, 1UL, MADV_PAGEOUT, 0UL);
        _exit(0);
    }

    int status;
    if (waitpid(pid, &status, 0) < 0) return 1;
    sleep(1); /* let the external decrement flush to the ring buffer */
    /* No munmap: an in-context decrement would mask the external-path signal. */
    return 0;
}
