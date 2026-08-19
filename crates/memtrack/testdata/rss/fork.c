#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#include "rss_report.h"

static int append_rss(const char* path, const char* label) {
    long kb = rss_status_kb("RssAnon:");
    if (kb < 0) return 1;
    FILE* report = fopen(path, "a");
    if (!report) return 1;
    fprintf(report, "%s: %ld\n", label, kb);
    fclose(report);
    return 0;
}

int main(int argc, char** argv) {
    if (argc != 2) return 1;
    sleep(1);
    size_t len = 0x20 * 1024 * 1024;
    void* parent = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (parent == MAP_FAILED) return 1;
    memset(parent, 0x42, len);
    pid_t pid = fork();
    if (pid < 0) return 1;
    if (pid == 0) {
        void* child = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (child == MAP_FAILED) _exit(1);
        memset(child, 0x24, len);
        _exit(append_rss(argv[1], "ChildRssAnonKb"));
    }
    int status;
    if (waitpid(pid, &status, 0) < 0) return 1;
    int ret = append_rss(argv[1], "ParentRssAnonKb");
    munmap(parent, len);
    return ret || !WIFEXITED(status) || WEXITSTATUS(status) != 0;
}
