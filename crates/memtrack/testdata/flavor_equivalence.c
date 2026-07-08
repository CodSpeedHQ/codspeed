/* Deterministic allocation workload for the perf-vs-token flavor equivalence
 * test. Brackets a fixed sequence of allocations with 0xC0D59EED marker
 * mallocs so the test can isolate exactly these events from libc/runtime noise
 * (see assert_events_with_marker! in tests/shared.rs).
 *
 * The leading sleep gives the tracker time to attach its probes and register
 * this PID before the marked allocations run. The fork exercises the
 * fork-follow hook (tp_btf in the token flavor, a classic tracepoint in the
 * perf flavor), which must behave identically. */
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

static void marker(void) {
    void* m = malloc(0xC0D59EED);
    free(m);
}

int main(void) {
    sleep(1);

    marker();

    /* A fixed, varied set of allocations the test asserts on. */
    void* a = malloc(4096);
    memset(a, 1, 4096);
    void* c = calloc(16, 256);
    void* r = malloc(1024);
    r = realloc(r, 8192);
    void* al = aligned_alloc(64, 2048);

    /* Fork a child that allocates and frees; the parent must inherit-track it. */
    pid_t pid = fork();
    if (pid == 0) {
        void* child = malloc(333);
        free(child);
        _exit(0);
    }
    waitpid(pid, NULL, 0);

    free(a);
    free(c);
    free(r);
    free(al);

    marker();

    return 0;
}
