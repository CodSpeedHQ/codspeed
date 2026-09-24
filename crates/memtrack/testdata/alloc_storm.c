// Saturates the allocator uprobes from several threads at once so the event
// ring fills faster than a slow poller can drain it.
//
// usage: alloc_storm <threads> <iterations-per-thread>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>

static long iterations;

static void* storm(void* arg) {
    (void)arg;
    for (long i = 0; i < iterations; i++) {
        volatile char* p = malloc(16);
        p[0] = (char)i;
        free((void*)p);
    }
    return NULL;
}

int main(int argc, char** argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: %s <threads> <iterations>\n", argv[0]);
        return 2;
    }
    int threads = atoi(argv[1]);
    iterations = atol(argv[2]);

    pthread_t* handles = calloc((size_t)threads, sizeof(*handles));
    for (int i = 0; i < threads; i++) {
        if (pthread_create(&handles[i], NULL, storm, NULL) != 0) {
            perror("pthread_create");
            return 1;
        }
    }
    for (int i = 0; i < threads; i++) {
        pthread_join(handles[i], NULL);
    }
    free(handles);
    return 0;
}
