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
} mm_to_pid SEC(".maps");

static __always_inline int submit_rss_event(__u32 owner_pid, __s32 member, __u64 size) {
    SUBMIT_EVENT_AS(owner_pid, EVENT_TYPE_RSS, {
        e->data.rss.member = member;
        e->data.rss.size = size;
    });
}

SEC("tracepoint/kmem/rss_stat")
int tracepoint_rss_stat(struct trace_event_raw_rss_stat* ctx) {
    if (ctx->member == MM_SWAPENTS) {
        return 0;
    }

    __u32 cur = bpf_get_current_pid_tgid() >> 32;
    __u64 key = ((__u64)ctx->mm_id << 32) | (__u32)ctx->member;
    __u64 size = ctx->size;
    __u32 owner;

    if (ctx->curr) {
        if (!is_tracked(cur)) {
            return 0;
        }
        owner = cur;
        struct rss_owner state = {.pid = cur, .size = size};
        bpf_map_update_elem(&mm_to_pid, &key, &state, BPF_ANY);
    } else {
        struct rss_owner* found = bpf_map_lookup_elem(&mm_to_pid, &key);
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

#endif /* __RSS_BPF_H__ */
