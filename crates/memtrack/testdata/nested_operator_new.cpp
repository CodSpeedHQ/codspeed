// libstdc++'s `operator new` calls `malloc`, and memtrack hooks both. When the
// nested `malloc` uprobe copies the user stack, `operator new`'s return slot
// already holds the kernel's uretprobe trampoline instead of the return
// address into `allocate`.
//
// Writes the start of the `[uprobes]` mapping (the trampoline page) to the path
// given as argv[1], so the test can look for it in the captured stack bytes.
// `allocate` has C linkage so the test can find its address range by name.
#include <cstdio>
#include <cstring>

// Distinctive size so the test can find these allocations among libc's own.
struct Payload {
    char bytes[0x2A5];
};

static void* volatile escaped_pointer;

extern "C" __attribute__((noinline)) void allocate() {
    Payload* value = new Payload();
    escaped_pointer = value;
    delete value;
}

static unsigned long uprobes_mapping_start() {
    FILE* maps = std::fopen("/proc/self/maps", "r");
    if (!maps) {
        return 0;
    }

    char line[512];
    unsigned long start = 0;
    while (std::fgets(line, sizeof(line), maps)) {
        if (std::strstr(line, "[uprobes]")) {
            std::sscanf(line, "%lx-", &start);
            break;
        }
    }
    std::fclose(maps);
    return start;
}

int main(int argc, char** argv) {
    if (argc < 2) {
        return 2;
    }

    for (int i = 0; i < 16; ++i) {
        allocate();
    }

    FILE* out = std::fopen(argv[1], "w");
    if (!out) {
        return 1;
    }
    std::fprintf(out, "%lx\n", uprobes_mapping_start());
    std::fclose(out);
    return 0;
}
