/* BPF-token attach variant: uprobe_multi links + tp_btf fork hook, authorized
 * through bpf() so a delegated token can load them inside the sandbox. Requires
 * a kernel with uprobe_multi (>= 6.6). The program bodies live in the shared
 * memtrack.bpf.c; only the SEC() annotations differ, keyed on this define. */
#define MEMTRACK_BPF_LINKS 1
#include "memtrack.bpf.c"
