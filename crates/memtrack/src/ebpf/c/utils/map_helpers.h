#ifndef __MAP_HELPERS_H__
#define __MAP_HELPERS_H__

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

/* Task-local storage: one value per task_struct, reached by pointer chase off
 * the task rather than a hashed lookup, and freed with the task. NO_PREALLOC is
 * mandatory for this map type. */
#define BPF_TASK_STORAGE(name, value_type)       \
    struct {                                     \
        __uint(type, BPF_MAP_TYPE_TASK_STORAGE); \
        __uint(map_flags, BPF_F_NO_PREALLOC);    \
        __type(key, int);                        \
        __type(value, value_type);               \
    } name SEC(".maps")

#define BPF_RINGBUF(name, size)             \
    struct {                                \
        __uint(type, BPF_MAP_TYPE_RINGBUF); \
        __uint(max_entries, size);          \
    } name SEC(".maps")

#endif /* __MAP_HELPERS_H__ */
