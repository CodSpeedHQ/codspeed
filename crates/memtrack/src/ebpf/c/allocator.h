#ifndef __ALLOCATOR_H__
#define __ALLOCATOR_H__

#include "utils/event_helpers.h"
#include "utils/map_helpers.h"
#include "utils/process_tracking.h"

#define UPROBE_ARG_RET(name, arg_expr, submit_block)                                      \
    BPF_HASH_MAP(name##_arg, __u64, __u64, 10000);                                        \
    SEC("uprobe")                                                                         \
    int uprobe_##name(struct pt_regs* ctx) { return store_param(&name##_arg, arg_expr); } \
    SEC("uretprobe")                                                                      \
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

#define UPROBE_RET(name, arg_expr, submit_block) \
    SEC("uprobe")                                \
    int uprobe_##name(struct pt_regs* ctx) {     \
        __u64 arg0 = arg_expr;                   \
        if (arg0 == 0) {                         \
            return 0;                            \
        }                                        \
        submit_block;                            \
    }

#define UPROBE_ARGS_RET(name, arg0_expr, arg1_expr, submit_block)             \
    struct name##_args_t {                                                    \
        __u64 arg0;                                                           \
        __u64 arg1;                                                           \
    };                                                                        \
    BPF_HASH_MAP(name##_args, __u64, struct name##_args_t, 10000);            \
    SEC("uprobe")                                                             \
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
    SEC("uretprobe")                                                          \
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

UPROBE_ARG_RET(malloc, PT_REGS_PARM1(ctx), { return submit_alloc_event(arg0, ret_val); })

UPROBE_RET(free, PT_REGS_PARM1(ctx), { return submit_free_event(arg0); })

UPROBE_ARG_RET(calloc, PT_REGS_PARM1(ctx) * PT_REGS_PARM2(ctx),
               { return submit_calloc_event(arg0, ret_val); })

UPROBE_ARGS_RET(realloc, PT_REGS_PARM2(ctx), PT_REGS_PARM1(ctx),
                { return submit_realloc_event(arg1, ret_val, arg0); })

UPROBE_ARG_RET(aligned_alloc, PT_REGS_PARM2(ctx),
               { return submit_aligned_alloc_event(arg0, ret_val); })

UPROBE_ARG_RET(memalign, PT_REGS_PARM2(ctx), { return submit_aligned_alloc_event(arg0, ret_val); })

/*
 * posix_memalign(void** memptr, size_t alignment, size_t size)
 *
 * Unlike memalign/aligned_alloc, it returns int 0 on SUCCESS (nonzero errno on
 * failure) and delivers the pointer through the memptr out-parameter rather
 * than the return register. So the size lives in PARM3 (not PARM2), success is
 * ret == 0 (not a non-NULL return), and the address must be read back from
 * *memptr once the call returns.
 */
struct posix_memalign_args_t {
    __u64 memptr;
    __u64 size;
};
BPF_HASH_MAP(posix_memalign_args, __u64, struct posix_memalign_args_t, 10000);

SEC("uprobe")
int uprobe_posix_memalign(struct pt_regs* ctx) {
    __u64 tid = bpf_get_current_pid_tgid();
    __u32 pid = tid >> 32;
    if (!is_tracked(pid)) {
        return 0;
    }

    struct posix_memalign_args_t args = {.memptr = PT_REGS_PARM1(ctx), .size = PT_REGS_PARM3(ctx)};
    bpf_map_update_elem(&posix_memalign_args, &tid, &args, BPF_ANY);
    return 0;
}

SEC("uretprobe")
int uretprobe_posix_memalign(struct pt_regs* ctx) {
    __u64 tid = bpf_get_current_pid_tgid();
    struct posix_memalign_args_t* args = bpf_map_lookup_elem(&posix_memalign_args, &tid);
    if (!args) {
        return 0;
    }

    struct posix_memalign_args_t a = *args;
    bpf_map_delete_elem(&posix_memalign_args, &tid);

    if (PT_REGS_RC(ctx) != 0) {
        return 0;
    }

    __u64 addr = 0;
    if (bpf_probe_read_user(&addr, sizeof(addr), (void*)a.memptr) != 0 || addr == 0) {
        return 0;
    }

    return submit_aligned_alloc_event(a.size, addr);
}

struct mmap_args {
    __u64 addr;
    __u64 len;
};

BPF_HASH_MAP(mmap_temp, __u64, struct mmap_args, 10000);

static __always_inline void store_mmap_args(__u64 addr, __u64 len) {
    __u64 tid = bpf_get_current_pid_tgid();
    __u32 pid = tid >> 32;
    if (is_tracked(pid)) {
        struct mmap_args args = {.addr = addr, .len = len};
        bpf_map_update_elem(&mmap_temp, &tid, &args, BPF_ANY);
    }
}

SEC("tracepoint/syscalls/sys_enter_mmap")
int tracepoint_sys_enter_mmap(struct trace_event_raw_sys_enter* ctx) {
    store_mmap_args(ctx->args[0], ctx->args[1]);
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

SEC("tracepoint/syscalls/sys_enter_munmap")
int tracepoint_sys_enter_munmap(struct trace_event_raw_sys_enter* ctx) {
    __u64 addr = ctx->args[0];
    __u64 len = ctx->args[1];

    if (addr == 0 || len == 0) {
        return 0;
    }

    return submit_mmap_event(addr, len, EVENT_TYPE_MUNMAP);
}

BPF_HASH_MAP(brk_temp, __u64, __u64, 10000);

SEC("tracepoint/syscalls/sys_enter_brk")
int tracepoint_sys_enter_brk(struct trace_event_raw_sys_enter* ctx) {
    store_param(&brk_temp, ctx->args[0]);
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

#endif /* __ALLOCATOR_H__ */
