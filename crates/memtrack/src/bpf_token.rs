/// Whether libbpf has a delegated BPF token to attach to its `bpf()` calls. It
/// reads one from the bpffs mount named by `LIBBPF_BPF_TOKEN_PATH`, which is how
/// eBPF gets loaded without host privileges.
///
/// <https://docs.ebpf.io/linux/concepts/token/>
pub fn has_delegated_bpf_token() -> bool {
    std::env::var_os("LIBBPF_BPF_TOKEN_PATH")
        .is_some_and(|p| !p.is_empty() && std::path::Path::new(&p).is_dir())
}
