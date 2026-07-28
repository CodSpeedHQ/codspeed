#include <stddef.h>

/* Self-contained bump allocator so mi_malloc does not delegate to libc malloc
 * (which is probed separately and would double-count). Classifies as Mimalloc
 * via the mi_malloc/mi_free symbols. */
static char arena[1 << 20];
static size_t off = 0;

void* mi_malloc(size_t size) {
    size_t aligned = (size + 15) & ~((size_t)15);
    if (off + aligned > sizeof(arena)) {
        return 0;
    }
    void* p = &arena[off];
    off += aligned;
    return p;
}

void mi_free(void* p) {
    (void)p;
}
