#include <stdlib.h>
#include <unistd.h>

/*
 * posix_memalign returns nonzero (EINVAL) when the alignment is not a power of
 * two multiple of sizeof(void*), leaving *memptr untouched. The uretprobe must
 * drop such calls (ret != 0). The trailing malloc proves tracking is still live,
 * so an empty AlignedAlloc means "dropped", not "never attached".
 */
int main() {
    sleep(1);
    void* p = NULL;
    posix_memalign(&p, 3, 768);
    void* q = malloc(512);
    free(q);
    return 0;
}
