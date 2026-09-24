#ifndef __PRESSURE_BPF_H__
#define __PRESSURE_BPF_H__

#include <bpf/bpf_helpers.h>

#include "map_helpers.h"
#include "process_tracking.h"
#include "stopped.h"

/* Ring pressure stop. Call only after submit/discard or a failed reserve:
 * stopping with a live reservation would wedge the ring. A tracked producer
 * that writes while the ring is over the watermark is stopped and recorded,
 * so processes that do not write keep running. Userspace resumes the
 * recorded producers once it has flushed the ring. */

#define MEMTRACK_PRESSURE_HEADROOM_FRAC 4 /* stop at (FRAC-1)/FRAC = 75% used */

static __always_inline int memtrack_ring_over_watermark(void* ring) {
    __u64 size = bpf_ringbuf_query(ring, BPF_RB_RING_SIZE);
    __u64 avail = bpf_ringbuf_query(ring, BPF_RB_AVAIL_DATA);
    return avail >= size - size / MEMTRACK_PRESSURE_HEADROOM_FRAC;
}

static __always_inline void memtrack_check_ring_pressure(void* ring, __u32 current_tgid) {
    if (!memtrack_ring_over_watermark(ring)) {
        return;
    }

    /* Never stop an untracked process that happens to trigger a probe. */
    if (!is_tracked(current_tgid)) {
        return;
    }

    __u8 marker = 1;
    if (bpf_map_update_elem(&pressure_stopped, &current_tgid, &marker, BPF_ANY) != 0) {
        return;
    }
    bpf_send_signal(MEMTRACK_SIGSTOP);
}

#endif /* __PRESSURE_BPF_H__ */
