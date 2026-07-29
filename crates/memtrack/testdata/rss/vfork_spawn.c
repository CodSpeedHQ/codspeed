#define _GNU_SOURCE
#include <fcntl.h>
#include <sched.h>
#include <signal.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#include "file_region.h"

/* Exercise ownership of an mm shared through CLONE_VM.
 *
 * The fixture pauses before later parent faults can refresh the binding, then
 * reclaims the parent's file pages from another process. Ownership must remain
 * with the parent throughout.
 *
 * argv: [1] = checkpoint-ready file to create, [2] = release file to wait for. */

#define REGION (64UL * 1024 * 1024)
#define SCRATCH (1UL * 1024 * 1024)
#define CHILD_STACK (256UL * 1024)

static char* scratch;

/* Fault fresh anonymous pages into the shared address space from the child. */
static int clone_vm_child(void* arg) {
    (void)arg;
    memset(scratch, 0x42, SCRATCH);
    /* POSIX guarantees /bin/sh; only the exec transition matters. */
    execl("/bin/sh", "sh", "-c", "exit 0", (char*)NULL);
    _exit(127);
}

int main(int argc, char** argv) {
    if (argc < 3) return 2;
    sleep(1); /* let the tracker attach + enable + add root pid */

    void* mem = map_and_fault_file(argv[1], REGION);
    if (!mem) return 1;

    /* Leave the mapping untouched so only the CLONE_VM child faults its pages. */
    scratch = mmap(NULL, SCRATCH, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (scratch == MAP_FAILED) return 1;

    char* stack = mmap(NULL, CHILD_STACK, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS | MAP_STACK, -1, 0);
    if (stack == MAP_FAILED) return 1;

    /* glibc's posix_spawn uses CLONE_VM|CLONE_VFORK. CLONE_VFORK orders the
     * child's exec before the later foreign reclaim. */
    pid_t c = clone(clone_vm_child, stack + CHILD_STACK, CLONE_VM | CLONE_VFORK | SIGCHLD, NULL);
    if (c < 0) return 1;
    int status;
    if (waitpid(c, &status, 0) < 0) return 1;
    /* A failed exec would skip the ownership transition under test. */
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) return 1;

    /* Pause before a parent-side fault can refresh the ownership binding. Use
     * only already-faulted memory until the test releases the fixture. */
    int rfd = creat(argv[1], 0644);
    if (rfd < 0) return 1;
    close(rfd);
    /* Bounded: a test that dies before writing the release file must not leave
     * an orphan spinning here while holding the 64 MiB mapping. */
    for (int i = 0; access(argv[2], F_OK) != 0; i++) {
        if (i > 1500) return 3;
        usleep(20000);
    }

    if (reclaim_from_child(mem, REGION) != 0) return 1;
    sleep(1); /* let the external decrements flush to the ring buffer */
    /* No munmap: an in-context decrement would mask the external-path signal. */
    return 0;
}
