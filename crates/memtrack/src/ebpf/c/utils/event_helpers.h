#ifndef __EVENT_HELPERS_H__
#define __EVENT_HELPERS_H__

#include "../event.h"
#include "map_helpers.h"
#include "process_tracking.h"

BPF_RINGBUF(events, 256 * 1024 * 1024);
BPF_ARRAY_MAP(dropped_events, __u64, 1);

/* Wake the consumer only once this much unconsumed data has accumulated.
 * Per-event wakeups dominate submission cost at high event rates; batching
 * them behind a data watermark amortizes the wakeup to ~1 per thousand
 * events. The userspace poller's poll timeout flushes the tail that never
 * reaches the watermark. */
#define WAKEUP_DATA_SIZE (64 * 1024)

static __always_inline long wake_flags(void) {
    long avail = bpf_ringbuf_query(&events, BPF_RB_AVAIL_DATA);
    return avail >= WAKEUP_DATA_SIZE ? BPF_RB_FORCE_WAKEUP : BPF_RB_NO_WAKEUP;
}

static __always_inline int store_param(void* map, __u64 value) {
    /* Key by the tid: unique per thread, so it survives the entry/exit pair even
     * when several threads are inside the same allocator call. */
    struct task_ids ids = current_task_ids();
    __u64 tid = ids.tid;
    if (is_tracked(ids.tgid)) {
        bpf_map_update_elem(map, &tid, &value, BPF_ANY);
    }
    return 0;
}

static __always_inline __u64* take_param(void* map) {
    __u64 tid = current_tid();
    __u64* value = bpf_map_lookup_elem(map, &tid);
    if (value) {
        bpf_map_delete_elem(map, &tid);
    }
    return value;
}

#define SUBMIT_EVENT_AS(owner_pid, evt_type, fill_data)                 \
    {                                                                   \
        struct task_ids ids = current_task_ids();                       \
                                                                        \
        if (!is_enabled()) {                                            \
            return 0;                                                   \
        }                                                               \
                                                                        \
        struct event* e = bpf_ringbuf_reserve(&events, sizeof(*e), 0);  \
        if (!e) {                                                       \
            __u32 zero = 0;                                             \
            __u64* drops = bpf_map_lookup_elem(&dropped_events, &zero); \
            if (drops) {                                                \
                __sync_fetch_and_add(drops, 1);                         \
            }                                                           \
            return 0;                                                   \
        }                                                               \
                                                                        \
        e->header.timestamp = bpf_ktime_get_ns();                       \
        e->header.pid = owner_pid;                                      \
        e->header.tid = ids.tid;                                        \
        e->header.event_type = evt_type;                                \
                                                                        \
        fill_data;                                                      \
                                                                        \
        bpf_ringbuf_submit(e, wake_flags());                            \
        return 0;                                                       \
    }

#define SUBMIT_EVENT(evt_type, fill_data)                 \
    {                                                     \
        struct task_ids owner = current_task_ids();       \
                                                          \
        if (!is_tracked(owner.tgid)) {                    \
            return 0;                                     \
        }                                                 \
                                                          \
        SUBMIT_EVENT_AS(owner.tgid, evt_type, fill_data); \
    }

static __always_inline int submit_alloc_event(__u64 size, __u64 addr) {
    SUBMIT_EVENT(EVENT_TYPE_MALLOC, {
        e->data.alloc.addr = addr;
        e->data.alloc.size = size;
    });
}

static __always_inline int submit_aligned_alloc_event(__u64 size, __u64 addr) {
    SUBMIT_EVENT(EVENT_TYPE_ALIGNED_ALLOC, {
        e->data.alloc.addr = addr;
        e->data.alloc.size = size;
    });
}

static __always_inline int submit_calloc_event(__u64 size, __u64 addr) {
    SUBMIT_EVENT(EVENT_TYPE_CALLOC, {
        e->data.alloc.addr = addr;
        e->data.alloc.size = size;
    });
}

static __always_inline int submit_free_event(__u64 addr) {
    SUBMIT_EVENT(EVENT_TYPE_FREE, { e->data.free.addr = addr; });
}

static __always_inline int submit_realloc_event(__u64 old_addr, __u64 new_addr, __u64 size) {
    SUBMIT_EVENT(EVENT_TYPE_REALLOC, {
        e->data.realloc.old_addr = old_addr;
        e->data.realloc.new_addr = new_addr;
        e->data.realloc.size = size;
    });
}

static __always_inline int submit_mmap_event(__u64 addr, __u64 size, __u8 event_type) {
    SUBMIT_EVENT(event_type, {
        e->data.mmap.addr = addr;
        e->data.mmap.size = size;
    });
}

#endif /* __EVENT_HELPERS_H__ */
