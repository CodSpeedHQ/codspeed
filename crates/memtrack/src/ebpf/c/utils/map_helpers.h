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

#define BPF_RINGBUF(name, size)             \
    struct {                                \
        __uint(type, BPF_MAP_TYPE_RINGBUF); \
        __uint(max_entries, size);          \
    } name SEC(".maps")

#endif /* __MAP_HELPERS_H__ */
