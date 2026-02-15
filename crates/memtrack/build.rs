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

    // Build the BPF program
    let arch = env::var("CARGO_CFG_TARGET_ARCH")
        .expect("CARGO_CFG_TARGET_ARCH must be set in build script");
    let memtrack_out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("memtrack.skel.rs");
    SkeletonBuilder::new()
        .source("src/ebpf/c/memtrack.bpf.c")
        .clang_args([
            "-I",
            &vmlinux::include_path_root().join(arch).to_string_lossy(),
        ])
        .build_and_generate(&memtrack_out)
        .unwrap();

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
