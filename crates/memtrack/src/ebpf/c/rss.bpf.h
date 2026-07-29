#ifndef __RSS_BPF_H__
#define __RSS_BPF_H__

#include "event.h"
#include "utils/event_helpers.h"
#include "utils/process_tracking.h"

/* (rss_stat mm_id << 32 | member) -> {owning tgid, its mm when seeded, last
 * in-context size}. An external (curr==0) update may only lower the counter, and
 * only while pid_mm still binds the owner to the seeded mm: mm_id is a hash, so
 * without both guards a stale reclaim read, a hash collision, or a recycled
 * mm_struct slab slot could invent a peak. */
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
} rss_counter_owner SEC(".maps");

/* Foreign-actor rmap attribution: rmap events run by a task other than the mm's
 * owner (kswapd reclaim, another process's process_madvise, khugepaged, KSM,
 * uffd) carry no owning-pid context, so mm_owner recovers it from the mm_struct
 * pointer. pid_mm is the inverse, letting exec and exit remove an entry by
 * value; attribution requires both to agree, so a stale mm fails closed.
 *
 * pid_mm must not use LRU eviction: losing the inverse binding would leave exec
 * and exit unable to remove the forward entry. */
struct {
    __uint(type, BPF_MAP_TYPE_LRU_HASH);
    __uint(max_entries, 10240);
    __type(key, __u64);
    __type(value, __u32);
} mm_owner SEC(".maps");
BPF_HASH_MAP(pid_mm, __u32, __u64, 10240);

/* Rebind pid's address space; mm == 0 unbinds at exit. The mm_owner entry is only
 * dropped while it still names pid: a live CLONE_VM sibling shares the mm and must
 * keep its registration. */
static __always_inline void set_pid_mm(__u32 pid, __u64 mm) {
    __u64* cur = bpf_map_lookup_elem(&pid_mm, &pid);
    if (cur && *cur == mm) {
        return;
    }
    if (cur) {
        __u32* owner = bpf_map_lookup_elem(&mm_owner, cur);
        if (owner && *owner == pid) {
            bpf_map_delete_elem(&mm_owner, cur);
        }
    }
    if (mm) {
        bpf_map_update_elem(&pid_mm, &pid, &mm, BPF_ANY);
    } else {
        bpf_map_delete_elem(&pid_mm, &pid);
    }
}

/* Claim mm for pid without stealing from a live owner: CLONE_VM siblings share the
 * mm, and overwriting would let the child's exec-time cleanup delete the entry out
 * from under the still-live parent. */
static __always_inline void mm_owner_take(__u64 mm, __u32 pid) {
    __u32* reg = bpf_map_lookup_elem(&mm_owner, &mm);
    if (!reg) {
        bpf_map_update_elem(&mm_owner, &mm, &pid, BPF_ANY);
        return;
    }
    if (*reg == pid) {
        return;
    }
    __u64* reg_mm = bpf_map_lookup_elem(&pid_mm, reg);
    if (!reg_mm || *reg_mm != mm) {
        bpf_map_update_elem(&mm_owner, &mm, &pid, BPF_ANY);
    }
}

#define FOLIO_MAPPING_ANON 0x1UL

const volatile __u32 page_shift = 12;

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
        /* curr == 1 means current->mm is the counter's mm. pid_mm is maintained
         * here too because the rmap hooks may not be attached. */
        struct task_struct* task = bpf_get_current_task_btf();
        __u64 mm = (__u64)BPF_CORE_READ(task, mm);
        struct rss_owner state = {.pid = cur, .mm = mm, .size = size};
        bpf_map_update_elem(&rss_counter_owner, &key, &state, BPF_ANY);
        set_pid_mm(cur, mm);
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
        __u64* owner_mm = bpf_map_lookup_elem(&pid_mm, &owner);
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

/* Kernels < 6.18 store folio->flags as a bare unsigned long instead of
 * memdesc_flags_t; probe which layout the running kernel has. */
struct folio___legacy {
    unsigned long flags;
} __attribute__((preserve_access_index));

static __always_inline unsigned long folio_read_flags(struct folio* folio) {
    if (bpf_core_field_exists(folio->flags.f)) {
        return BPF_CORE_READ(folio, flags).f;
    }
    return BPF_CORE_READ((struct folio___legacy*)folio, flags);
}

/* Kernels < 6.6 store the large-folio order in a dedicated byte instead of
 * the low byte of _flags_1. */
struct folio___order_byte {
    unsigned char _folio_order;
} __attribute__((preserve_access_index));

