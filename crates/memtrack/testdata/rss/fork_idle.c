#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#include "rss_report.h"

/* The child only touches memory inherited from the parent: COW faults of anon
 * pages are counter-neutral, so the child never reports its own RSS. Its
 * footprint is only observable through fork-event seeding. The child must not
 * allocate (no stdio/malloc), so the parent samples /proc/<child>/status while
 * the child blocks on a pipe. */
int main(int argc, char** argv) {
    if (argc != 2) return 1;
    sleep(1);
    size_t len = 0x20 * 1024 * 1024;
    void* mem = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (mem == MAP_FAILED) return 1;
    memset(mem, 0x42, len);

    int ready[2];
    int release[2];
    if (pipe(ready) || pipe(release)) return 1;

    pid_t pid = fork();
    if (pid < 0) return 1;
    if (pid == 0) {
        memset(mem, 0x24, len);
        char b = 1;
        if (write(ready[1], &b, 1) != 1) _exit(1);
        if (read(release[0], &b, 1) != 1) _exit(1);
        _exit(0);
    }

    char b;
    if (read(ready[0], &b, 1) != 1) return 1;
    long child_anon = rss_status_kb_pid(pid, "RssAnon:");
    int ret = write_rss_report(argv[1]);
    FILE* report = fopen(argv[1], "a");
    if (!report) return 1;
    fprintf(report, "ChildRssAnon: %ld\n", child_anon);
    fclose(report);
    if (write(release[1], &b, 1) != 1) return 1;

    int status;
    if (waitpid(pid, &status, 0) < 0) return 1;
    return ret || child_anon < 0 || !WIFEXITED(status) || WEXITSTATUS(status) != 0;
}
