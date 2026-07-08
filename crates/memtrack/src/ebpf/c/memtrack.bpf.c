// clang-format off
// Prevent clang-format from reformatting the include statement, which is
// needed for the bpf headers below.
#include "vmlinux.h"
// clang-format on

#include <bpf/bpf_core_read.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>

#include "event.h"

char LICENSE[] SEC("license") = "GPL";

/* Attach mechanism, selected at build time. With a delegated BPF token
 * (MEMTRACK_BPF_LINKS) the uprobes attach as uprobe_multi links and the fork
 * hook as tp_btf, both authorized through bpf(). Without it they use the classic
 * perf-based uprobe/tracepoint paths, which work on kernels predating
 * uprobe_multi (< 6.6) but cannot be delegated into the sandbox. */
#ifdef MEMTRACK_BPF_LINKS
#define UPROBE_SEC "uprobe.multi"
#define URETPROBE_SEC "uretprobe.multi"
#else
#define UPROBE_SEC "uprobe"
#define URETPROBE_SEC "uretprobe"
#endif

/* Macros for common map definitions */
#define BPF_HASH_MAP(name, key_type, value_type, max_ents) \
    struct {                                               \
        __uint(type, BPF_MAP_TYPE_HASH);                   \
        __uint(max_entries, max_ents);                     \
        __type(key, key_type);                             \
        __type(value, value_type);                         \
    } name SEC(".maps")

#define BPF_ARRAY_MAP(name, value_type, max_ents) \
    struct {                                      \
        __uint(type, BPF_MAP_TYPE_ARRAY);         \
        __uint(max_entries, max_ents);            \
        __type(key, __u32);                       \
        __type(value, value_type);                \
    } name SEC(".maps")

#define BPF_RINGBUF(name, size)             \
    struct {                                \
        __uint(type, BPF_MAP_TYPE_RINGBUF); \
        __uint(max_entries, size);          \
    } name SEC(".maps")

/* PID namespace the userspace tracker resolves PIDs in. When set (ino != 0),
 * PIDs are read relative to this namespace so tracking works when the tracker
 * runs inside a PID namespace (e.g. the macro-agent sandbox) while eBPF sees
 * global PIDs. Zero means "use global PIDs" — the default outside a sandbox. */
const volatile __u64 target_pidns_dev = 0;
const volatile __u64 target_pidns_ino = 0;

/* Current task's PID in the configured namespace (or global when unset). */
static __always_inline __u32 current_pid(void) {
    if (target_pidns_ino == 0) {
        return bpf_get_current_pid_tgid() >> 32;
    }
    struct bpf_pidns_info nsinfo = {};
    if (bpf_get_ns_current_pid_tgid(target_pidns_dev, target_pidns_ino, &nsinfo, sizeof(nsinfo)) !=
        0) {
        return 0;
    }
    return nsinfo.tgid;
}

/* A task's PID as seen in the configured namespace (or global when unset).
 * Walks the task's pid struct: numbers[level].nr holds the PID at that namespace
 * depth, and numbers[level].ns identifies which namespace it is. */
static __always_inline __u32 task_ns_pid(struct task_struct* task) {
    if (target_pidns_ino == 0) {
        return BPF_CORE_READ(task, pid);
    }
    struct pid* thread_pid = BPF_CORE_READ(task, thread_pid);
    if (!thread_pid) {
        return 0;
    }
    unsigned int level = BPF_CORE_READ(thread_pid, level);
    /* Bound the level to satisfy the verifier and match the pid array. */
    if (level >= 4) {
        return 0;
    }
    struct upid* up = &thread_pid->numbers[level];
    __u64 ino = BPF_CORE_READ(up, ns, ns.inum);
    if (ino != target_pidns_ino) {
        return 0;
    }
    return BPF_CORE_READ(up, nr);
}

/* Map to store PIDs we're tracking */
BPF_HASH_MAP(tracked_pids, __u32, __u8, 10000);
/* Map to store parent-child relationships to detect hierarchy */
BPF_HASH_MAP(pids_ppid, __u32, __u32, 10000);
/* Ring buffer for sending events to userspace */
BPF_RINGBUF(events, 16 * 1024 * 1024);
/* Map to control whether tracking is enabled (0 = disabled, 1 = enabled) */
BPF_ARRAY_MAP(tracking_enabled, __u8, 1);
/*  Counter for events that couldn't be added to the ring buffer */
BPF_ARRAY_MAP(dropped_events, __u64, 1);

/* == Code that tracks process forks and execs == */

