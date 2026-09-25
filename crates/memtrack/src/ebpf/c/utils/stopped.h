#ifndef __STOPPED_H__
#define __STOPPED_H__

#include <bpf/bpf_helpers.h>

#include "map_helpers.h"

#define MEMTRACK_SIGCONT 18
#define MEMTRACK_SIGSTOP 19

/* tgid -> 1 for every process BPF stopped, one map per reason. A process is
 * recorded before its stop can take effect, and userspace resumes only
 * recorded processes, so a process stopped for both reasons resumes once
 * neither map holds it. Sized like tracked_pids. */
BPF_HASH_MAP(pressure_stopped, __u32, __u8, 10000);
BPF_HASH_MAP(attach_stopped, __u32, __u8, 10000);

/* Stop the current process and record it in `map`. SIGSTOP is queued before
 * the record is written: it only takes effect on return to user mode, so any
 * SIGCONT sent after userspace sees the record cancels or ends the stop.
 * Requires task context with IRQs enabled, where the signal is queued
 * synchronously rather than via irq_work. */
static __always_inline void memtrack_stop_current(void* map, __u32 tgid) {
    if (bpf_send_signal(MEMTRACK_SIGSTOP) != 0) {
        return;
    }

    __u8 marker = 1;
    if (bpf_map_update_elem(map, &tgid, &marker, BPF_ANY) != 0) {
        /* Unrecorded, so nothing would resume it. */
        bpf_send_signal(MEMTRACK_SIGCONT);
    }
}

#endif /* __STOPPED_H__ */
