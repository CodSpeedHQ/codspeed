#include <dlfcn.h>
#include <stdio.h>
#include <stddef.h>

/* dlopen an allocator lib and IMMEDIATELY (no sleep) allocate through it. The
 * immediacy is the race assertion: every allocation must be captured, which
 * requires the on-demand watcher to have stopped-classified-attached-resumed
 * before dlopen returns. */
int main(int argc, char** argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s <lib>\n", argv[0]);
        return 1;
    }

    void* handle = dlopen(argv[1], RTLD_NOW);
    if (!handle) {
        fprintf(stderr, "dlopen failed: %s\n", dlerror());
        return 1;
    }

    void* (*mi_malloc)(size_t) = (void* (*)(size_t))dlsym(handle, "mi_malloc");
    void (*mi_free)(void*) = (void (*)(void*))dlsym(handle, "mi_free");
    if (!mi_malloc || !mi_free) {
        fprintf(stderr, "dlsym failed\n");
        return 1;
    }

    for (int i = 0; i < 100; i++) {
        void* p = mi_malloc(4242);
        mi_free(p);
    }

    return 0;
}
