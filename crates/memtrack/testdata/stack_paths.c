#include <stdio.h>
#include <stdlib.h>

static volatile void *escaped_pointer;
static volatile unsigned int remaining_a = 50;
static volatile unsigned int remaining_b = 50;

__attribute__((noinline)) static void path_a_inner(void) {
    void *pointer = malloc(64);
    escaped_pointer = pointer;
    free(pointer);
}

__attribute__((noinline)) static void path_a(void) {
    while (remaining_a != 0) {
        path_a_inner();
        --remaining_a;
    }
}

__attribute__((noinline)) static void path_b_inner(void) {
    void *pointer = malloc(192);
    escaped_pointer = pointer;
    free(pointer);
}

__attribute__((noinline)) static void path_b(void) {
    while (remaining_b != 0) {
        path_b_inner();
        --remaining_b;
    }
}

int main(void) {
    void *marker_before = malloc(0xC0D59EED);
    escaped_pointer = marker_before;
    free(marker_before);

    path_a();
    path_b();

    void *marker_after = malloc(0xC0D59EED);
    escaped_pointer = marker_after;
    free(marker_after);
    return 0;
}
