#include <dlfcn.h>
#include <pthread.h>
#include <stddef.h>
#include <stdio.h>

/* Two threads each dlopen a distinct allocator lib concurrently and allocate
 * through it. Exercises concurrent stop-the-world: both threads trigger the
 * watcher, both must be stopped, both libs classified/attached, both resumed. */
struct job {
    const char* path;
    const char* alloc_sym;
    const char* free_sym;
    size_t size;
};

static void* run(void* arg) {
    struct job* j = (struct job*)arg;
    void* handle = dlopen(j->path, RTLD_NOW);
    if (!handle) {
        return NULL;
    }

    void* (*alloc)(size_t) = (void* (*)(size_t))dlsym(handle, j->alloc_sym);
    void (*dealloc)(void*) = (void (*)(void*))dlsym(handle, j->free_sym);
    if (!alloc || !dealloc) {
        return NULL;
    }

    for (int i = 0; i < 100; i++) {
        void* p = alloc(j->size);
        dealloc(p);
    }

    return NULL;
}

int main(int argc, char** argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: %s <mimalloc-lib> <jemalloc-lib>\n", argv[0]);
        return 1;
    }

    struct job j1 = {argv[1], "mi_malloc", "mi_free", 4242};
    struct job j2 = {argv[2], "je_malloc", "je_free", 4243};

    pthread_t t1, t2;
    pthread_create(&t1, NULL, run, &j1);
    pthread_create(&t2, NULL, run, &j2);
    pthread_join(t1, NULL);
    pthread_join(t2, NULL);

    return 0;
}
