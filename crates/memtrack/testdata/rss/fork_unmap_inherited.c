#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

int main(void) {
    sleep(1);
    size_t len = 0x20 * 1024 * 1024;
    void* region = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (region == MAP_FAILED) return 1;
    memset(region, 0x42, len);
    pid_t pid = fork();
    if (pid < 0) return 1;
    if (pid == 0) {
        /* Unmap the inherited region without ever touching it: fork copies
         * page tables through the folio_*_dup_* helpers, which have no rmap
         * hooks, so the removes fired here have no matching adds under the
         * child's pid. */
        munmap(region, len);
        _exit(0);
    }
    int status;
    if (waitpid(pid, &status, 0) < 0) return 1;
    munmap(region, len);
    return !WIFEXITED(status) || WEXITSTATUS(status) != 0;
}
