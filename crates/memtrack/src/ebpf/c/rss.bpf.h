#ifndef __RSS_BPF_H__
#define __RSS_BPF_H__

#include "event.h"
#include "utils/event_helpers.h"
#include "utils/process_tracking.h"

/* (rss_stat mm_id << 32 | member) -> {owning tgid, last in-context size}. Keyed per
 * counter so an external (curr==0) update is attributed only once that mm/member was
 * established in-context. An external event may only lower a counter: any size above
 * the last in-context value is dropped, so neither a stale/racing reclaim read nor an
 * mm_id hash collision with another task can invent a peak. LRU eviction + re-seeding
 * on the owner's next in-context event covers hash reuse, so no teardown hook needed. */
struct rss_owner {
    __u32 pid;
    __u64 size;
};
struct {
    __uint(type, BPF_MAP_TYPE_LRU_HASH);
    __uint(max_entries, 40960);
    __type(key, __u64);
    __type(value, struct rss_owner);
} rss_counter_owner SEC(".maps");

static __always_inline int submit_rss_event(__u32 owner_pid, __s32 member, __u64 size) {
    SUBMIT_EVENT_AS(owner_pid, EVENT_TYPE_RSS, {
        e->data.rss.member = member;
        e->data.rss.size = size;
    });
}

SEC("tracepoint/kmem/rss_stat")
int tracepoint_rss_stat(struct trace_event_raw_rss_stat* ctx) {
    /* Swap is out of scope: MM_SWAPENTS counts entries that are no longer
     * resident, and nothing downstream reports them. */
    if (ctx->member == MM_SWAPENTS) {
        return 0;
    }

    __u32 cur = current_tgid();
    __u64 key = ((__u64)ctx->mm_id << 32) | (__u32)ctx->member;
    __u64 size = ctx->size;
    __u32 owner;

    if (ctx->curr) {
        if (!is_tracked(cur)) {
            return 0;
        }
        owner = cur;
        struct rss_owner state = {.pid = cur, .size = size};
        bpf_map_update_elem(&rss_counter_owner, &key, &state, BPF_ANY);
    } else {
        struct rss_owner* found = bpf_map_lookup_elem(&rss_counter_owner, &key);
        if (!found) {
            return 0;
        }
        owner = found->pid;
        /* The owner's own teardown also presents as curr==0 (current->mm is cleared
         * on exit), so drop it. Genuine external actors (reclaim, another process's
         * madvise) run in a different task, so cur != owner. */
        if (cur == owner) {
            return 0;
        }
        /* An external actor may only lower a counter. A larger value is a stale
         * reclaim read or an mm_id hash collision with another task; dropping it
         * keeps the reconstructed peak identical to the in-context timeline. */
        if (size > found->size) {
            return 0;
        }
        found->size = size;
    }

    return submit_rss_event(owner, ctx->member, size);
}

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
