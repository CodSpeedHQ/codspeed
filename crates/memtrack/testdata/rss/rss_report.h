#ifndef RSS_REPORT_H
#define RSS_REPORT_H

#include <stdio.h>
#include <string.h>
#include <unistd.h>

static long rss_status_kb_pid(int pid, const char* key) {
    char status_path[64];
    snprintf(status_path, sizeof(status_path), "/proc/%d/status", pid);
    FILE* status = fopen(status_path, "r");
    if (!status) return -1;
    char line[256];
    long kb = -1;
    size_t key_len = strlen(key);
    while (fgets(line, sizeof(line), status)) {
        if (strncmp(line, key, key_len) == 0 && sscanf(line + key_len, " %ld", &kb) == 1) {
            break;
        }
    }
    fclose(status);
    return kb;
}

static long rss_status_kb(const char* key) {
    return rss_status_kb_pid(getpid(), key);
}

/* Reads Rss* through /proc/<pid>/status of the given task. For the
 * thread-group dir this is the leader's task: once the leader is a zombie its
 * mm pointer is gone and the Rss lines disappear, so callers whose leader has
 * exited must pass a live thread's tid instead. */
static int write_rss_report_pid(int pid, const char* path) {
    long anon = rss_status_kb_pid(pid, "RssAnon:");
    long file = rss_status_kb_pid(pid, "RssFile:");
    long shmem = rss_status_kb_pid(pid, "RssShmem:");
    /* VmHWM instead of getrusage(): ru_maxrss includes signal->maxrss, which
     * survives execve and so reports the peak of the pre-exec parent image.
     * VmHWM belongs to the mm and starts fresh at exec. */
    long max_rss = rss_status_kb_pid(pid, "VmHWM:");
    if (anon < 0 || file < 0 || shmem < 0 || max_rss < 0) {
        return 1;
    }
    FILE* report = fopen(path, "w");
    if (!report) {
        return 1;
    }
    fprintf(report, "RssAnon: %ld\nRssFile: %ld\nRssShmem: %ld\nMaxRssKb: %ld\n", anon, file,
            shmem, max_rss);
    fclose(report);
    return 0;
}

static int write_rss_report(const char* path) {
    return write_rss_report_pid(getpid(), path);
}

#endif
