#include <stdlib.h>
#include <unistd.h>

/*
 * posix_memalign has an inverted ABI vs memalign/aligned_alloc: it returns
 * int 0 on SUCCESS and delivers the pointer through the memptr out-parameter.
 * The memalign uretprobe skips any call returning 0 (a NULL malloc-style
 * return = failure), so successful posix_memalign calls must not be dropped.
 */
int main() {
    sleep(1);
    void* p = NULL;
    posix_memalign(&p, 128, 768);
    free(p);
    return 0;
}
