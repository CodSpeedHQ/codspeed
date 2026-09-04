#ifndef __EVENT_H__
#define __EVENT_H__

#define EVENT_TYPE_MALLOC 1
#define EVENT_TYPE_FREE 2
#define EVENT_TYPE_CALLOC 3
#define EVENT_TYPE_REALLOC 4
#define EVENT_TYPE_ALIGNED_ALLOC 5
#define EVENT_TYPE_MMAP 6
#define EVENT_TYPE_MUNMAP 7
#define EVENT_TYPE_BRK 8
#define EVENT_TYPE_FORK 9
#define EVENT_TYPE_EXEC 10
#define EVENT_TYPE_EXIT 11
#define EVENT_TYPE_RSS 12
#define EVENT_TYPE_RMAP 13

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

/* Counter slots in the stack_counters array map. */
#define MEMTRACK_STACK_COUNTER_COPY_FAILED 0
#define MEMTRACK_STACK_COUNTER_HASH_MAP_FULL 1
/* bpf_get_stackid() has several negative outcomes (no user callchain,
 * hash-bucket collision, or no free bucket), so this counts only missing ids. */
#define MEMTRACK_STACK_COUNTER_STACKID_FAILED 2
#define MEMTRACK_STACK_COUNTER_TRUNCATED 3
#define MEMTRACK_STACK_COUNTER_RING_FULL 4
#define MEMTRACK_STACK_COUNTER_COUNT 5

struct stack_regs {
    uint64_t reg[MEMTRACK_STACK_REGS];
};

/* Head of a stack record; `copy_len` raw stack bytes read upwards from `sp`
 * follow it. */
struct stack_header {
    uint64_t hash;
    uint64_t timestamp; /* monotonic time in nanoseconds (CLOCK_MONOTONIC) */
    int64_t stackid;    /* bpf_get_stackid() result; negative means unavailable */
    uint64_t sp;        /* user stack pointer the copy starts at */
    uint32_t pid;
    uint32_t tid;
    uint32_t copy_len;
    uint8_t truncated; /* the copy hit the size cap */
    uint8_t _pad[3];
    struct stack_regs regs;
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

        /* Memory mapping events (mmap, munmap, brk) */
        struct {
            uint64_t addr; /* address of mapping */
            uint64_t size; /* size of mapping */
        } mmap;

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
