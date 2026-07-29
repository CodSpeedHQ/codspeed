#ifndef FILE_REGION_H
#define FILE_REGION_H

#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <sys/wait.h>
#include <unistd.h>

/* Maps len bytes of a private file mapping and faults every page in, so the
 * pages land in the caller's RSS (MM_FILEPAGES) in-context and seed ownership.
 * Returns the mapping, or NULL.
 *
 * The data file is derived from base rather than placed in /tmp, which is tmpfs
 * on Ubuntu >= 25.04 and would account mapped file pages as shmem, not file. */
static void* map_and_fault_file(const char* base, size_t len) {
    char path[4096];
    snprintf(path, sizeof(path), "%s.data-XXXXXX", base);
    int fd = mkstemp(path);
    if (fd < 0) return NULL;
    unlink(path);
    if (ftruncate(fd, len) != 0) return NULL;

    void* mem = mmap(NULL, len, PROT_READ, MAP_PRIVATE, fd, 0);
    if (mem == MAP_FAILED) return NULL;

    /* volatile so the reads are not elided. 4096 is the fault stride: a smaller
     * one would fault the same page twice, a larger one would skip pages. */
    volatile char sink = 0;
    for (size_t i = 0; i < len; i += 4096) sink ^= ((volatile char*)mem)[i];
    (void)sink;
    return mem;
}

/* Pages out [mem, mem+len) of the calling process from a forked child, and
 * waits for it. The reclaim must run in another task's context to exercise
 * foreign-actor attribution. Returns 0 on success. */
static int reclaim_from_child(void* mem, size_t len) {
    pid_t pid = fork();
    if (pid < 0) return 1;
    if (pid == 0) {
        int pidfd = syscall(SYS_pidfd_open, getppid(), 0);
        if (pidfd < 0) _exit(1);
        struct iovec iov = {.iov_base = mem, .iov_len = len};
        syscall(SYS_process_madvise, pidfd, &iov, 1UL, MADV_PAGEOUT, 0UL);
        _exit(0);
    }
    int status;
    return waitpid(pid, &status, 0) < 0;
}

#endif