static __always_inline unsigned long folio_order(struct folio* folio) {
    if (bpf_core_field_exists(((struct folio___order_byte*)folio)->_folio_order)) {
        return BPF_CORE_READ((struct folio___order_byte*)folio, _folio_order);
    }
    return BPF_CORE_READ(folio, _flags_1) & 0xff;
}

static __always_inline __u64 folio_nr_pages_est(struct folio* folio) {
    unsigned long flags = folio_read_flags(folio);
    if (!(flags & (1UL << bpf_core_enum_value(enum pageflags, PG_head)))) {
        return 1;
    }
    unsigned long order = folio_order(folio);
    return 1UL << order;
}

static __always_inline int folio_is_anon(struct folio* folio) {
    unsigned long mapping = (unsigned long)BPF_CORE_READ(folio, mapping);
    return (mapping & FOLIO_MAPPING_ANON) != 0;
}

/* Mirrors the kernel's mm_counter(): anon folios are also swapbacked, so the
 * anon check must come first. */
static __always_inline __s32 folio_mm_counter(struct folio* folio) {
    if (folio_is_anon(folio)) {
        return MM_ANONPAGES;
    }
    unsigned long flags = folio_read_flags(folio);
    if (flags & (1UL << bpf_core_enum_value(enum pageflags, PG_swapbacked))) {
        return MM_SHMEMPAGES;
    }
    return MM_FILEPAGES;
}

static __always_inline __u64 folio_page_address(struct folio* folio, struct page* page,
                                                struct vm_area_struct* vma) {
    __u64 page_idx = ((__u64)page - (__u64)folio) / bpf_core_type_size(struct page);
    __u64 pgoff = BPF_CORE_READ(folio, index) + page_idx;
    __u64 vm_pgoff = BPF_CORE_READ(vma, vm_pgoff);
    __u64 vm_start = BPF_CORE_READ(vma, vm_start);
    if (pgoff < vm_pgoff) {
        return vm_start;
    }
    return vm_start + ((pgoff - vm_pgoff) << page_shift);
}

static __always_inline int submit_rmap(struct vm_area_struct* vma, __s32 member, __s64 delta,
                                       __u64 addr) {
    __u64 mm = (__u64)BPF_CORE_READ(vma, vm_mm);
    struct task_struct* task = bpf_get_current_task_btf();
    __u32 pid = bpf_get_current_pid_tgid() >> 32;
    __u32 owner;

    if ((__u64)BPF_CORE_READ(task, mm) == mm) {
        if (!is_tracked(pid)) {
            return 0;
        }

        mm_owner_take(mm, pid);
        set_pid_mm(pid, mm);
        owner = pid;
    } else {
        /* Foreign actor (task->mm != mm, including kthreads whose task->mm is NULL):
         * recover the owner from the in-context registration. Fail toward dropping
         * the event on any uncertainty about ownership. */
        __u32* found = bpf_map_lookup_elem(&mm_owner, &mm);
        if (!found) {
            return 0;
        }
        owner = *found;
        if (!is_tracked(owner)) {
            return 0;
        }
        /* An mm_struct address may be reused while a stale owner entry remains.
         * Accept only the current inverse binding. */
        __u64* owner_mm = bpf_map_lookup_elem(&pid_mm, &owner);
        if (!owner_mm || *owner_mm != mm) {
            return 0;
        }
    }

    /* header.tid is stamped from the current task; for a foreign actor it
     * identifies the performer, not the owning pid. */
    SUBMIT_EVENT_AS(owner, EVENT_TYPE_RMAP, {
        e->data.rmap.member = member;
        e->data.rmap.delta = delta;
        e->data.rmap.addr = addr;
    });
}

SEC("fentry/folio_add_new_anon_rmap")
int BPF_PROG(fentry_folio_add_new_anon_rmap, struct folio* folio, struct vm_area_struct* vma,
             unsigned long address) {
    return submit_rmap(vma, MM_ANONPAGES, (__s64)folio_nr_pages_est(folio), address);
}

SEC("fentry/folio_add_anon_rmap_ptes")
int BPF_PROG(fentry_folio_add_anon_rmap_ptes, struct folio* folio, struct page* page, int nr_pages,
             struct vm_area_struct* vma, unsigned long address) {
    return submit_rmap(vma, MM_ANONPAGES, (__s64)nr_pages, address);
}

SEC("fentry/folio_add_anon_rmap_pmd")
int BPF_PROG(fentry_folio_add_anon_rmap_pmd, struct folio* folio, struct page* page,
             struct vm_area_struct* vma, unsigned long address) {
    return submit_rmap(vma, MM_ANONPAGES, (__s64)folio_nr_pages_est(folio), address);
}

