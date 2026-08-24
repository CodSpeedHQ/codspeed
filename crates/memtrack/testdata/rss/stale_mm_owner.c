/* Recycled mm_struct, stale rss_stat owner.
 *
 * A child faults a page (registering mm0's hash as owned by that pid), execs into
 * "big" (freeing mm0 while the pid lives on), and idles holding REGION_MIB of anon
 * RSS on mm1. Each churn fork then allocates an mm_struct from the same slab and
 * populates it from the PARENT's context before tearing it down from the child's,
 * so both updates are out-of-context. mm0's slot sits at the head of the per-cpu
 * freelist, so the whole burst hashes to mm0's id.
 */
#define _GNU_SOURCE
#include <sched.h>
#include <signal.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#include "rss_report.h"

#define REGION_MIB 256
#define CHURN_FORKS 256

/* Keeps the mm_struct freed at exec at the head of this CPU's slab freelist, which
 * is LIFO only per cpu. */
static int pin_one_cpu(void) {
    cpu_set_t one;
    CPU_ZERO(&one);
    CPU_SET(sched_getcpu(), &one);
    return sched_setaffinity(0, sizeof(one), &one) != 0;
}

static int run_big(const char* report_path) {
    size_t len = (size_t)REGION_MIB * 1024 * 1024;
    void* region = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (region == MAP_FAILED) return 1;
    memset(region, 0x42, len);
    if (write_rss_report(report_path) != 0) return 1;
    /* Idle while the parent churns: no further fault means no in-context
     * rss_stat event repairs a bogus sample. */
    pause();
    return 0;
}

int main(int argc, char** argv) {
    if (argc < 2) return 1;
    if (argc >= 3 && strcmp(argv[2], "big") == 0) return run_big(argv[1]);

    /* Inherited across both the fork and the exec below. */
    if (pin_one_cpu() != 0) return 1;

    pid_t big = fork();
    if (big < 0) return 1;
    if (big == 0) {
        void* page = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (page == MAP_FAILED) _exit(1);
        memset(page, 1, 4096);
        char* args[] = {argv[0], argv[1], "big", NULL};
        execv("/proc/self/exe", args);
        _exit(1);
    }

    /* run_big writes the report right after its memset, so its existence
     * means the region is resident. */
    for (int i = 0; access(argv[1], F_OK) != 0; i++) {
        if (i > 5000) return 1;
        usleep(1000);
    }

    for (int i = 0; i < CHURN_FORKS; i++) {
        pid_t churn = fork();
        if (churn < 0) return 1;
        if (churn == 0) _exit(0);
        int churn_status;
        if (waitpid(churn, &churn_status, 0) < 0) return 1;
    }

    if (kill(big, SIGKILL) != 0) return 1;
    int status;
    return waitpid(big, &status, 0) < 0;
}
