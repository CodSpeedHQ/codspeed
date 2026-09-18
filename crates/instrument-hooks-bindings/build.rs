use std::env;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(use_instrument_hooks)");

    println!("cargo:rerun-if-changed=instrument-hooks/dist/core.c");
    println!("cargo:rerun-if-changed=instrument-hooks/includes/core.h");
    println!("cargo:rerun-if-changed=build.rs");

    let mut build = cc::Build::new();
    build
        .flag("-std=c11")
        .file("instrument-hooks/dist/core.c")
        .include("instrument-hooks/includes")
        // We generated the C code from Zig, which contains some warnings
        // that can be safely ignored.
        .flag("-Wno-format")
        .flag("-Wno-format-security")
        .flag("-Wno-unused-but-set-variable")
        .flag("-Wno-unused-const-variable")
        .flag("-Wno-type-limits")
        .flag("-Wno-uninitialized")
        // Ignore warnings when cross-compiling:
        .flag("-Wno-overflow")
        .flag("-Wno-unused-function")
        .flag("-Wno-constant-conversion")
        .flag("-Wno-incompatible-pointer-types")
        // Disable warnings, as we will have lots of them
        .warnings(false)
        .extra_warnings(false)
        .cargo_warnings(false)
        .opt_level(3);

    let result = build.try_compile("instrument_hooks");
    match result {
        Ok(_) => println!("cargo:rustc-cfg=use_instrument_hooks"),
        Err(e) => {
            let compiler = build.try_get_compiler().expect("Failed to get C compiler");

            // Falling back to the noop implementation makes every hook a
            // no-op, so a build that lands there runs benchmarks and reports
            // no measurement at all, at exit code 0. Linux is where we
            // actually measure, so fail the build instead of emitting a
            // warning nobody reads.
            if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
                panic!(
                    "Failed to compile the instrument-hooks native library with cc-rs.\n\
                     A Linux build must not fall back to the noop implementation: it \
                     would run benchmarks and measure nothing.\n\
                     Make sure a C compiler for the target is installed and reachable \
                     by cc-rs (for musl targets, `musl-tools` provides \
                     `<arch>-linux-musl-gcc`).\n\
                     Compiler information: {compiler:?}\n\
                     Compilation error: {e}"
                );
            }

            eprintln!("\n\nWARNING: Failed to compile instrument-hooks native library with cc-rs.");
            eprintln!(
                "The library will still compile, but instrument-hooks functionality will be disabled."
            );
            eprintln!("Compiler information: {compiler:?}");
            eprintln!("Compilation error: {e}\n");

            println!(
                "cargo:warning=Failed to compile instrument-hooks native library with cc-rs. Continuing with noop implementation."
            );
        }
    }
}
