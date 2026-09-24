#ifndef __STOPPED_H__
#define __STOPPED_H__

#include "map_helpers.h"

#define MEMTRACK_SIGSTOP 19

/* tgid -> stop record for every process BPF stopped, one map per reason:
 * pressure_stopped holds the stop ktime (ns), attach_stopped a 1 marker. A
 * process is only stopped once it is recorded, and userspace resumes only
 * recorded processes, so a process stopped for both reasons resumes once
 * neither map holds it. Sized like tracked_pids. */
BPF_HASH_MAP(pressure_stopped, __u32, __u64, 10000);
BPF_HASH_MAP(attach_stopped, __u32, __u8, 10000);

#endif /* __STOPPED_H__ */
