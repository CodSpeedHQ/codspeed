#ifndef __PROCESS_STOP_H__
#define __PROCESS_STOP_H__

#include <bpf/bpf_helpers.h>

#include "map_helpers.h"

#define MEMTRACK_SIGCONT 18
#define MEMTRACK_SIGSTOP 19

/* tgid -> stop ktime (ns) for every process BPF stopped, one map per reason.
 * A process is recorded before its stop can take effect, and userspace
 * resumes only recorded processes, so a process stopped for both reasons
 * resumes once neither map holds it. Sized like tracked_pids. */
BPF_HASH_MAP(pressure_stopped, __u32, __u64, 10000);
BPF_HASH_MAP(attach_stopped, __u32, __u64, 10000);
/* Stops that could not be recorded because the map was full; userspace warns. */
BPF_ARRAY_MAP(stop_record_failed, __u64, 1);

/* Stop the current process and record it in `map`. SIGSTOP is queued before
 * the record is written: it only takes effect on return to user mode, so any
 * SIGCONT sent after userspace sees the record cancels or ends the stop.
 * Requires task context with IRQs enabled, where the signal is queued
 * synchronously rather than via irq_work. */
static __always_inline void memtrack_stop_current(void* map, __u32 tgid) {
    if (bpf_send_signal(MEMTRACK_SIGSTOP) != 0) {
        return;
    }

    __u64 stopped_at = bpf_ktime_get_ns();
    if (bpf_map_update_elem(map, &tgid, &stopped_at, BPF_ANY) == 0) {
        return;
    }

    __u32 zero = 0;
    __u64* failed = bpf_map_lookup_elem(&stop_record_failed, &zero);
    if (failed) {
        __sync_fetch_and_add(failed, 1);
    }
    /* Unrecorded, so nothing would resume it for this reason. Keep it stopped
     * if the other reason holds it: that release will resume it. */
    if (bpf_map_lookup_elem(&pressure_stopped, &tgid) ||
        bpf_map_lookup_elem(&attach_stopped, &tgid)) {
        return;
    }
    bpf_send_signal(MEMTRACK_SIGCONT);
}

#endif /* __PROCESS_STOP_H__ */
