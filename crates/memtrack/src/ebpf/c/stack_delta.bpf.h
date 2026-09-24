#ifndef __STACK_DELTA_BPF_H__
#define __STACK_DELTA_BPF_H__

/* Delta-encoded stack records (layout in event.h). Included from
 * stack_capture.bpf.h after the shared read/hash helpers.
 *
 * Each tid keeps the last emitted copy as the reference and a staging slot
 * for the capture in flight; the slots swap only after the record reached
 * the ring, so the reference always matches what userspace decoded last.
 *
 * The encoder is branch-free per word: every literal is stored
 * speculatively and the output index advances by 8 only when the delta is
 * nonzero. With one verifier state per word the cost stays linear in the
 * copy budget. Values that clang would otherwise turn back into
 * compare-and-branch or wide expression trees go through barrier_var().
 */

const volatile __u8 stack_compression_enabled = 0;

struct stack_ref_slot {
    __u64 sp;
    __u64 hash;
    __u32 copy_len;
    __u32 _pad;
    __u64 regs[MEMTRACK_STACK_REGS];
    __u8 data[MEMTRACK_MAX_STACK_COPY];
};

struct stack_ref {
    __u32 cur; /* slot index holding the reference; 1 - cur is staging */
    struct stack_ref_slot slot[2];
    /* +8: the last speculative literal store may land past the payload. */
    __u8 out[sizeof(struct stack_delta_header) + MEMTRACK_STACK_DELTA_MAX_PAYLOAD + 8];
};

/* Userspace shrinks this to one entry when compression is off. */
BPF_LRU_HASH_MAP(stack_refs, __u32, struct stack_ref, 512);

/* Map helpers need the insert value in map memory; the value is too large for
 * the BPF stack. */
static const struct stack_ref zero_stack_ref = {};

static __always_inline struct stack_ref* stack_ref_get(__u32 tid) {
    struct stack_ref* ref = bpf_map_lookup_elem(&stack_refs, &tid);
    if (ref) {
        return ref;
    }
    bpf_map_update_elem(&stack_refs, &tid, &zero_stack_ref, BPF_NOEXIST);
    return bpf_map_lookup_elem(&stack_refs, &tid);
}

/* BPF has no set-on-condition instruction, so any `x != 0` or `a < b` that
 * reaches the backend is lowered to a branch, and the verifier then forks a
 * state per word. These helpers keep the predicates as arithmetic; the
 * barriers stop clang from recognising them as comparisons. */
static __always_inline __u64 nonzero_bit(__u64 x) {
    __u64 t = x | (0 - x);
    barrier_var(t);
    return t >> 63;
}

/* All ones when j < limit, treating j >= 2^63 (a wrapped negative index) as
 * out of range; zero otherwise. Requires limit < 2^63. Both operands are
 * opaque so clang cannot prove they are booleans and turn the AND into a
 * select. */
static __always_inline __u64 below_mask(__u64 j, __u64 limit) {
    __u64 not_negative = ~j;
    barrier_var(not_negative);
    __u64 diff = j - limit;
    barrier_var(diff);
    return (__u64)((__s64)not_negative >> 63) & (__u64)((__s64)diff >> 63);
}