/* Helper to check if a PID or any of its ancestors should be tracked */
static __always_inline int is_tracked(__u32 pid) {
    /* Direct check */
    if (bpf_map_lookup_elem(&tracked_pids, &pid)) {
        return 1;
    }

/* Check parent recursively (up to 5 levels) */
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

/* Record a parent→child fork so the child inherits the parent's tracked state. */
static __always_inline void follow_fork(__u32 parent_pid, __u32 child_pid) {
    if (parent_pid == 0 || child_pid == 0) {
        return;
    }
    if (is_tracked(parent_pid)) {
        __u8 marker = 1;
        bpf_map_update_elem(&tracked_pids, &child_pid, &marker, BPF_ANY);
        bpf_map_update_elem(&pids_ppid, &child_pid, &parent_pid, BPF_ANY);
    }
}

#ifdef MEMTRACK_BPF_LINKS
/* tp_btf attaches via bpf(), so a delegated BPF token can authorize it inside
 * the sandbox. Reads task_struct pointers, resolving PIDs in the tracker's
 * namespace. */
SEC("tp_btf/sched_process_fork")
int BPF_PROG(tracepoint_sched_fork, struct task_struct* parent, struct task_struct* child) {
    follow_fork(task_ns_pid(parent), task_ns_pid(child));
    return 0;
}
#else
/* Classic perf tracepoint for kernels without (or runs without) a BPF token.
 * The raw tracepoint context carries global PIDs directly; without a token we
 * never run in a PID namespace, so global PIDs are correct. */
SEC("tracepoint/sched/sched_process_fork")
int tracepoint_sched_fork(struct trace_event_raw_sched_process_fork* ctx) {
    follow_fork(ctx->parent_pid, ctx->child_pid);
    return 0;
}
#endif

/* == Helper functions for the allocation tracking == */

/* Helper to check if tracking is currently enabled */
static __always_inline int is_enabled(void) {
    __u32 key = 0;
    __u8* enabled = bpf_map_lookup_elem(&tracking_enabled, &key);

    /* Default to enabled if map not initialized */
    if (!enabled) {
        return 1;
    }

    return *enabled;
}

/* Helper to store parameter value in map for tracking between entry and return
 */
static __always_inline int store_param(void* map, __u64 value) {
    /* Key by the global tid (stable, unique per thread across the entry/exit
     * pair); gate on the namespace-relative PID that the tracker registered. */
    __u64 tid = bpf_get_current_pid_tgid();
    if (is_tracked(current_pid())) {
        bpf_map_update_elem(map, &tid, &value, BPF_ANY);
    }
    return 0;
}

/* Helper to take parameter value from map (lookup and delete) */
static __always_inline __u64* take_param(void* map) {
    __u64 tid = bpf_get_current_pid_tgid();
    __u64* value = bpf_map_lookup_elem(map, &tid);
    if (value) {
        bpf_map_delete_elem(map, &tid);
    }
    return value;
}

/* Macro to handle common event submission boilerplate
 * Usage: SUBMIT_EVENT(event_type, { e->data.foo = bar; })
 */
#define SUBMIT_EVENT(evt_type, fill_data)                               \
    {                                                                   \
        __u64 tid = bpf_get_current_pid_tgid();                         \
        __u32 pid = current_pid();                                      \
                                                                        \
        if (!is_tracked(pid) || !is_enabled()) {                        \
            return 0;                                                   \
        }                                                               \
                                                                        \
        struct event* e = bpf_ringbuf_reserve(&events, sizeof(*e), 0);  \
        if (!e) {                                                       \
            __u32 zero = 0;                                             \
            __u64* drops = bpf_map_lookup_elem(&dropped_events, &zero); \
            if (drops) {                                                \
                __sync_fetch_and_add(drops, 1);                         \
            }                                                           \
            return 0;                                                   \
        }                                                               \
                                                                        \
        e->header.timestamp = bpf_ktime_get_ns();                       \
        e->header.pid = pid;                                            \
        e->header.tid = tid & 0xFFFFFFFF;                               \
        e->header.event_type = evt_type;                                \
                                                                        \
        fill_data;                                                      \
                                                                        \
        bpf_ringbuf_submit(e, 0);                                       \
        return 0;                                                       \
    }

/* Helper to submit an allocation event (malloc, calloc) */
static __always_inline int submit_alloc_event(__u64 size, __u64 addr) {
    SUBMIT_EVENT(EVENT_TYPE_MALLOC, {
        e->data.alloc.addr = addr;
        e->data.alloc.size = size;
    });
}

/* Helper to submit an aligned allocation event (aligned_alloc, memalign) */
static __always_inline int submit_aligned_alloc_event(__u64 size, __u64 addr) {
    SUBMIT_EVENT(EVENT_TYPE_ALIGNED_ALLOC, {
        e->data.alloc.addr = addr;
        e->data.alloc.size = size;
    });
}

/* Helper to submit a calloc event */
static __always_inline int submit_calloc_event(__u64 size, __u64 addr) {
    SUBMIT_EVENT(EVENT_TYPE_CALLOC, {
        e->data.alloc.addr = addr;
        e->data.alloc.size = size;
    });
}

/* Helper to submit a free event */
static __always_inline int submit_free_event(__u64 addr) {
    SUBMIT_EVENT(EVENT_TYPE_FREE, { e->data.free.addr = addr; });
}

/* Helper to submit a realloc event with both old and new addresses */
static __always_inline int submit_realloc_event(__u64 old_addr, __u64 new_addr, __u64 size) {
    SUBMIT_EVENT(EVENT_TYPE_REALLOC, {
        e->data.realloc.old_addr = old_addr;
        e->data.realloc.new_addr = new_addr;
        e->data.realloc.size = size;
    });
}

/* Helper to submit a memory mapping event (mmap, munmap, brk) */
static __always_inline int submit_mmap_event(__u64 addr, __u64 size, __u8 event_type) {
    SUBMIT_EVENT(event_type, {
        e->data.mmap.addr = addr;
        e->data.mmap.size = size;
    });
}

/* Macro to generate uprobe/uretprobe pairs for allocation functions with 1 argument */
#define UPROBE_ARG_RET(name, arg_expr, submit_block)                                      \
    BPF_HASH_MAP(name##_arg, __u64, __u64, 10000);                                        \
    SEC(UPROBE_SEC)                                                                       \
    int uprobe_##name(struct pt_regs* ctx) { return store_param(&name##_arg, arg_expr); } \
    SEC(URETPROBE_SEC)                                                                    \
    int uretprobe_##name(struct pt_regs* ctx) {                                           \
        __u64* arg_ptr = take_param(&name##_arg);                                         \
        if (!arg_ptr) {                                                                   \
            return 0;                                                                     \
        }                                                                                 \
        __u64 ret_val = PT_REGS_RC(ctx);                                                  \
        if (ret_val == 0) {                                                               \
            return 0;                                                                     \
        }                                                                                 \
        __u64 arg0 = *arg_ptr;                                                            \
        submit_block;                                                                     \
    }

/* Macro for simple return value only functions like free */
#define UPROBE_RET(name, arg_expr, submit_block) \
    SEC(UPROBE_SEC)                              \
    int uprobe_##name(struct pt_regs* ctx) {     \
        __u64 arg0 = arg_expr;                   \
        if (arg0 == 0) {                         \
            return 0;                            \
        }                                        \
        submit_block;                            \
    }

/* Macro to generate uprobe/uretprobe pairs for functions with 2 arguments */
#define UPROBE_ARGS_RET(name, arg0_expr, arg1_expr, submit_block)             \
    struct name##_args_t {                                                    \
        __u64 arg0;                                                           \
        __u64 arg1;                                                           \
    };                                                                        \
    BPF_HASH_MAP(name##_args, __u64, struct name##_args_t, 10000);            \
    SEC(UPROBE_SEC)                                                           \
    int uprobe_##name(struct pt_regs* ctx) {                                  \
        __u64 tid = bpf_get_current_pid_tgid();                               \
        __u32 pid = tid >> 32;                                                \
                                                                              \
        if (!is_tracked(pid)) {                                               \
            return 0;                                                         \
        }                                                                     \
                                                                              \
        struct name##_args_t args = {.arg0 = arg0_expr, .arg1 = arg1_expr};   \
                                                                              \
        bpf_map_update_elem(&name##_args, &tid, &args, BPF_ANY);              \
        return 0;                                                             \
    }                                                                         \
    SEC(URETPROBE_SEC)                                                        \
    int uretprobe_##name(struct pt_regs* ctx) {                               \
        __u64 tid = bpf_get_current_pid_tgid();                               \
        struct name##_args_t* args = bpf_map_lookup_elem(&name##_args, &tid); \
                                                                              \
        if (!args) {                                                          \
            return 0;                                                         \
        }                                                                     \
                                                                              \
        struct name##_args_t a = *args;                                       \
        bpf_map_delete_elem(&name##_args, &tid);                              \
                                                                              \
        __u64 ret_val = PT_REGS_RC(ctx);                                      \
        if (ret_val == 0) {                                                   \
            return 0;                                                         \
        }                                                                     \
                                                                              \
        __u64 arg0 = a.arg0;                                                  \
        __u64 arg1 = a.arg1;                                                  \
        submit_block;                                                         \
    }

/* malloc: allocates with size parameter */
UPROBE_ARG_RET(malloc, PT_REGS_PARM1(ctx), { return submit_alloc_event(arg0, ret_val); })

/* free: deallocates by address */
UPROBE_RET(free, PT_REGS_PARM1(ctx), { return submit_free_event(arg0); })

/* calloc: allocates with nmemb * size */
UPROBE_ARG_RET(calloc, PT_REGS_PARM1(ctx) * PT_REGS_PARM2(ctx),
               { return submit_calloc_event(arg0, ret_val); })

/* realloc: reallocates with old_addr and new size */
UPROBE_ARGS_RET(realloc, PT_REGS_PARM2(ctx), PT_REGS_PARM1(ctx),
                { return submit_realloc_event(arg1, ret_val, arg0); })

/* aligned_alloc: allocates with alignment and size */
UPROBE_ARG_RET(aligned_alloc, PT_REGS_PARM2(ctx),
               { return submit_aligned_alloc_event(arg0, ret_val); })

/* memalign: allocates with alignment and size (legacy interface) */
UPROBE_ARG_RET(memalign, PT_REGS_PARM2(ctx), { return submit_aligned_alloc_event(arg0, ret_val); })

/* Map to store mmap parameters between entry and return */
struct mmap_args {
    __u64 addr;
    __u64 len;
};

BPF_HASH_MAP(mmap_temp, __u64, struct mmap_args, 10000);

SEC("tracepoint/syscalls/sys_enter_mmap")
int tracepoint_sys_enter_mmap(struct trace_event_raw_sys_enter* ctx) {
    __u64 tid = bpf_get_current_pid_tgid();
    __u32 pid = tid >> 32;

    if (is_tracked(pid)) {
        struct mmap_args args = {0};

        /* mmap(addr, len, prot, flags, fd, offset)
         * We care about addr (can be 0 for kernel choice) and len */
        args.addr = ctx->args[0];
        args.len = ctx->args[1];

        bpf_map_update_elem(&mmap_temp, &tid, &args, BPF_ANY);
    }

    return 0;
}

SEC("tracepoint/syscalls/sys_exit_mmap")
int tracepoint_sys_exit_mmap(struct trace_event_raw_sys_exit* ctx) {
    struct mmap_args* args = (struct mmap_args*)take_param(&mmap_temp);
    if (!args) {
        return 0;
    }

    __s64 ret = ctx->ret;
    if (ret <= 0) {
        return 0;
    }

    return submit_mmap_event((__u64)ret, args->len, EVENT_TYPE_MMAP);
}

/* munmap tracking */
SEC("tracepoint/syscalls/sys_enter_munmap")
int tracepoint_sys_enter_munmap(struct trace_event_raw_sys_enter* ctx) {
    __u64 addr = ctx->args[0];
    __u64 len = ctx->args[1];

    if (addr == 0 || len == 0) {
        return 0;
    }

    return submit_mmap_event(addr, len, EVENT_TYPE_MUNMAP);
}

/* brk tracking - adjusts the program break (heap boundary) */
BPF_HASH_MAP(brk_temp, __u64, __u64, 10000);

SEC("tracepoint/syscalls/sys_enter_brk")
int tracepoint_sys_enter_brk(struct trace_event_raw_sys_enter* ctx) {
    __u64 tid = bpf_get_current_pid_tgid();
    __u32 pid = tid >> 32;

    if (is_tracked(pid)) {
        /* brk(addr) - if addr is 0, just queries current break */
        __u64 requested_brk = ctx->args[0];
        bpf_map_update_elem(&brk_temp, &tid, &requested_brk, BPF_ANY);
    }

    return 0;
}

SEC("tracepoint/syscalls/sys_exit_brk")
int tracepoint_sys_exit_brk(struct trace_event_raw_sys_exit* ctx) {
    __u64* requested_brk = take_param(&brk_temp);
    if (!requested_brk) {
        return 0;
    }

    __u64 new_brk = ctx->ret;
    __u64 req_brk = *requested_brk;

    if (req_brk == 0 || new_brk <= 0) {
        return 0;
    }

    return submit_mmap_event(new_brk, 0, EVENT_TYPE_BRK);
}
