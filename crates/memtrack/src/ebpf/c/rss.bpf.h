#ifndef __RSS_BPF_H__
#define __RSS_BPF_H__

#include "event.h"
#include "utils/event_helpers.h"
#include "utils/process_tracking.h"

/* == Process lifecycle events ==
 *
 * FORK lets userland seed a child's RSS from its parent at fork time: the
 * kernel copies the mm counters during dup_mmap, but those updates fire
 * rss_stat out of the child's context and anon COW faults are
 * counter-neutral, so a child that only touches inherited memory never
 * reports its RSS on its own. EXEC and EXIT mark the points where the
 * address space is replaced or torn down, so userland resets to zero.
 */

#define CLONE_THREAD 0x00010000

SEC("tp_btf/task_newtask")
int BPF_PROG(tracepoint_task_newtask, struct task_struct* child, __u64 clone_flags) {
    if (clone_flags & CLONE_THREAD) {
        return 0;
    }

    __u32 parent_pid = current_tgid();
    if (!is_tracked(parent_pid)) {
        return 0;
    }

    /* Register the child here rather than on sched_process_fork: that
     * tracepoint fires for CLONE_THREAD too and carries only raw task pids,
     * which would fill the tracking maps with thread tids that no exit path
     * removes (group death deletes only the tgid). task_newtask fires before
     * wake_up_new_task, so registration precedes any event from the child.
     * The BTF-typed variant is used because the child's pid must be read in
     * the tracker's namespace, which the tracepoint's raw pid field cannot
     * give. */
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
     * tracked_pids delete below arbitrates: only the task that wins it emits. */
    struct task_struct* task = bpf_get_current_task_btf();
    if (BPF_CORE_READ(task, signal, live.counter) != 0) {
        return 0;
    }

    /* Untrack the pid before submitting: lifetime events are gated only on
     * is_tracked, so a stale entry would keep streaming events if the kernel
     * reuses the pid for an unrelated process. */
    if (bpf_map_delete_elem(&tracked_pids, &pid) != 0) {
        return 0;
    }
    bpf_map_delete_elem(&pids_ppid, &pid);

    SUBMIT_EVENT_AS(pid, EVENT_TYPE_EXIT, {});
}

#endif /* __RSS_BPF_H__ */
