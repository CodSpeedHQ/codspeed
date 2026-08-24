#ifndef __RMAP_BPF_H__
#define __RMAP_BPF_H__

#include "event.h"
#include "utils/event_helpers.h"
#include "utils/folio.h"
#include "utils/mm_ownership.h"
#include "utils/process_tracking.h"

static __always_inline int submit_rmap(struct vm_area_struct* vma, __s32 member, __s64 delta,
                                       __u64 addr) {
    __u64 mm = (__u64)BPF_CORE_READ(vma, vm_mm);
    struct task_struct* task = bpf_get_current_task_btf();
    __u32 pid = current_tgid();
    __u32 owner;

    if ((__u64)BPF_CORE_READ(task, mm) == mm) {
        if (!is_tracked(pid)) {
            return 0;
        }

        claim_mm_owner(mm, pid);
        rebind_pid_mm(pid, mm);
        owner = pid;
    } else {
        /* Foreign actor (task->mm != mm, including kthreads whose task->mm is NULL):
         * recover the owner from the in-context registration. Fail toward dropping
         * the event on any uncertainty about ownership. */
        __u32* found = bpf_map_lookup_elem(&owner_by_mm, &mm);
        if (!found) {
            return 0;
        }
        owner = *found;
        if (!is_tracked(owner)) {
            return 0;
        }
        /* An mm_struct address may be reused while a stale owner entry remains.
         * Accept only the current inverse binding. */
        __u64* owner_mm = bpf_map_lookup_elem(&mm_by_pid, &owner);
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

#endif /* __RMAP_BPF_H__ */
