#ifndef __STOPPED_H__
#define __STOPPED_H__

#include "map_helpers.h"

#define MEMTRACK_SIGSTOP 19

/* tgid -> 1 for every process BPF stopped, one map per reason. A process is
 * only stopped once it is recorded, and userspace resumes only recorded
 * processes, so a process stopped for both reasons resumes once neither map
 * holds it. Sized like tracked_pids. */
BPF_HASH_MAP(pressure_stopped, __u32, __u8, 10000);
BPF_HASH_MAP(attach_stopped, __u32, __u8, 10000);

#endif /* __STOPPED_H__ */
