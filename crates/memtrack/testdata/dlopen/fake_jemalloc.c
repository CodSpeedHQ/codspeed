#include <stddef.h>

/* Self-contained bump allocator; classifies as Jemalloc via the je_malloc
 * symbol. See fake_mimalloc.c for why it does not call libc malloc. */
static char arena[1 << 20];
static size_t off = 0;

void* je_malloc(size_t size) {
    size_t aligned = (size + 15) & ~((size_t)15);
    if (off + aligned > sizeof(arena)) {
        return 0;
    }
    void* p = &arena[off];
    off += aligned;
    return p;
}

void je_free(void* p) {
    (void)p;
}
