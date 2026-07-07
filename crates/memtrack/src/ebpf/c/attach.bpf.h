/* == Exec-mapping watcher ==
 *
 * On a tracked process's first exec-map of an unknown inode: stop it
 * (SIGSTOP) and queue an attach request on a ring buffer. Userspace
 * classifies the file, attaches allocator probes, then resumes it. */

#define MEMTRACK_PROT_EXEC 0x4
#define MEMTRACK_SIGSTOP 19

struct inode_key {
    __u64 dev;
    __u64 ino;
};

/* (dev, ino) -> 1; populated by userspace after classify/attach */
BPF_HASH_MAP(known_inodes, struct inode_key, __u8, 8192);
/* Requests are 24 B and rare; overflow aborts the run via the counter below */
BPF_RINGBUF(attach_requests, 128 * 1024);
BPF_ARRAY_MAP(attach_request_dropped, __u64, 1);

SEC("fentry/security_mmap_file")
int BPF_PROG(watch_exec_mmap, struct file* file, unsigned long prot, unsigned long flags) {
    if (!file || !(prot & MEMTRACK_PROT_EXEC)) {
        return 0;
    }

    __u64 pid_tgid = bpf_get_current_pid_tgid();
    __u32 tgid = pid_tgid >> 32;
    if (!is_tracked(tgid)) {
        return 0;
    }

    struct inode_key key = {
        .dev = BPF_CORE_READ(file, f_inode, i_sb, s_dev),
        .ino = BPF_CORE_READ(file, f_inode, i_ino),
    };
    if (bpf_map_lookup_elem(&known_inodes, &key)) {
        return 0;
    }

    struct attach_request* req = bpf_ringbuf_reserve(&attach_requests, sizeof(*req), 0);
    if (!req) {
        __u32 zero = 0;
        __u64* drops = bpf_map_lookup_elem(&attach_request_dropped, &zero);
        if (drops) {
            __sync_fetch_and_add(drops, 1);
        }
        return 0;
    }
    req->pid = tgid;
    req->dev = key.dev;
    req->ino = key.ino;
    bpf_ringbuf_submit(req, 0);

    bpf_send_signal(MEMTRACK_SIGSTOP);
    return 0;
}
