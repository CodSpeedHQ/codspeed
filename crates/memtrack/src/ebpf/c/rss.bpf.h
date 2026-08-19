#ifndef __RSS_BPF_H__
#define __RSS_BPF_H__

#include "event.h"
#include "utils/event_helpers.h"
#include "utils/mm_ownership.h"
#include "utils/process_tracking.h"

/* Key: rss_stat's mm_id in the upper 32 bits, the counter member in the lower.
 * Value: the tgid that last reported this counter from its own context, the mm it
 * held then, and the last size accepted for it.
 *
 * mm_id is a hash, so an out-of-context report (curr == 0) is accepted only while
 * mm_by_pid still binds that tgid to the seeded mm, and only when it lowers the
 * counter: a hash collision or a stale reclaim read must never invent a peak. */
struct rss_owner {
    __u32 pid;
    __u64 mm;
    __u64 size;
};
struct {
    __uint(type, BPF_MAP_TYPE_LRU_HASH);
    __uint(max_entries, 40960);
    __type(key, __u64);
    __type(value, struct rss_owner);
} rss_owner_by_counter SEC(".maps");

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
        /* curr == 1 means current->mm is the counter's mm. mm_by_pid is maintained
         * here too because the rmap hooks may not be attached. */
        struct task_struct* task = bpf_get_current_task_btf();
        __u64 mm = (__u64)BPF_CORE_READ(task, mm);
        struct rss_owner state = {.pid = cur, .mm = mm, .size = size};
        bpf_map_update_elem(&rss_owner_by_counter, &key, &state, BPF_ANY);
        rebind_pid_mm(cur, mm);
    } else {
        struct rss_owner* found = bpf_map_lookup_elem(&rss_owner_by_counter, &key);
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

        __u64* owner_mm = bpf_map_lookup_elem(&mm_by_pid, &owner);
        if (!owner_mm || *owner_mm != found->mm) {
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

#endif /* __RSS_BPF_H__ */
