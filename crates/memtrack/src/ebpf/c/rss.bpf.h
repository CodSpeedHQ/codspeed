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
    if ((__u64)BPF_CORE_READ(task, mm) != mm) {
        return 0;
    }

    __u64 tid = bpf_get_current_pid_tgid();
    __u32 pid = tid >> 32;
    if (!is_tracked(pid)) {
        return 0;
    }

    SUBMIT_EVENT_AS(pid, EVENT_TYPE_RMAP, {
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

#endif /* __RSS_BPF_H__ */
