/* Classic perf attach variant: perf-based uprobe/uretprobe + a perf
 * sched_process_fork tracepoint. Works on kernels predating uprobe_multi but
 * needs CAP_PERFMON in the init user namespace, so it cannot be delegated into
 * the sandbox. The program bodies live in the shared memtrack.bpf.c. */
#include "memtrack.bpf.c"
