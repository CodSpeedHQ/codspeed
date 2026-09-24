#ifndef __EVENT_H__
#define __EVENT_H__

#define EVENT_TYPE_MALLOC 1
#define EVENT_TYPE_FREE 2
#define EVENT_TYPE_CALLOC 3
#define EVENT_TYPE_REALLOC 4
#define EVENT_TYPE_ALIGNED_ALLOC 5
#define EVENT_TYPE_FORK 6
#define EVENT_TYPE_EXEC 7
#define EVENT_TYPE_EXIT 8
#define EVENT_TYPE_RSS 9
#define EVENT_TYPE_RMAP 10

/* Largest user-stack copy one definition can carry. Every capture reserves
 * header + budget in the ring up front, so this bounds ring space held per
 * in-flight capture, per-capture copy cost, and the verifier work per load
 * (which scales with the frozen budget). The kernel itself allows records up
 * to the ring size; this is a policy cap. */
#define MEMTRACK_MAX_STACK_COPY (32 * 1024)

/* Registers, indexed by the capturing architecture's DWARF register number
 * (x86_64: 0=rax .. 7=rsp, 8..15=r8-r15, 16=rip; aarch64: 0..30=x0-x30,
 * 31=sp, 32=pc). Slots the architecture does not define stay zero. An offline
 * DWARF unwinder needs the callee-saved ones to evaluate CFA rules, not just
 * ip/sp/bp. */
#define MEMTRACK_STACK_REGS 33

#define MEMTRACK_STACK_COUNTER_COPY_FAILED 0
#define MEMTRACK_STACK_COUNTER_HASH_MAP_FULL 1
/* bpf_get_stackid() has several negative outcomes (no user callchain,
 * hash-bucket collision, or no free bucket), so this counts only missing ids. */
#define MEMTRACK_STACK_COUNTER_STACKID_FAILED 2
#define MEMTRACK_STACK_COUNTER_TRUNCATED 3
#define MEMTRACK_STACK_COUNTER_RING_FULL 4
/* Delta encoding could not get a reference slot and fell back to a raw record. */
#define MEMTRACK_STACK_COUNTER_DELTA_FALLBACK 5
#define MEMTRACK_STACK_COUNTER_COUNT 6

struct stack_regs {
    uint64_t reg[MEMTRACK_STACK_REGS];
};

/* Both stack ring record layouts start with `kind` so the consumer can
 * dispatch on it; raw and delta records share one ring. */
#define STACK_RECORD_RAW 1
#define STACK_RECORD_DELTA 2

/* Raw record: fixed header followed by `copy_len` bytes read upward from `sp`. */
struct stack_header {
    uint32_t kind;     /* STACK_RECORD_RAW */
    uint32_t copy_len;
    uint64_t hash;
    uint64_t timestamp; /* monotonic time in nanoseconds (CLOCK_MONOTONIC) */
    int64_t stackid;    /* bpf_get_stackid() result; negative means unavailable */
    uint64_t sp;        /* user stack pointer the copy starts at */
    uint32_t pid;
    uint32_t tid;
    uint8_t truncated; /* the copy hit the size cap */
    uint8_t _pad[7];
    struct stack_regs regs;
};

/* Delta record. The stack is expressed as an XOR against the previous record
 * emitted for the same tid (the reference), aligned by absolute address:
 * word i of this copy (bytes [8i, 8i+8) above `sp`) pairs with reference word
 * j = i + (sp - ref.sp) / 8, or with 0 when j is outside the reference copy.
 * A keyframe (ref_hash == 0) encodes against an all-zero, empty reference,
 * so the same layout carries a plain sparse copy.
 *
 *   header
 *   u64 reg literal        x popcount(regs_mask)          (ascending register)
 *   for each set bit g of group_mask, ascending:
 *       u64 word_bitmap    bit k set <=> delta word 64g+k != 0
 *       u64 literal        x popcount(word_bitmap)        (ascending word)
 *
 * A group is 64 words (512 bytes). Groups whose delta is all zero are omitted
 * and have their group_mask bit clear. `hash` covers the reconstructed raw
 * bytes and uses the same function as the raw record, so the consumer can
 * check that it decoded against the right reference.
 */
#define MEMTRACK_STACK_GROUP_WORDS 64
#define MEMTRACK_STACK_MAX_WORDS (MEMTRACK_MAX_STACK_COPY / 8)
#define MEMTRACK_STACK_MAX_GROUPS (MEMTRACK_STACK_MAX_WORDS / MEMTRACK_STACK_GROUP_WORDS)
#define MEMTRACK_STACK_DELTA_MAX_PAYLOAD \
    (MEMTRACK_STACK_REGS * 8 + MEMTRACK_STACK_MAX_GROUPS * 8 + MEMTRACK_MAX_STACK_COPY)

#define STACK_DELTA_FLAG_TRUNCATED 1

struct stack_delta_header {
    uint32_t kind;        /* STACK_RECORD_DELTA */
    uint32_t copy_len;    /* reconstructed raw byte count, multiple of 512 */
    uint64_t hash;        /* hash of the reconstructed raw bytes */
    uint64_t ref_hash;    /* hash of the reference record; 0 on a keyframe */
    uint64_t timestamp;   /* monotonic time in nanoseconds (CLOCK_MONOTONIC) */
    int64_t stackid;      /* bpf_get_stackid() result; negative means unavailable */
    uint64_t sp;          /* user stack pointer the copy starts at */
    uint32_t pid;
    uint32_t tid;
    uint32_t payload_len; /* bytes following this header */
    uint8_t flags;        /* STACK_DELTA_FLAG_* */
    uint8_t _pad[3];
    uint64_t group_mask;  /* bit g set <=> group g present in the payload */
    uint64_t regs_mask;   /* bit r set <=> register r literal present */
};

/* Common header shared by all event types */
struct event_header {
    uint8_t event_type; /* See EVENT_TYPE_* constants above */
    uint64_t timestamp; /* monotonic time in nanoseconds (CLOCK_MONOTONIC) */
    uint32_t pid;
    uint32_t tid;
};

/* Tagged union event structure */
struct event {
    struct event_header header;
    union {
        /* Allocation events (malloc, calloc, aligned_alloc) */
        struct {
            uint64_t addr;       /* address returned */
            uint64_t size;       /* size requested */
            uint64_t stack_hash; /* caller stack identity; 0 = not captured */
        } alloc;

        /* Deallocation event (free) */
        struct {
            uint64_t addr;       /* address to free */
            uint64_t stack_hash; /* caller stack identity; 0 = not captured */
        } free;

        /* Reallocation event - includes both old and new addresses */
        struct {
            uint64_t old_addr;   /* previous address (can be NULL) */
            uint64_t new_addr;   /* new address returned */
            uint64_t size;       /* new size requested */
            uint64_t stack_hash; /* caller stack identity; 0 = not captured */
        } realloc;

        /* Process lifecycle events (fork carries the parent; exec/exit have no payload) */
        struct {
            uint32_t parent_pid;
        } fork;

        struct {
            int32_t member;
            uint64_t size;
        } rss;

        struct {
            int32_t member; /* MM_* counter index */
            int64_t delta;
            uint64_t addr;
        } rmap;
    } data;
};

/* Request from the exec-mapping watcher to the userspace attach worker */
struct attach_request {
    uint32_t pid;
    uint64_t dev; /* kernel s_dev encoding: (major << 20) | minor */
    uint64_t ino;
};

#endif /* __EVENT_H__ */
