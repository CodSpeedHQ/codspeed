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

static int write_rss_report(const char* path) {
    long anon = rss_status_kb("RssAnon:");
    long file = rss_status_kb("RssFile:");
    long shmem = rss_status_kb("RssShmem:");
    /* VmHWM instead of getrusage(): ru_maxrss includes signal->maxrss, which
     * survives execve and so reports the peak of the pre-exec parent image.
     * VmHWM belongs to the mm and starts fresh at exec. */
    long max_rss = rss_status_kb("VmHWM:");
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

#endif