static __always_inline __u64 capture_stack_delta(struct pt_regs* ctx, struct task_ids ids,
                                                 struct stack_ref* ref) {
    __u32 cur = ref->cur & 1;
    struct stack_ref_slot* prev = &ref->slot[cur];
    struct stack_ref_slot* stg = &ref->slot[cur ^ 1];
    __u8* out = ref->out;

    /* Hash lanes live in the output scratch; the header overwrites them later. */
    __u64* lanes = (__u64*)out;
    __u64 sp = PT_REGS_SP(ctx);
    __u32 got = read_stack_chunks(stg->data, lanes, sp);
    if (got == 0) {
        bump_stack_counter(MEMTRACK_STACK_COUNTER_COPY_FAILED);
        memtrack_check_ring_pressure(&stacks, ids.tgid);
        return 0;
    }

    stg->hash = fnv64_finish(lanes, got);
    stg->sp = sp;
    stg->copy_len = got;
    fill_stack_regs((struct stack_regs*)stg->regs, ctx);
    __u64 hash = stg->hash;

    long gate_result =
        bpf_map_update_elem(&seen_stack_hashes, &stg->hash, &seen_stack_marker, BPF_NOEXIST);
    if (gate_result == -17) { /* -EEXIST */
        memtrack_check_ring_pressure(&stacks, ids.tgid);
        return hash;
    }

    /* Every distinct verifier state entering the encoder loop walks all of it
     * again, so the loop must see one state. The read loop exits with a
     * different constant `got` per chunk; reloading it from map memory makes
     * it an unknown scalar the exit paths share. All other branches
     * (counters, stackid, flags) run after the loop for the same reason. */
    got = *(volatile __u32*)&stg->copy_len;

    /* A keyframe encodes against the zeroed slot: ref_words is 0 and every
     * reference word is masked. */
    __u64 ref_words = prev->copy_len / 8;
    __u64 shift = (__u64)((__s64)(sp - prev->sp) >> 3);

    __u32 o = sizeof(struct stack_delta_header);
    __u64 regs_mask = 0;
#pragma unroll
    for (__u32 r = 0; r < MEMTRACK_STACK_REGS; r++) {
        __u64 d = stg->regs[r] ^ prev->regs[r];
        __u64 nz = nonzero_bit(d);
        *(__u64*)(out + o) = d;
        o += (__u32)(nz << 3);
        regs_mask |= nz << r;
        barrier_var(regs_mask);
    }

    const __u64* cur_words = (const __u64*)stg->data;
    const __u64* ref_data = (const __u64*)prev->data;
    __u64 group_mask = 0;

    /* Same frozen bound as the read loop; `got` ends it early. */
#pragma clang loop unroll(disable)
    for (__u32 off = 0; off + STACK_COPY_CHUNK <= stack_copy_budget; off += STACK_COPY_CHUNK) {
        if (off >= got) {
            break;
        }

        __u32 base = off / 8;
        __u64 bitmap = 0;
        __u32 bitmap_pos = o;
        /* Literal offsets are tracked relative to the group so the verifier
         * keeps a tight [0, 512] range instead of a difference of two
         * independently bounded indices. */
        __u32 lit = 8;
#pragma unroll
        for (__u32 k = 0; k < MEMTRACK_STACK_GROUP_WORDS; k++) {
            __u64 j = (__u64)(base + k) + shift;
            __u64 refw = ref_data[j & (MEMTRACK_STACK_MAX_WORDS - 1)] & below_mask(j, ref_words);
            __u64 d = cur_words[base + k] ^ refw;
            __u64 nz = nonzero_bit(d);
            *(__u64*)(out + bitmap_pos + lit) = d;
            lit += (__u32)(nz << 3);
            bitmap |= nz << k;
            barrier_var(bitmap);
        }

        *(__u64*)(out + bitmap_pos) = bitmap;
        __u64 any = nonzero_bit(bitmap);
        group_mask |= any << (off / STACK_COPY_CHUNK);
        /* An all-zero group contributes nothing; its speculative bitmap and
         * literal stores are overwritten by the next group or ignored. */
        o = bitmap_pos + (lit & (__u32)(0 - any));
        barrier_var(o);
    }

    __u8 truncated = got >= stack_copy_budget;
    if (truncated) {
        bump_stack_counter(MEMTRACK_STACK_COUNTER_TRUNCATED);
    }
    if (gate_result != 0) {
        bump_stack_counter(MEMTRACK_STACK_COUNTER_HASH_MAP_FULL);
    }
    __s64 stackid = bpf_get_stackid(ctx, &stack_traces, BPF_F_USER_STACK);
    if (stackid < 0) {
        bump_stack_counter(MEMTRACK_STACK_COUNTER_STACKID_FAILED);
    }

    struct stack_delta_header* header = (struct stack_delta_header*)out;
    header->kind = STACK_RECORD_DELTA;
    header->copy_len = got;
    header->hash = hash;
    header->ref_hash = prev->hash;
    header->timestamp = bpf_ktime_get_ns();
    header->stackid = stackid;
    header->sp = sp;
    header->pid = ids.tgid;
    header->tid = ids.tid;
    header->payload_len = o - sizeof(struct stack_delta_header);
    header->flags = truncated ? STACK_DELTA_FLAG_TRUNCATED : 0;
    header->_pad[0] = 0;
    header->_pad[1] = 0;
    header->_pad[2] = 0;
    header->group_mask = group_mask;
    header->regs_mask = regs_mask;

    if (bpf_ringbuf_output(&stacks, out, o, 0) != 0) {
        bump_stack_counter(MEMTRACK_STACK_COUNTER_RING_FULL);
        /* Let the next capture of this stack emit it again. */
        if (gate_result == 0) {
            bpf_map_delete_elem(&seen_stack_hashes, &stg->hash);
        }
        memtrack_check_ring_pressure(&stacks, ids.tgid);
        return 0;
    }

    ref->cur = cur ^ 1;
    memtrack_check_ring_pressure(&stacks, ids.tgid);
    return hash;
}

#endif /* __STACK_DELTA_BPF_H__ */
