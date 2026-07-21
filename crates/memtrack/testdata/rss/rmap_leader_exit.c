#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>

#include "rss_report.h"

/* The main thread publishes its tid and leaves via pthread_exit while a worker
 * thread keeps the process alive. The worker waits until the leader is a
 * zombie — its exit path, including the sched_process_exit tracepoint, has
 * fully run — then faults an anon region, writes the report, and exits the
 * whole group. A tracker keyed on leader exit instead of thread-group death
 * would stop watching before the worker's region is faulted.
 *
 * argv: [1]=report path. */

static const char* report_path;
static pid_t leader_tid;

static int leader_is_zombie(void) {
    char path[64];
    char buf[256];
    snprintf(path, sizeof(path), "/proc/self/task/%d/stat", leader_tid);
    FILE* f = fopen(path, "r");
    if (!f) {
        return 1;
    }
    size_t n = fread(buf, 1, sizeof(buf) - 1, f);
    fclose(f);
    buf[n] = '\0';
    /* The state field follows the parenthesized comm. */
    const char* p = strrchr(buf, ')');
    if (!p || p[1] == '\0' || p[2] == '\0') {
        return 0;
    }
    return p[2] == 'Z';
}

static void* worker(void* arg) {
    (void)arg;
    while (!leader_is_zombie()) {
        usleep(1000);
    }

    size_t region = 64UL * 1024 * 1024;
    void* mem =
        mmap(NULL, region, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (mem == MAP_FAILED) {
        _exit(1);
    }
    memset(mem, 0x42, region);

    /* The leader is a zombie by now, so /proc/<pid>/status has no Rss lines;
     * report through this worker's own task instead. */
    int ret = write_rss_report_pid((int)syscall(SYS_gettid), report_path);
    munmap(mem, region);
    _exit(ret);
}

int main(int argc, char** argv) {
    if (argc != 2) {
        return 1;
    }
    report_path = argv[1];
    leader_tid = (pid_t)syscall(SYS_gettid);

    pthread_t thread;
    if (pthread_create(&thread, NULL, worker, NULL) != 0) {
        return 1;
    }
    pthread_exit(NULL);
}
