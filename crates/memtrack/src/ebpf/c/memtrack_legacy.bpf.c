/* Legacy variant: uprobes attach through perf_event_open(), which works on
 * kernels predating uprobe_multi (< 6.6) but needs CAP_PERFMON in the init user
 * namespace and so cannot be delegated via a BPF token. See utils/variant.h.
 * Program bodies live in main.bpf.c. */
#include "main.bpf.c"
