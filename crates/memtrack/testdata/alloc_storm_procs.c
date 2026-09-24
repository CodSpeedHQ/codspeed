// Saturates the allocator uprobes from several processes at once, so the
// event ring fills while many producers write to it concurrently.
//
// usage: alloc_storm_procs <processes> <iterations-per-process>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <unistd.h>

static void storm(long iterations) {
    for (long i = 0; i < iterations; i++) {
        volatile char* p = malloc(16);
        p[0] = (char)i;
        free((void*)p);
    }
}

int main(int argc, char** argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: %s <processes> <iterations>\n", argv[0]);
        return 2;
    }
    int processes = atoi(argv[1]);
    long iterations = atol(argv[2]);

    for (int i = 0; i < processes; i++) {
        pid_t pid = fork();
        if (pid < 0) {
            perror("fork");
            return 1;
        }
        if (pid == 0) {
            storm(iterations);
            _exit(0);
        }
    }

    int failed = 0;
    for (int i = 0; i < processes; i++) {
        int status;
        if (wait(&status) < 0 || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
            failed = 1;
        }
    }
    return failed;
}
