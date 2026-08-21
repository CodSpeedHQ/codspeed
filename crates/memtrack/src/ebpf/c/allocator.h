#ifndef __ALLOCATOR_H__
#define __ALLOCATOR_H__

#include "utils/event_helpers.h"
#include "utils/map_helpers.h"
#include "utils/process_tracking.h"

#define UPROBE_ARG_RET(name, slot, arg_expr, submit_block) \
    SEC(UPROBE_SEC)                                        \
    int uprobe_##name(struct pt_regs* ctx) {               \
        return store_arg(slot, arg_expr);                  \
    }                                                      \
    SEC(URETPROBE_SEC)                                     \
    int uretprobe_##name(struct pt_regs* ctx) {            \
        struct memtrack_task_state* st = take_slot(slot);  \
        if (!st) {                                         \
            return 0;                                      \
        }                                                  \
        __u64 ret_val = PT_REGS_RC(ctx);                   \
        if (ret_val == 0) {                                \
            return 0;                                      \
        }                                                  \
        __u64 arg0 = st->arg0[slot];                       \
        submit_block;                                      \
    }

#define UPROBE_RET(name, arg_expr, submit_block) \
    SEC(UPROBE_SEC)                              \
    int uprobe_##name(struct pt_regs* ctx) {     \
        __u64 arg0 = arg_expr;                   \
        if (arg0 == 0) {                         \
            return 0;                            \
        }                                        \
        if (!tracked_state()) {                  \
            return 0;                            \
        }                                        \
        submit_block;                            \
    }

#define UPROBE_ARGS_RET(name, slot, arg0_expr, arg1_expr, submit_block) \
    SEC(UPROBE_SEC)                                                     \
    int uprobe_##name(struct pt_regs* ctx) {                            \
        return store_args(slot, arg0_expr, arg1_expr);                  \
    }                                                                   \
    SEC(URETPROBE_SEC)                                                  \
    int uretprobe_##name(struct pt_regs* ctx) {                         \
        struct memtrack_task_state* st = take_slot(slot);               \
        if (!st) {                                                      \
            return 0;                                                   \
        }                                                               \
        __u64 ret_val = PT_REGS_RC(ctx);                                \
        if (ret_val == 0) {                                             \
            return 0;                                                   \
        }                                                               \
        __u64 arg0 = st->arg0[slot];                                    \
        __u64 arg1 = st->arg1[slot];                                    \
        submit_block;                                                   \
    }

UPROBE_ARG_RET(malloc, SLOT_MALLOC, PT_REGS_PARM1(ctx),
               { return submit_alloc_event(arg0, ret_val); })

UPROBE_RET(free, PT_REGS_PARM1(ctx), { return submit_free_event(arg0); })

UPROBE_ARG_RET(calloc, SLOT_CALLOC, PT_REGS_PARM1(ctx) * PT_REGS_PARM2(ctx),
               { return submit_calloc_event(arg0, ret_val); })

UPROBE_ARGS_RET(realloc, SLOT_REALLOC, PT_REGS_PARM2(ctx), PT_REGS_PARM1(ctx),
                { return submit_realloc_event(arg1, ret_val, arg0); })

UPROBE_ARG_RET(aligned_alloc, SLOT_ALIGNED_ALLOC, PT_REGS_PARM2(ctx),
               { return submit_aligned_alloc_event(arg0, ret_val); })

UPROBE_ARG_RET(memalign, SLOT_MEMALIGN, PT_REGS_PARM2(ctx),
               { return submit_aligned_alloc_event(arg0, ret_val); })

/*
 * posix_memalign(void** memptr, size_t alignment, size_t size)
 *
 * Unlike memalign/aligned_alloc, it returns int 0 on SUCCESS (nonzero errno on
 * failure) and delivers the pointer through the memptr out-parameter rather
 * than the return register. So the size lives in PARM3 (not PARM2), success is
 * ret == 0 (not a non-NULL return), and the address must be read back from
 * *memptr once the call returns.
 */
SEC(UPROBE_SEC)
int uprobe_posix_memalign(struct pt_regs* ctx) {
    return store_args(SLOT_POSIX_MEMALIGN, PT_REGS_PARM1(ctx), PT_REGS_PARM3(ctx));
}

SEC(URETPROBE_SEC)
int uretprobe_posix_memalign(struct pt_regs* ctx) {
    struct memtrack_task_state* st = take_slot(SLOT_POSIX_MEMALIGN);
    if (!st) {
        return 0;
    }

    if (PT_REGS_RC(ctx) != 0) {
        return 0;
    }

    __u64 memptr = st->arg0[SLOT_POSIX_MEMALIGN];
    __u64 size = st->arg1[SLOT_POSIX_MEMALIGN];

    __u64 addr = 0;
    if (bpf_probe_read_user(&addr, sizeof(addr), (void*)memptr) != 0 || addr == 0) {
        return 0;
    }

    return submit_aligned_alloc_event(size, addr);
}

SEC("tracepoint/syscalls/sys_enter_mmap")
int tracepoint_sys_enter_mmap(struct trace_event_raw_sys_enter* ctx) {
    return store_args(SLOT_MMAP, ctx->args[0], ctx->args[1]);
}

SEC("tracepoint/syscalls/sys_exit_mmap")
int tracepoint_sys_exit_mmap(struct trace_event_raw_sys_exit* ctx) {
    struct memtrack_task_state* st = take_slot(SLOT_MMAP);
    if (!st) {
        return 0;
    }

    __s64 ret = ctx->ret;
    if (ret <= 0) {
        return 0;
    }

    return submit_mmap_event((__u64)ret, st->arg1[SLOT_MMAP], EVENT_TYPE_MMAP);
}

SEC("tracepoint/syscalls/sys_enter_munmap")
int tracepoint_sys_enter_munmap(struct trace_event_raw_sys_enter* ctx) {
    __u64 addr = ctx->args[0];
    __u64 len = ctx->args[1];

    if (addr == 0 || len == 0) {
        return 0;
    }

    if (!tracked_state()) {
        return 0;
    }

    return submit_mmap_event(addr, len, EVENT_TYPE_MUNMAP);
}

SEC("tracepoint/syscalls/sys_enter_brk")
int tracepoint_sys_enter_brk(struct trace_event_raw_sys_enter* ctx) {
    return store_arg(SLOT_BRK, ctx->args[0]);
}

SEC("tracepoint/syscalls/sys_exit_brk")
int tracepoint_sys_exit_brk(struct trace_event_raw_sys_exit* ctx) {
    struct memtrack_task_state* st = take_slot(SLOT_BRK);
    if (!st) {
        return 0;
    }

    __u64 new_brk = ctx->ret;
    __u64 req_brk = st->arg0[SLOT_BRK];

    if (req_brk == 0 || new_brk <= 0) {
        return 0;
    }

    return submit_mmap_event(new_brk, 0, EVENT_TYPE_BRK);
}

#endif /* __ALLOCATOR_H__ */
