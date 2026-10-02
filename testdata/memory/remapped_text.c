// Maps the executable segment holding `allocate` a second time at another
// address, the way V8 remaps its embedded builtins, and allocates through both
// copies. The process then holds two placements of its own binary, each with a
// different load bias.
//
// Prints the executable mappings of the binary, as /proc/self/maps lines,
// followed by `allocate <original address> <remapped address>`.
#define _GNU_SOURCE
#include <fcntl.h>
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

typedef void *(*alloc_fn)(size_t);
typedef void *(*allocate_fn)(alloc_fn, size_t);

// Only touches its arguments, so it runs unchanged from either copy.
__attribute__((noinline, optimize("no-optimize-sibling-calls"))) void *
allocate(alloc_fn alloc, size_t size) {
  return alloc(size);
}

int main(void) {
  char exe[PATH_MAX];
  ssize_t exe_len = readlink("/proc/self/exe", exe, sizeof(exe) - 1);
  if (exe_len < 0) {
    return 1;
  }
  exe[exe_len] = '\0';

  FILE *maps = fopen("/proc/self/maps", "r");
  if (!maps) {
    return 1;
  }

  uintptr_t target = (uintptr_t)&allocate;
  uintptr_t start = 0, end = 0;
  unsigned long long offset = 0;
  char line[PATH_MAX + 128];
  while (fgets(line, sizeof(line), maps)) {
    uintptr_t s, e;
    char perms[5];
    unsigned long long off;
    if (sscanf(line, "%lx-%lx %4s %llx", &s, &e, perms, &off) == 4 &&
        perms[2] == 'x' && s <= target && target < e) {
      start = s;
      end = e;
      offset = off;
      break;
    }
  }
  fclose(maps);
  if (!start) {
    return 1;
  }

  int fd = open(exe, O_RDONLY);
  if (fd < 0) {
    return 1;
  }
  uint8_t *copy = mmap(NULL, end - start, PROT_READ | PROT_EXEC, MAP_PRIVATE,
                       fd, (off_t)offset);
  close(fd);
  if (copy == MAP_FAILED) {
    return 1;
  }

  allocate_fn remapped = (allocate_fn)(void *)(copy + (target - start));
  free(allocate(malloc, 1111));
  free(remapped(malloc, 2222));
  maps = fopen("/proc/self/maps", "r");
  if (!maps) {
    return 1;
  }
  while (fgets(line, sizeof(line), maps)) {
    char perms[5];
    if (sscanf(line, "%*x-%*x %4s", perms) == 1 && perms[2] == 'x' &&
        strstr(line, exe)) {
      fputs(line, stdout);
    }
  }
  fclose(maps);

  printf("allocate %lx %lx\n", target, (uintptr_t)remapped);
  return 0;
}
