#ifndef __MM_OWNERSHIP_H__
#define __MM_OWNERSHIP_H__

#include "map_helpers.h"

/* Foreign-actor rmap attribution: rmap events run by a task other than the mm's
 * owner (kswapd reclaim, another process's process_madvise, khugepaged, KSM,
 * uffd) carry no owning-pid context, so owner_by_mm recovers it from the mm_struct
 * pointer. mm_by_pid is the inverse, letting exec and exit remove an entry by
 * value; attribution requires both to agree, so a stale mm fails closed.
 *
 * mm_by_pid must not use LRU eviction: losing the inverse binding would leave exec
 * and exit unable to remove the forward entry. */
struct {
    __uint(type, BPF_MAP_TYPE_LRU_HASH);
    __uint(max_entries, 10240);
    __type(key, __u64);
    __type(value, __u32);
} owner_by_mm SEC(".maps");
BPF_HASH_MAP(mm_by_pid, __u32, __u64, 10240);

/* Rebind pid's address space; mm == 0 unbinds at exit. The owner_by_mm entry is only
 * dropped while it still names pid: a live CLONE_VM sibling shares the mm and must
 * keep its registration. */
static __always_inline void rebind_pid_mm(__u32 pid, __u64 mm) {
    __u64* cur = bpf_map_lookup_elem(&mm_by_pid, &pid);
    if (cur && *cur == mm) {
        return;
    }
    if (cur) {
        __u32* owner = bpf_map_lookup_elem(&owner_by_mm, cur);
        if (owner && *owner == pid) {
            bpf_map_delete_elem(&owner_by_mm, cur);
        }
    }
    if (mm) {
        bpf_map_update_elem(&mm_by_pid, &pid, &mm, BPF_ANY);
    } else {
        bpf_map_delete_elem(&mm_by_pid, &pid);
    }
}

/* Claim mm for pid without stealing from a live owner: CLONE_VM siblings share the
 * mm, and overwriting would let the child's exec-time cleanup delete the entry out
 * from under the still-live parent. */
static __always_inline void claim_mm_owner(__u64 mm, __u32 pid) {
    __u32* reg = bpf_map_lookup_elem(&owner_by_mm, &mm);
    if (!reg) {
        bpf_map_update_elem(&owner_by_mm, &mm, &pid, BPF_ANY);
        return;
    }
    if (*reg == pid) {
        return;
    }
    __u64* reg_mm = bpf_map_lookup_elem(&mm_by_pid, reg);
    if (!reg_mm || *reg_mm != mm) {
        bpf_map_update_elem(&owner_by_mm, &mm, &pid, BPF_ANY);
    }
}

#endif /* __MM_OWNERSHIP_H__ */
