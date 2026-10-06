#ifndef __STACK_CAPTURE_BPF_H__
#define __STACK_CAPTURE_BPF_H__

#include "event.h"
#include "utils/map_helpers.h"
#include "utils/pressure.bpf.h"
#include "utils/process_tracking.h"

/* Emit raw stack bytes and registers once per hash for offline DWARF unwinding.
 * Allocation events carry the hash; stackid provides the frame-pointer fallback.
 * Hashes may repeat after stack data changes or LRU eviction.
 */

const volatile __u8 capture_stacks_enabled = 0;
const volatile __u32 stack_copy_budget = 4096;

#define STACK_TRACE_MAX_DEPTH 127
#define STACK_COPY_CHUNK 512
#define FNV64_OFFSET 0xcbf29ce484222325ULL
#define FNV64_PRIME 0x00000100000001b3ULL

struct {
    __uint(type, BPF_MAP_TYPE_STACK_TRACE);
    __uint(max_entries, 16384);
    __type(key, __u32);
    __uint(value_size, STACK_TRACE_MAX_DEPTH * sizeof(__u64));
} stack_traces SEC(".maps");

/* A separate ring keeps allocation events fixed-size. */
BPF_RINGBUF(stacks, 512 * 1024 * 1024);
BPF_LRU_HASH_MAP(seen_stack_hashes, __u64, __u8, 262144);
/* seen_stack_hashes is a hash set: only keys matter, every value is this
 * marker. It lives in .rodata because map helpers reject a key and value that
 * both point into the same ring reservation. */
static const __u8 seen_stack_marker = 1;
BPF_HASH_MAP(pending_stack_hash, __u64, __u64, 10000);
BPF_ARRAY_MAP(stack_counters, __u64, MEMTRACK_STACK_COUNTER_COUNT);

static __always_inline void bump_stack_counter(__u32 index) {
    __u64* slot = bpf_map_lookup_elem(&stack_counters, &index);
    if (slot) {
        __sync_fetch_and_add(slot, 1);
    }
}

/* Matches the kernel's MAX_URETPROBE_DEPTH: deeper calls get no uretprobe. */
#define MAX_PENDING_URETPROBES 64

/* A pending uretprobe replaces its function's return address with the
 * [uprobes] trampoline. When a hooked allocator calls another one
 * (operator new -> malloc), the nested capture copies that trampoline, and
 * offline unwinding cannot get past it. Write the original return addresses
 * back from the task's pending return_instances, innermost first.
 *
 * Since v6.11 the kernel repairs perf and bpf_get_stackid callchains the same
 * way (fixup_uretprobe_trampoline_entries), but not raw bpf_probe_read_user
 * copies. It can match trampoline values because it works on an unwound
 * callchain. Raw stack bytes also hold stale trampoline words, so here only
 * the hijacked slot is patched: on x86, the entry sp recorded as ri->stack.
 * See https://github.com/torvalds/linux/commit/4a365eb8a6d9940e838739935f1ce21f1ec8e33f
 *
 * arm64 hijacks the link register, and the kernel does not track where the
 * callee spills it, so there is no exact slot to patch. */
