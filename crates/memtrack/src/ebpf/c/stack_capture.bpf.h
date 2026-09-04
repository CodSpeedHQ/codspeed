#ifndef __STACK_CAPTURE_BPF_H__
#define __STACK_CAPTURE_BPF_H__

#include "event.h"
#include "utils/map_helpers.h"
#include "utils/process_tracking.h"

/* Raw user-stack bytes and registers are emitted once per hash for offline
 * DWARF unwinding. Allocation events carry the hash; bpf_get_stackid() supplies
 * the frame-pointer fallback.
 *
 * Stack data changes may give one call path multiple hashes. Hashes may also
 * recur after LRU eviction in seen_stack_hashes, so consumers must tolerate
 * duplicate definitions.
 */

const volatile __u8 capture_stacks_enabled = 0;
const volatile __u32 stack_copy_budget = 4096;

#define STACK_TRACE_MAX_DEPTH 127
/* Captured lengths are rounded down to this granularity. */
#define STACK_COPY_CHUNK 512
#define FNV64_OFFSET 0xcbf29ce484222325ULL
#define FNV64_PRIME 0x00000100000001b3ULL

/* Frame-pointer fallback keyed by bpf_get_stackid(). */
struct {
    __uint(type, BPF_MAP_TYPE_STACK_TRACE);
    __uint(max_entries, 16384);
    __type(key, __u32);
    __uint(value_size, STACK_TRACE_MAX_DEPTH * sizeof(__u64));
} stack_traces SEC(".maps");

/* A separate ring keeps allocation events fixed-size. */
BPF_RINGBUF(stacks, 512 * 1024 * 1024);
BPF_LRU_HASH_MAP(seen_stack_hashes, __u64, __u8, 262144);
BPF_HASH_MAP(pending_stack_hash, __u64, __u64, 10000);
BPF_ARRAY_MAP(stack_counters, __u64, MEMTRACK_STACK_COUNTER_COUNT);

static __always_inline void bump_stack_counter(__u32 index) {
    __u64* slot = bpf_map_lookup_elem(&stack_counters, &index);
    if (slot) {
        __sync_fetch_and_add(slot, 1);
    }
}

/* 4-lane FNV-1a over one STACK_COPY_CHUNK worth of 8-byte words. Fixed-size,
 * unrolled so the verifier sees a bounded loop. */
static __always_inline void fnv64_hash_chunk(__u64 lanes[4], const __u64* words) {
#pragma unroll
    for (__u32 i = 0; i < STACK_COPY_CHUNK / 8; i += 4) {
        lanes[0] = (lanes[0] ^ words[i]) * FNV64_PRIME;
        lanes[1] = (lanes[1] ^ words[i + 1]) * FNV64_PRIME;
        lanes[2] = (lanes[2] ^ words[i + 2]) * FNV64_PRIME;
        lanes[3] = (lanes[3] ^ words[i + 3]) * FNV64_PRIME;
    }
}

#if defined(__TARGET_ARCH_x86)
static __always_inline void fill_stack_regs(struct stack_regs* out, struct pt_regs* ctx) {
    out->reg[0] = ctx->ax;
    out->reg[1] = ctx->dx;
    out->reg[2] = ctx->cx;
    out->reg[3] = ctx->bx;
    out->reg[4] = ctx->si;
    out->reg[5] = ctx->di;
    out->reg[6] = ctx->bp;
    out->reg[7] = ctx->sp;
    out->reg[8] = ctx->r8;
    out->reg[9] = ctx->r9;
    out->reg[10] = ctx->r10;
    out->reg[11] = ctx->r11;
    out->reg[12] = ctx->r12;
    out->reg[13] = ctx->r13;
    out->reg[14] = ctx->r14;
    out->reg[15] = ctx->r15;
    out->reg[16] = ctx->ip;
}
#elif defined(__TARGET_ARCH_arm64)
static __always_inline void fill_stack_regs(struct stack_regs* out, struct pt_regs* ctx) {
    struct user_pt_regs* uregs = (struct user_pt_regs*)ctx;
#pragma unroll
    for (int i = 0; i < 31; i++) {
        out->reg[i] = uregs->regs[i];
    }
    out->reg[31] = uregs->sp;
    out->reg[32] = uregs->pc;
}
#else
#error "stack capture needs a DWARF register mapping for this architecture"
#endif

