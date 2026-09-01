#include <stdlib.h>
#include <unistd.h>

/*
 * Each level allocates twice as much as its caller, then frees on the way back
 * up, so the free order is the reverse of the allocation order:
 *
 *   level1  malloc(1024) --> level2  malloc(2048) --> level3  malloc(4096)
 *          free(1024)   <--        free(2048)    <--        free(4096)
 *
 * Every malloc and every free sits at a distinct call depth, so the six events
 * also carry six distinct allocation stacks.
 */

static volatile void* escaped_pointer;

__attribute__((noinline)) static void level3(void) {
    void* p = malloc(4096);
    sleep(1);
    escaped_pointer = p;
    free(p);
}

__attribute__((noinline)) static void level2(void) {
    void* p = malloc(2048);
    escaped_pointer = p;
    sleep(1);
    level3();
    free(p);
}

__attribute__((noinline)) static void level1(void) {
    void* p = malloc(1024);
    escaped_pointer = p;
    sleep(1);
    level2();
    free(p);
}

int main() {
    sleep(1);
    level1();
    return 0;
}
