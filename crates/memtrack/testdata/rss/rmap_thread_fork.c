#include <pthread.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#include "rss_report.h"

/* A worker thread (not the group leader) calls fork(); the child faults an
 * anon region and writes the report. Child tracking must key on the parent's
 * TGID: a scheme keyed on the raw creator tid would only cover forks issued
 * by the leader. */

static const char* report_path;

static void* worker(void* arg) {
    (void)arg;

    pid_t pid = fork();
    if (pid < 0) {
        _exit(1);
    }
    if (pid == 0) {
        size_t region = 64UL * 1024 * 1024;
        void* mem = mmap(NULL, region, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (mem == MAP_FAILED) {
            _exit(1);
        }
        memset(mem, 0x42, region);

        int ret = write_rss_report(report_path);
        munmap(mem, region);
        _exit(ret);
    }

    int status;
    if (waitpid(pid, &status, 0) < 0 || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        _exit(1);
    }
    return NULL;
}

int main(int argc, char** argv) {
    if (argc != 2) {
        return 1;
    }
    report_path = argv[1];

    pthread_t thread;
    if (pthread_create(&thread, NULL, worker, NULL) != 0) {
        return 1;
    }
    if (pthread_join(thread, NULL) != 0) {
        return 1;
    }
    return 0;
}
