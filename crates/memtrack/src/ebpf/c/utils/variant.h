#ifndef __VARIANT_H__
#define __VARIANT_H__

/* Attach mechanism, selected at build time by MEMTRACK_BPF_VARIANT_TOKEN.
 *
 * A BPF token only relaxes capability checks made by the bpf() syscall itself
 * (bpf_token_capable() in kernel/bpf/token.c), so only attach paths that go
 * entirely through bpf() can be delegated into an unprivileged user namespace.
 * uprobe_multi links qualify: BPF_LINK_CREATE with BPF_TRACE_UPROBE_MULTI does
 * no capability check of its own, the CAP_BPF/CAP_PERFMON check happens at
 * BPF_PROG_LOAD, where the token applies.
 *
 * Classic uprobes do not: libbpf attaches them with perf_event_open(), which is
 * not a bpf() command and checks capabilities against the init user namespace
 * (capable(CAP_SYS_ADMIN) in perf_uprobe_event_init(), or CAP_PERFMON plus
 * tracefs write access on the legacy path). No token can grant those.
 *
 * uprobe_multi requires kernel >= 6.6.
 *
 * https://docs.ebpf.io/linux/concepts/token/
 * https://lwn.net/Articles/959350/
 */
#ifdef MEMTRACK_BPF_VARIANT_TOKEN
#define UPROBE_SEC "uprobe.multi"
#define URETPROBE_SEC "uretprobe.multi"
#else
#define UPROBE_SEC "uprobe"
#define URETPROBE_SEC "uretprobe"
#endif

/* PID namespace the userspace tracker resolves PIDs in, as the (dev, ino) pair
 * bpf_get_ns_current_pid_tgid() expects. Set from userspace; ino == 0 means
 * "report global PIDs", which is what these helpers resolve to in the init
 * namespace anyway.
 *
 * eBPF observes PIDs from the init namespace, so when the tracker runs inside a
 * PID namespace the PIDs it registers are namespace-local and would never match
 * what the probes see. Resolving in the tracker's namespace keeps both agreeing.
 *
 * https://docs.ebpf.io/linux/helper-function/bpf_get_ns_current_pid_tgid/
 */
const volatile __u64 target_pidns_dev = 0;
const volatile __u64 target_pidns_ino = 0;

/* Identity of the current task in the configured namespace.
 *
 * tgid is the thread-group id (what userspace calls the pid); tid is the thread
 * id (what the kernel calls the pid), unique per thread, so it keys the
 * uprobe/uretprobe argument hand-off maps.
 */
struct task_ids {
    __u32 tgid;
    __u32 tid;
};

/* The current task's (tgid << 32) | pid in the configured namespace, packed the
 * way bpf_get_current_pid_tgid() returns it.
 *
 * https://docs.ebpf.io/linux/helper-function/bpf_get_current_pid_tgid/
 */
static __always_inline __u64 memtrack_current_pid_tgid(void) {
    if (target_pidns_ino == 0) {
        return bpf_get_current_pid_tgid();
    }
    struct bpf_pidns_info nsinfo = {};
    if (bpf_get_ns_current_pid_tgid(target_pidns_dev, target_pidns_ino, &nsinfo, sizeof(nsinfo)) !=
        0) {
        return 0;
    }
    return ((__u64)nsinfo.tgid << 32) | nsinfo.pid;
}

/* Both ids from a single helper call, for paths that need them together. */
static __always_inline struct task_ids current_task_ids(void) {
    __u64 pid_tgid = memtrack_current_pid_tgid();
    struct task_ids ids = {.tgid = pid_tgid >> 32, .tid = (__u32)pid_tgid};
    return ids;
}

static __always_inline __u32 current_tgid(void) {
    return current_task_ids().tgid;
}

static __always_inline __u32 current_tid(void) {
    return current_task_ids().tid;
}

/* Thread-group id of an arbitrary task in the configured namespace.
 *
 * struct pid holds one upid per namespace level the task is visible in, with
 * numbers[level] the innermost. A task whose innermost namespace is not the
 * target lives in a sibling or deeper namespace, so it has no PID the tracker
 * could match, hence 0.
 */
static __always_inline __u32 task_ns_tgid(struct task_struct* task) {
    if (target_pidns_ino == 0) {
        return BPF_CORE_READ(task, tgid);
    }
    struct pid* thread_pid = BPF_CORE_READ(task, thread_pid);
    if (!thread_pid) {
        return 0;
    }
    unsigned int level = BPF_CORE_READ(thread_pid, level);
    /* Bounded so the verifier can prove the numbers[] access is in range. */
    if (level >= 4) {
        return 0;
    }
    struct upid* up = &thread_pid->numbers[level];
    if (BPF_CORE_READ(up, ns, ns.inum) != target_pidns_ino) {
        return 0;
    }
    return BPF_CORE_READ(up, nr);
}

#endif /* __VARIANT_H__ */
