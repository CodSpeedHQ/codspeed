/* Token variant: uprobes attach as uprobe_multi links, so every attach goes
 * through bpf() and a delegated BPF token can authorize it without host
 * privileges. Requires kernel >= 6.6. See utils/variant.h for why classic
 * uprobes cannot be delegated. Program bodies live in main.bpf.c; only the
 * SEC() annotations differ, keyed on this define. */
#define MEMTRACK_BPF_VARIANT_TOKEN 1
#include "main.bpf.c"