static __always_inline void restore_uretprobe_return_addresses(__u8* bytes, __u32 len, __u64 sp) {
#if defined(__TARGET_ARCH_x86)
    struct task_struct* task = bpf_get_current_task_btf();
    struct return_instance* ri = BPF_CORE_READ(task, utask, return_instances);
    if (!ri) {
        return;
    }

    __u64 trampoline = BPF_CORE_READ(task, mm, uprobes_state.xol_area, vaddr);

#pragma clang loop unroll(disable)
    for (__u32 i = 0; i < MAX_PENDING_URETPROBES && ri; i++) {
        /* Slots below sp wrap to huge offsets and fail the budget check, which
         * also bounds the access for the verifier. */
        __u64 off = BPF_CORE_READ(ri, stack) - sp;
        if (off <= stack_copy_budget - sizeof(__u64) && off + sizeof(__u64) <= len) {
            /* An instance left behind by longjmp can point at a reused slot. */
            __u64* word = (__u64*)(bytes + off);
            if (*word == trampoline) {
                *word = BPF_CORE_READ(ri, orig_ret_vaddr);
            }
        }
        ri = BPF_CORE_READ(ri, next);
    }
#endif
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

/* Copy the user stack at sp in STACK_COPY_CHUNK reads, stopping at the first
 * unreadable chunk. Returns the bytes copied, a multiple of STACK_COPY_CHUNK.
 * The loop is bounded by stack_copy_budget (a frozen rodata constant) so every
 * write into the reservation is provably in range. */
static __always_inline __u32 copy_user_stack(__u8* bytes, __u64 sp) {
    __u32 got = 0;
#pragma clang loop unroll(disable)
    for (__u32 off = 0; off + STACK_COPY_CHUNK <= stack_copy_budget; off += STACK_COPY_CHUNK) {
        if (bpf_probe_read_user(bytes + off, STACK_COPY_CHUNK, (void*)(sp + off)) != 0) {
            break;
        }
        got = off + STACK_COPY_CHUNK;
    }
    return got;
}

/* Hash the first len bytes, one chunk at a time. The loop has the same budget
 * bound as the copy so the verifier accepts the reads; len ends it early. */
static __always_inline void hash_stack_bytes(__u64 lanes[4], const __u8* bytes, __u32 len) {
#pragma clang loop unroll(disable)
    for (__u32 off = 0; off + STACK_COPY_CHUNK <= stack_copy_budget; off += STACK_COPY_CHUNK) {
        if (off >= len) {
            break;
        }
        fnv64_hash_chunk(lanes, (const __u64*)(bytes + off));
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
        memtrack_check_ring_pressure(&stacks, ids.tgid);
        return 0;
    }

    /* Keep hashing scratch in the unpublished record. Large kprobe-family BPF
     * stacks may use per-CPU storage, which nested uprobes can overwrite. */
    struct stack_header* header = (struct stack_header*)slot;
    __u64* lanes = &header->hash;
    lanes[0] = FNV64_OFFSET ^ 0;
    lanes[1] = FNV64_OFFSET ^ 1;
    lanes[2] = FNV64_OFFSET ^ 2;
    lanes[3] = FNV64_OFFSET ^ 3;

    __u8* bytes = (__u8*)slot + sizeof(struct stack_header);
    __u32 got = copy_user_stack(bytes, PT_REGS_SP(ctx));

    if (got == 0) {
        bpf_ringbuf_discard(slot, 0);
        bump_stack_counter(MEMTRACK_STACK_COUNTER_COPY_FAILED);
        memtrack_check_ring_pressure(&stacks, ids.tgid);
        return 0;
    }

    /* Copy, patch, then hash. The patch can rewrite a word in any copied chunk,
     * so hashing waits for the whole copy. The hash must cover the
     * patched bytes: stacks that differ only in the hijacked return slot would
     * otherwise share a hash, and dedup would drop the second one. */
    restore_uretprobe_return_addresses(bytes, got, PT_REGS_SP(ctx));
    hash_stack_bytes(lanes, bytes, got);

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

    header->hash = hash;
    long gate_result =
        bpf_map_update_elem(&seen_stack_hashes, &header->hash, &seen_stack_marker, BPF_NOEXIST);
    if (gate_result == -17) { /* -EEXIST */
        bpf_ringbuf_discard(slot, 0);
        memtrack_check_ring_pressure(&stacks, ids.tgid);
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

    header->hash = hash;
    header->timestamp = bpf_ktime_get_ns();
    header->stackid = stackid;
    header->sp = PT_REGS_SP(ctx);
    header->pid = ids.tgid;
    header->tid = ids.tid;
    header->copy_len = got;
    header->truncated = truncated;
    header->_pad[0] = 0;
    header->_pad[1] = 0;
    header->_pad[2] = 0;
    fill_stack_regs(&header->regs, ctx);

    bpf_ringbuf_submit(slot, 0);
    memtrack_check_ring_pressure(&stacks, ids.tgid);
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