static __always_inline int submit_file_rmap(struct folio* folio, struct page* page,
                                            struct vm_area_struct* vma, __s64 delta) {
    return submit_rmap(vma, folio_mm_counter(folio), delta, folio_page_address(folio, page, vma));
}

SEC("fentry/folio_add_file_rmap_ptes")
int BPF_PROG(fentry_folio_add_file_rmap_ptes, struct folio* folio, struct page* page, int nr_pages,
             struct vm_area_struct* vma) {
    return submit_file_rmap(folio, page, vma, (__s64)nr_pages);
}

SEC("fentry/folio_add_file_rmap_pmd")
int BPF_PROG(fentry_folio_add_file_rmap_pmd, struct folio* folio, struct page* page,
             struct vm_area_struct* vma) {
    return submit_file_rmap(folio, page, vma, (__s64)folio_nr_pages_est(folio));
}

SEC("fentry/folio_add_file_rmap_pud")
int BPF_PROG(fentry_folio_add_file_rmap_pud, struct folio* folio, struct page* page,
             struct vm_area_struct* vma) {
    return submit_file_rmap(folio, page, vma, (__s64)folio_nr_pages_est(folio));
}

SEC("fentry/folio_remove_rmap_ptes")
int BPF_PROG(fentry_folio_remove_rmap_ptes, struct folio* folio, struct page* page, int nr_pages,
             struct vm_area_struct* vma) {
    return submit_file_rmap(folio, page, vma, -(__s64)nr_pages);
}

SEC("fentry/folio_remove_rmap_pmd")
int BPF_PROG(fentry_folio_remove_rmap_pmd, struct folio* folio, struct page* page,
             struct vm_area_struct* vma) {
    return submit_file_rmap(folio, page, vma, -(__s64)folio_nr_pages_est(folio));
}

SEC("fentry/folio_remove_rmap_pud")
int BPF_PROG(fentry_folio_remove_rmap_pud, struct folio* folio, struct page* page,
             struct vm_area_struct* vma) {
    return submit_file_rmap(folio, page, vma, -(__s64)folio_nr_pages_est(folio));
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

SEC("tracepoint/task/task_newtask")
int tracepoint_task_newtask(struct trace_event_raw_task_newtask* ctx) {
    if (ctx->clone_flags & CLONE_THREAD) {
        return 0;
    }

    __u64 tid = bpf_get_current_pid_tgid();
    __u32 parent_pid = tid >> 32;
    if (!is_tracked(parent_pid)) {
        return 0;
    }

    /* Register the child here rather than on sched_process_fork: that
     * tracepoint fires for CLONE_THREAD too and carries only raw task pids,
     * which would fill the tracking maps with thread tids that no exit path
     * removes (group death deletes only the tgid). task_newtask fires before
     * wake_up_new_task, so registration precedes any event from the child. */
    __u32 child_pid = ctx->pid;
    track_child(child_pid, parent_pid);

    SUBMIT_EVENT_AS(child_pid, EVENT_TYPE_FORK, { e->data.fork.parent_pid = parent_pid; });
}

SEC("tracepoint/sched/sched_process_exec")
int tracepoint_sched_process_exec(void* ctx) {
    __u32 pid = bpf_get_current_pid_tgid() >> 32;
    if (!is_tracked(pid)) {
        return 0;
    }

    /* SUBMIT_EVENT_AS returns, so the rebind must precede it. */
    struct task_struct* task = bpf_get_current_task_btf();
    __u64 new_mm = (__u64)BPF_CORE_READ(task, mm);
    if (new_mm) {
        mm_owner_take(new_mm, pid);
    }
    set_pid_mm(pid, new_mm);

    SUBMIT_EVENT_AS(pid, EVENT_TYPE_EXEC, {});
}

SEC("tracepoint/sched/sched_process_exit")
int tracepoint_sched_process_exit(void* ctx) {
    __u32 pid = bpf_get_current_pid_tgid() >> 32;
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
     * reuses the pid for an unrelated process. Untracking here also keeps the
     * fixed-size tracking maps from filling up over long sessions. The delete
     * doubles as the exactly-once claim on EXIT. */
    if (bpf_map_delete_elem(&tracked_pids, &pid) != 0) {
        return 0;
    }
    bpf_map_delete_elem(&pids_ppid, &pid);

    /* Drop the ownership mapping so foreign actors stop attributing to a pid
     * the kernel may reuse. */
    set_pid_mm(pid, 0);

    SUBMIT_EVENT_AS(pid, EVENT_TYPE_EXIT, {});
}

#endif /* __RSS_BPF_H__ */
