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
            uint64_t addr; /* address returned */
            uint64_t size; /* size requested */
        } alloc;

        /* Deallocation event (free) */
        struct {
            uint64_t addr; /* address to free */
        } free;

        /* Reallocation event - includes both old and new addresses */
        struct {
            uint64_t old_addr; /* previous address (can be NULL) */
            uint64_t new_addr; /* new address returned */
            uint64_t size;     /* new size requested */
        } realloc;

        /* Memory mapping events (mmap, munmap, brk) */
        struct {
            uint64_t addr; /* address of mapping */
            uint64_t size; /* size of mapping */
        } mmap;
    } data;
};

#endif /* __EVENT_H__ */
