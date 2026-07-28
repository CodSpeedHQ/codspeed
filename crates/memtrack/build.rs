#[cfg(feature = "ebpf")]
use std::{env, path::PathBuf};

#[cfg(feature = "ebpf")]
fn build_ebpf() {
    // Force a rebuild of the test target to be able to run the full test suite locally just by
    // setting GITHUB_ACTIONS=1 in the environment.
    // This is because `test_with` is evaluated at build time
    println!("cargo::rerun-if-env-changed=GITHUB_ACTIONS");

    use libbpf_cargo::SkeletonBuilder;

    println!("cargo:rerun-if-changed=src/ebpf/c");

    // The same source (main.bpf.c) is compiled into two skeletons through thin
    // wrappers that differ only in whether MEMTRACK_BPF_VARIANT_TOKEN is set:
    // the token variant attaches uprobes as uprobe_multi links, which a
    // delegated BPF token can authorize, and the legacy variant uses
    // perf_event_open for kernels predating uprobe_multi. See
    // src/ebpf/c/utils/variant.h. The runtime picks one based on token
    // availability.
    let arch = env::var("CARGO_CFG_TARGET_ARCH")
        .expect("CARGO_CFG_TARGET_ARCH must be set in build script");
    let vmlinux_inc = vmlinux::include_path_root()
        .join(arch)
        .to_string_lossy()
        .into_owned();
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    for (source, skel) in [
        ("src/ebpf/c/memtrack_token.bpf.c", "memtrack_token.skel.rs"),
        (
            "src/ebpf/c/memtrack_legacy.bpf.c",
            "memtrack_legacy.skel.rs",
        ),
    ] {
        SkeletonBuilder::new()
            .source(source)
            .clang_args(["-I", &vmlinux_inc])
            .build_and_generate(out_dir.join(skel))
            .unwrap();
    }

    // Generate bindings for event.h
    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("Unable to generate bindings");
    let out_file = PathBuf::from(env::var("OUT_DIR").unwrap()).join("event.rs");
    std::fs::write(&out_file, bindings.to_string()).expect("Couldn't write bindings!");
}

fn main() {
    #[cfg(feature = "ebpf")]
    build_ebpf();
}
