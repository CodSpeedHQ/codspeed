#include <stdlib.h>
#include <unistd.h>

/*
 * Same doubling allocation chain as nested_doubling.c, but ownership is handed
 * down and the innermost level frees all three buffers in reverse order:
 *
 *   level1 malloc(1024) --> level2 malloc(2048) --> level3 malloc(4096)
 *                                                         free(4096)
 *                                                         free(2048)
 *                                                         free(1024)
 *
 * The three mallocs come from three different call depths while all three frees
 * share one, so a deallocation event must be attributed to the free site rather
 * than to wherever its allocation happened.
 */

static volatile void* escaped_pointer;

__attribute__((noinline)) static void level3(void* outer, void* middle) {
    void* p = malloc(4096);
    escaped_pointer = p;
    free(p);
    free(middle);
    free(outer);
}

__attribute__((noinline)) static void level2(void* outer) {
    void* p = malloc(2048);
    escaped_pointer = p;
    level3(outer, p);
}

__attribute__((noinline)) static void level1(void) {
    void* p = malloc(1024);
    escaped_pointer = p;
    level2(p);
}

int main() {
    sleep(1);
    level1();
    return 0;
}
