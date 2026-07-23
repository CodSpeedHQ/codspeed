#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>

int main(void) {
    void *p = malloc(1 << 20);
    memset(p, 1, 1 << 20);
    free(p);

    void *m = mmap(NULL, 4 << 20, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    memset(m, 1, 4 << 20);
    munmap(m, 4 << 20);
    return 0;
}
