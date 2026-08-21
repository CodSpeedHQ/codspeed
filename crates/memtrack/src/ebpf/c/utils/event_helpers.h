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

/* Per-thread scratch for the allocator entry/exit hand-off.
 *
 * One slot per instrumented entry point rather than a single shared slot: an
 * allocator may call another (glibc realloc() reaches malloc()), and nested
 * calls on one thread must not clobber each other's saved arguments.
 *
 * `valid` marks which slots hold a value, so a zero argument is still
 * distinguishable from an absent one, and a return probe that fires without a
 * matching entry probe (attach raced with a call already in flight) is ignored.
 */
enum arg_slot {
    SLOT_MALLOC,
    SLOT_CALLOC,
    SLOT_REALLOC,
    SLOT_ALIGNED_ALLOC,
    SLOT_MEMALIGN,
    SLOT_POSIX_MEMALIGN,
    SLOT_MMAP,
    SLOT_BRK,
    SLOT__COUNT,
};

struct memtrack_task_state {
    __u64 arg0[SLOT__COUNT];
    __u64 arg1[SLOT__COUNT];
    __u32 valid;
    /* Memoized positive result of is_tracked(). Tracking is monotonic: pids are
     * only ever added to tracked_pids (from userspace or on fork), never
     * removed, so a task that is tracked stays tracked and the answer can be
     * cached. A negative result is never cached, since the tracker may register
     * this task later. */
    __u8 tracked;
};

BPF_TASK_STORAGE(task_state, struct memtrack_task_state);

/* Task state for the current task if it is tracked, else NULL.
 *
 * Hot path is a single task-storage lookup; the hashed is_tracked() walk runs
 * once per task, on the first hook that observes it. */
static __always_inline struct memtrack_task_state* tracked_state(void) {
    struct task_struct* task = (struct task_struct*)bpf_get_current_task_btf();
    struct memtrack_task_state* st = bpf_task_storage_get(&task_state, task, NULL, 0);
    if (st && st->tracked) {
        return st;
    }

    if (!is_tracked(current_tgid())) {
        return NULL;
    }

    if (!st) {
        st = bpf_task_storage_get(&task_state, task, NULL, BPF_LOCAL_STORAGE_GET_F_CREATE);
        if (!st) {
            return NULL;
        }
    }
    st->tracked = 1;
    return st;
}

static __always_inline int store_arg(enum arg_slot slot, __u64 value) {
    struct memtrack_task_state* st = tracked_state();
    if (!st) {
        return 0;
    }
    st->arg0[slot] = value;
    st->valid |= (1u << slot);
    return 0;
}

static __always_inline int store_args(enum arg_slot slot, __u64 arg0, __u64 arg1) {
    struct memtrack_task_state* st = tracked_state();
    if (!st) {
        return 0;
    }
    st->arg0[slot] = arg0;
    st->arg1[slot] = arg1;
    st->valid |= (1u << slot);
    return 0;
}

/* Submission is split into two classes:
 *  - lifetime events (rss_stat, rmap, fork/exec/exit): emitted whenever the
 *    pid is tracked, ignoring the enable toggle. The parser reconstructs
 *    absolute per-process state from these, so it needs every event from
 *    process birth — a delta stream that starts mid-life can never recover
 *    the resident baseline faulted before enable.
 *  - gated events (e.g. malloc/free/mmap/...): high-volume and only meaningful
 *    inside a measurement window, so they stay behind is_enabled().
 */
/* Consume a slot: returns the state with the slot cleared, or NULL if the entry
 * probe never ran for this call. */
static __always_inline struct memtrack_task_state* take_slot(enum arg_slot slot) {
    struct task_struct* task = (struct task_struct*)bpf_get_current_task_btf();
    struct memtrack_task_state* st = bpf_task_storage_get(&task_state, task, NULL, 0);
    if (!st || !(st->valid & (1u << slot))) {
        return NULL;
    }
    st->valid &= ~(1u << slot);
    return st;
}

#define SUBMIT_EVENT_AS(owner_pid, evt_type, fill_data)                 \
    {                                                                   \
        struct task_ids ids = current_task_ids();                       \
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
        bpf_ringbuf_submit(e, wake_flags());                  \
        return 0;                                                       \
    }

#define SUBMIT_GATED_EVENT(evt_type, fill_data)           \
    {                                                     \
        if (!is_enabled()) {                              \
            return 0;                                     \
        }                                                 \
                                                          \
        struct task_ids owner = current_task_ids();       \
        if (!is_tracked(owner.tgid)) {                    \
            return 0;                                     \
        }                                                 \
                                                          \
        SUBMIT_EVENT_AS(owner.tgid, evt_type, fill_data); \
    }

static __always_inline int submit_alloc_event(__u64 size, __u64 addr) {
    SUBMIT_GATED_EVENT(EVENT_TYPE_MALLOC, {
        e->data.alloc.addr = addr;
        e->data.alloc.size = size;
    });
}

static __always_inline int submit_aligned_alloc_event(__u64 size, __u64 addr) {
    SUBMIT_GATED_EVENT(EVENT_TYPE_ALIGNED_ALLOC, {
        e->data.alloc.addr = addr;
        e->data.alloc.size = size;
    });
}

static __always_inline int submit_calloc_event(__u64 size, __u64 addr) {
    SUBMIT_GATED_EVENT(EVENT_TYPE_CALLOC, {
        e->data.alloc.addr = addr;
        e->data.alloc.size = size;
    });
}

static __always_inline int submit_free_event(__u64 addr) {
    SUBMIT_GATED_EVENT(EVENT_TYPE_FREE, { e->data.free.addr = addr; });
}

static __always_inline int submit_realloc_event(__u64 old_addr, __u64 new_addr, __u64 size) {
    SUBMIT_GATED_EVENT(EVENT_TYPE_REALLOC, {
        e->data.realloc.old_addr = old_addr;
        e->data.realloc.new_addr = new_addr;
        e->data.realloc.size = size;
    });
}

static __always_inline int submit_mmap_event(__u64 addr, __u64 size, __u8 event_type) {
    SUBMIT_GATED_EVENT(event_type, {
        e->data.mmap.addr = addr;
        e->data.mmap.size = size;
    });
}

#endif /* __EVENT_HELPERS_H__ */
