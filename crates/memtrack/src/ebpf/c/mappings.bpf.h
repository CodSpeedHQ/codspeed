#ifndef __MAPPINGS_BPF_H__
#define __MAPPINGS_BPF_H__

#include "event.h"
#include "utils/folio.h"
#include "utils/map_helpers.h"
#include "utils/process_tracking.h"

/* == Mapping recorder ==
 *
 * Reconstructs what `PERF_RECORD_MMAP2` gives perf: which file a tracked
 * process mapped, where, so raw stack addresses can be attributed to modules
 * offline. No single hook carries both halves:
 *
 *   security_mmap_file(file, ..)   has the file, runs before the VMA exists
 *   perf_event_mmap(vma)           has the addresses, cannot resolve a path
 *
 * The path therefore lands in a per-inode cache, and the address-bearing hook
 * emits inode-keyed records that userspace joins against that cache while this
 * BPF object is still loaded.
 *
 * Path resolution is only reachable from an LSM program: `bpf_d_path()` is
 * restricted to sleepable LSM hooks, `BPF_TRACE_ITER` and an fentry allowlist
 * holding no mmap path, and the newer `bpf_path_d_path()` kfunc rejects
 * non-LSM program types. Both variants are compiled; userspace autoloads the
 * one the running kernel supports and neither when the bpf LSM is inactive. */

/* VM_EXEC from linux/mm.h, which vmlinux.h does not carry (it is a macro, not a
 * type). Only executable mappings are recorded: unwind data and symbols are
 * looked up by text address. */
#define MEMTRACK_VM_EXEC 0x00000004

/* d_path() fails with -ENAMETOOLONG rather than truncating, so a short buffer
 * loses whole modules. PATH_MAX keeps that from happening. */
#define MEMTRACK_MAX_PATH 4096

struct inode_path {
    __u32 len; /* bytes written by d_path, including the NUL */
    char path[MEMTRACK_MAX_PATH];
};

/* Every mapping of an inode shares its cached path. */
BPF_HASH_MAP(path_by_inode, struct inode_key, struct inode_path, 2048);

/* A dropped record may leave a module unresolved. */
BPF_RINGBUF(mappings, 256 * 1024);
BPF_ARRAY_MAP(mapping_dropped, __u64, 1);

/* The path does not fit on the BPF stack; build it in this per-CPU scratch map. */
struct {
    __uint(type, BPF_MAP_TYPE_PERCPU_ARRAY);
    __uint(max_entries, 1);
    __type(key, __u32);
    __type(value, struct inode_path);
} path_scratch SEC(".maps");

extern int bpf_path_d_path(const struct path* path, char* buf, __u64 buf__sz) __ksym __weak;

static __always_inline void bump_mapping_dropped(void) {
    __u32 zero = 0;
    __u64* drops = bpf_map_lookup_elem(&mapping_dropped, &zero);
    if (drops) {
        __sync_fetch_and_add(drops, 1);
    }
}

/* Return a scratch slot when this inode has no cached path. */
static __always_inline struct inode_path* mapping_path_slot(struct file* file,
                                                            struct inode_key* key) {
    if (!file || !is_tracked(current_tgid())) {
        return NULL;
    }

    key->dev = BPF_CORE_READ(file, f_inode, i_sb, s_dev);
    key->ino = BPF_CORE_READ(file, f_inode, i_ino);
    if (bpf_map_lookup_elem(&path_by_inode, key)) {
        return NULL;
    }

    __u32 zero = 0;
    return bpf_map_lookup_elem(&path_scratch, &zero);
}

/* Publish a resolved path. A failed resolution is not cached, so the next
 * mapping of the same inode retries instead of losing the module for the run. */
static __always_inline void commit_mapping_path(struct inode_key* key, struct inode_path* entry,
                                                int len) {
    if (len <= 0) {
        return;
    }
    entry->len = (__u32)len;
    bpf_map_update_elem(&path_by_inode, key, entry, BPF_NOEXIST);
}

/* Kernels >= 6.12: the kfunc is callable from any LSM program. */
SEC("lsm/mmap_file")
int BPF_PROG(cache_mmap_path_kfunc, struct file* file, unsigned long reqprot, unsigned long prot,
             unsigned long flags) {
    struct inode_key key = {};
    struct inode_path* entry = mapping_path_slot(file, &key);
    if (entry) {
        commit_mapping_path(&key, entry,
                            bpf_path_d_path(&file->f_path, entry->path, MEMTRACK_MAX_PATH));
    }
    return 0;
}

/* Kernels 5.11..6.11: `bpf_d_path()` needs a sleepable LSM hook, which
 * `mmap_file` has been since 5.11. */
SEC("lsm.s/mmap_file")
int BPF_PROG(cache_mmap_path_legacy, struct file* file, unsigned long reqprot, unsigned long prot,
             unsigned long flags) {
    struct inode_key key = {};
    struct inode_path* entry = mapping_path_slot(file, &key);
    if (entry) {
        commit_mapping_path(&key, entry, bpf_d_path(&file->f_path, entry->path, MEMTRACK_MAX_PATH));
    }
    return 0;
}

/* The same hook perf emits MMAP2 from, so the recorded geometry matches what
 * the walltime pipeline already consumes: the file offset is in bytes, not
 * pages. */
SEC("fentry/perf_event_mmap")
int BPF_PROG(record_mmap, struct vm_area_struct* vma) {
    if (!vma) {
        return 0;
    }

    __u32 tgid = current_tgid();
    if (!is_tracked(tgid)) {
        return 0;
    }

    struct file* file = BPF_CORE_READ(vma, vm_file);
    if (!file) {
        return 0;
    }
    if (!(BPF_CORE_READ(vma, vm_flags) & MEMTRACK_VM_EXEC)) {
        return 0;
    }

    struct mapping_record* rec = bpf_ringbuf_reserve(&mappings, sizeof(*rec), 0);
    if (!rec) {
        bump_mapping_dropped();
        return 0;
    }

    rec->pid = tgid;
    rec->dev = BPF_CORE_READ(file, f_inode, i_sb, s_dev);
    rec->ino = BPF_CORE_READ(file, f_inode, i_ino);
    rec->file_offset = (__u64)BPF_CORE_READ(vma, vm_pgoff) << page_shift;
    rec->start = BPF_CORE_READ(vma, vm_start);
    rec->end = BPF_CORE_READ(vma, vm_end);
    rec->timestamp = bpf_ktime_get_ns();
    bpf_ringbuf_submit(rec, 0);

    return 0;
}

#endif /* __MAPPINGS_BPF_H__ */
