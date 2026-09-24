#include <stdlib.h>
#include <string.h>

/*
 * Many allocations from a few call depths, with locals that change between
 * iterations. Consecutive captures on the same thread therefore share most of
 * their stack bytes but never hash equal, and the stack pointer moves both
 * deeper and shallower between captures.
 */

static volatile void* escaped_pointer;

__attribute__((noinline)) static void leaf(int i) {
    volatile char scratch[256];
    memset((char*)scratch, i & 0xff, sizeof(scratch));
    void* p = malloc(16 + (i % 7));
    escaped_pointer = p;
    free(p);
}

__attribute__((noinline)) static void middle(int i) {
    volatile long pad[8];
    pad[i & 7] = i;
    void* p = malloc(64);
    escaped_pointer = p;
    leaf(i);
    free(p);
}

__attribute__((noinline)) static void deep(int i) {
    volatile long pad[32];
    pad[i & 31] = i;
    middle(i);
    void* p = malloc(128);
    escaped_pointer = p;
    free(p);
}

int main() {
    for (int i = 0; i < 2000; i++) {
        if (i % 3 == 0) {
            deep(i);
        } else if (i % 3 == 1) {
            middle(i);
        } else {
            leaf(i);
        }
    }
    return 0;
}
