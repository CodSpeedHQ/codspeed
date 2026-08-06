#ifndef __PROCESS_TRACKING_H__
#define __PROCESS_TRACKING_H__

#include "map_helpers.h"
#include "variant.h"

BPF_HASH_MAP(tracked_pids, __u32, __u8, 10000);
BPF_HASH_MAP(pids_ppid, __u32, __u32, 10000);
BPF_ARRAY_MAP(tracking_enabled, __u8, 1);

static __always_inline int is_tracked(__u32 pid) {
    if (bpf_map_lookup_elem(&tracked_pids, &pid)) {
        return 1;
    }

#pragma unroll
    for (int i = 0; i < 5; i++) {
        __u32* ppid = bpf_map_lookup_elem(&pids_ppid, &pid);
        if (!ppid) {
            break;
        }
        pid = *ppid;
        if (bpf_map_lookup_elem(&tracked_pids, &pid)) {
            return 1;
        }
    }

    return 0;
}

static __always_inline int is_enabled(void) {
    __u32 key = 0;
    __u8* enabled = bpf_map_lookup_elem(&tracking_enabled, &key);
    /* ARRAY-map lookups can't fail for a valid index; fail closed if one ever does. */
    if (!enabled) {
        return 0;
    }
    return *enabled;
}

static __always_inline void track_child(__u32 child_pid, __u32 parent_pid) {
    __u8 marker = 1;
    bpf_map_update_elem(&tracked_pids, &child_pid, &marker, BPF_ANY);
    bpf_map_update_elem(&pids_ppid, &child_pid, &parent_pid, BPF_ANY);
}

#endif /* __PROCESS_TRACKING_H__ */
