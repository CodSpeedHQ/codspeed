#ifndef __PROCESS_TRACKING_BPF_H__
#define __PROCESS_TRACKING_BPF_H__

#include "event.h"
#include "utils/event_helpers.h"
#include "utils/mm_ownership.h"
#include "utils/process_tracking.h"

/* FORK lets userland seed a child's RSS from its parent at fork time: the
 * kernel copies the mm counters during dup_mmap, but those updates fire
 * rss_stat out of the child's context and anon COW faults are
 * counter-neutral, so a child that only touches inherited memory never
 * reports its RSS on its own. EXEC and EXIT mark the points where the
 * address space is replaced or torn down, so userland resets to zero.
 */

/* tp_btf rather than a classic tracepoint: it attaches with
 * BPF_RAW_TRACEPOINT_OPEN, which a token can delegate, and its task_struct
 * arguments let the child's pid resolve in the tracker's namespace. */
SEC("tp_btf/sched_process_fork")
int BPF_PROG(tracepoint_sched_process_fork, struct task_struct* parent, struct task_struct* child) {
    /* copy_process assigns tgid = current->tgid for CLONE_THREAD, pid otherwise.
     * A tid registered here would never be removed: group death untracks only
     * the tgid. */
    if (BPF_CORE_READ(child, pid) != BPF_CORE_READ(child, tgid)) {
        return 0;
    }

    __u32 parent_pid = current_tgid();
    if (!is_tracked(parent_pid)) {
        return 0;
    }

    __u32 child_pid = task_ns_tgid(child);
    if (!child_pid) {
        return 0;
    }
    track_child(child_pid, parent_pid);

    SUBMIT_EVENT_AS(child_pid, EVENT_TYPE_FORK, { e->data.fork.parent_pid = parent_pid; });
}

SEC("tracepoint/sched/sched_process_exec")
int tracepoint_sched_process_exec(void* ctx) {
    __u32 pid = current_tgid();
    if (!is_tracked(pid)) {
        return 0;
    }

    /* SUBMIT_EVENT_AS returns, so the rebind must precede it. */
    struct task_struct* task = bpf_get_current_task_btf();
    __u64 new_mm = (__u64)BPF_CORE_READ(task, mm);
    if (new_mm) {
        claim_mm_owner(new_mm, pid);
    }
    rebind_pid_mm(pid, new_mm);

    SUBMIT_EVENT_AS(pid, EVENT_TYPE_EXEC, {});
}

SEC("tracepoint/sched/sched_process_exit")
int tracepoint_sched_process_exit(void* ctx) {
    __u32 pid = current_tgid();
    if (!is_tracked(pid)) {
        return 0;
    }

    /* EXIT marks the death of the whole thread group, not of one thread: the
     * leader can pthread_exit while workers keep running, and the last thread
     * to exit need not be the leader. do_exit decrements signal->live before
     * this tracepoint fires, so live == 0 identifies the dying thread group's
     * final exit — but concurrently exiting threads can BOTH read 0, so the
     * untrack below arbitrates: only the task that wins it emits. */
    struct task_struct* task = bpf_get_current_task_btf();
    if (BPF_CORE_READ(task, signal, live.counter) != 0) {
        return 0;
    }

    /* Untrack the pid before submitting: lifetime events are gated only on
     * is_tracked, so a stale entry would keep streaming events if the kernel
     * reuses the pid for an unrelated process. */
    if (!untrack_pid(pid)) {
        return 0;
    }

    /* Drop the ownership mapping so foreign actors stop attributing to a pid
     * the kernel may reuse. */
    rebind_pid_mm(pid, 0);

    SUBMIT_EVENT_AS(pid, EVENT_TYPE_EXIT, {});
}

#endif /* __PROCESS_TRACKING_BPF_H__ */