static __always_inline __u64 capture_stack_inner(struct pt_regs* ctx, struct task_ids ids) {
    void* slot = bpf_ringbuf_reserve(&stacks, sizeof(struct stack_header) + stack_copy_budget, 0);
    if (!slot) {
        bump_stack_counter(MEMTRACK_STACK_COUNTER_RING_FULL);
        return 0;
    }

    __u64 sp = PT_REGS_SP(ctx);
    __u8* payload = (__u8*)slot + sizeof(struct stack_header);
    __u64 lanes[4] = {
        FNV64_OFFSET ^ 0,
        FNV64_OFFSET ^ 1,
        FNV64_OFFSET ^ 2,
        FNV64_OFFSET ^ 3,
    };
    __u32 got = 0;

    /* Chunked reads stop at the first unreadable stack region.
     * Loop bound is checked against stack_copy_budget (a frozen rodata constant)
     * so every slot access is provably in range. */
#pragma clang loop unroll(disable)
    for (__u32 off = 0; off + STACK_COPY_CHUNK <= stack_copy_budget; off += STACK_COPY_CHUNK) {
        if (bpf_probe_read_user(payload + off, STACK_COPY_CHUNK, (void*)(sp + off)) != 0) {
            break;
        }

        fnv64_hash_chunk(lanes, (const __u64*)(payload + off));
        got = off + STACK_COPY_CHUNK;
    }

    if (got == 0) {
        bpf_ringbuf_discard(slot, 0);
        bump_stack_counter(MEMTRACK_STACK_COUNTER_COPY_FAILED);
        return 0;
    }

    __u8 truncated = got >= stack_copy_budget;
    if (truncated) {
        bump_stack_counter(MEMTRACK_STACK_COUNTER_TRUNCATED);
    }

    __u64 hash =
        (((lanes[0] * FNV64_PRIME) ^ lanes[1]) * FNV64_PRIME ^ lanes[2]) * FNV64_PRIME ^ lanes[3];

    /* Length distinguishes a full copy from the same bytes as a truncated prefix.
     * Zero is reserved for allocation events without a stack. */
    hash = (hash ^ got) * FNV64_PRIME;
    if (hash == 0) {
        hash = FNV64_OFFSET;
    }

    __u8 marker = 1;
    long gate_result = bpf_map_update_elem(&seen_stack_hashes, &hash, &marker, BPF_NOEXIST);
    if (gate_result == -17) { /* -EEXIST */
        bpf_ringbuf_discard(slot, 0);
        return hash;
    }
    if (gate_result != 0) {
        /* Re-emit when deduplication is full so the hash remains resolvable. */
        bump_stack_counter(MEMTRACK_STACK_COUNTER_HASH_MAP_FULL);
    }

    __s64 stackid = bpf_get_stackid(ctx, &stack_traces, BPF_F_USER_STACK);
    if (stackid < 0) {
        bump_stack_counter(MEMTRACK_STACK_COUNTER_STACKID_FAILED);
    }

    struct stack_header* header = (struct stack_header*)slot;
    header->hash = hash;
    header->timestamp = bpf_ktime_get_ns();
    header->stackid = stackid;
    header->sp = sp;
    header->pid = ids.tgid;
    header->tid = ids.tid;
    header->copy_len = got;
    header->truncated = truncated;
    header->_pad[0] = 0;
    header->_pad[1] = 0;
    header->_pad[2] = 0;
    fill_stack_regs(&header->regs, ctx);

    bpf_ringbuf_submit(slot, 0);
    return hash;
}

static __always_inline __u64 capture_stack(struct pt_regs* ctx) {
    if (!capture_stacks_enabled || !is_enabled()) {
        return 0;
    }

    struct task_ids ids = current_task_ids();
    if (!is_tracked(ids.tgid)) {
        return 0;
    }

    return capture_stack_inner(ctx, ids);
}

static __always_inline void stash_stack_hash(__u64 hash) {
    if (hash == 0) {
        return;
    }

    __u64 tid = current_tid();
    bpf_map_update_elem(&pending_stack_hash, &tid, &hash, BPF_ANY);
}

static __always_inline __u64 take_stack_hash(void) {
    if (!capture_stacks_enabled) {
        return 0;
    }

    __u64 tid = current_tid();
    __u64* hash = bpf_map_lookup_elem(&pending_stack_hash, &tid);
    if (!hash) {
        return 0;
    }

    __u64 value = *hash;
    bpf_map_delete_elem(&pending_stack_hash, &tid);
    return value;
}

#endif /* __STACK_CAPTURE_BPF_H__ */
