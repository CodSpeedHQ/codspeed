//! Build script for exec-harness
//!
//! This script compiles the `libcodspeed_preload.so` shared library that is used
//! to inject instrumentation into child processes via LD_PRELOAD.
//!
//! The library is built using the `core.c` and headers from the `instrument-hooks-bindings`
//! crate's `instrument-hooks` directory.

use std::env;
use std::path::PathBuf;

/// Shared constants for the preload library.
/// These are passed as C defines during compilation and exported as environment
/// variables for the Rust code to use via `env!()`.
struct PreloadConstants {
    /// Environment variable name for the benchmark URI.
    uri_env: &'static str,
    /// Integration name reported to CodSpeed.
    integration_name: &'static str,
    /// Integration version reported to CodSpeed.
    integration_version: &'static str,
    /// Filename for the preload shared library.
    preload_lib_filename: &'static str,
}

fn main() {
    println!("cargo:rerun-if-changed=preload/codspeed_preload.c");
    println!("cargo:rerun-if-env-changed=CODSPEED_INSTRUMENT_HOOKS_DIR");

    let preload_constants: PreloadConstants = PreloadConstants::default();

    // Export constants as environment variables for the Rust code
    println!(
        "cargo:rustc-env=CODSPEED_URI_ENV={}",
        preload_constants.uri_env
    );
    println!(
        "cargo:rustc-env=CODSPEED_INTEGRATION_NAME={}",
        preload_constants.integration_name
    );
    println!(
        "cargo:rustc-env=CODSPEED_INTEGRATION_VERSION={}",
        preload_constants.integration_version
    );
    println!(
        "cargo:rustc-env=CODSPEED_PRELOAD_LIB_FILENAME={}",
        preload_constants.preload_lib_filename
    );

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // Try to get the instrument-hooks directory from the environment variable first,
    // otherwise use the one from the instrument-hooks-bindings crate
    let instrument_hooks_dir = manifest_dir
        .parent()
        .unwrap()
        .join("instrument-hooks-bindings/instrument-hooks");

    // Build the preload shared library
    let paths = PreloadBuildPaths {
        preload_c: manifest_dir.join("preload/codspeed_preload.c"),
        core_c: instrument_hooks_dir.join("dist/core.c"),
        includes_dir: instrument_hooks_dir.join("includes"),
    };
    paths.check_sources_exist();
    build_shared_library(&paths, &preload_constants);
}

/// Build the shared library using the cc crate
fn build_shared_library(paths: &PreloadBuildPaths, constants: &PreloadConstants) {
    let uri_env_val = format!("\"{}\"", constants.uri_env);
    let integration_name_val = format!("\"{}\"", constants.integration_name);
    let integration_version_val = format!("\"{}\"", constants.integration_version);
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let out_file = out_dir.join(constants.preload_lib_filename);

    let mut build = cc::Build::new();
    build
        .file(&paths.preload_c)
        .file(&paths.core_c)
        .include(&paths.includes_dir)
        .pic(true)
        .opt_level(3)
        // There's no need to output cargo metadata as we are just building a shared library
        // that will be copied to disk and loaded through LD_PRELOAD at runtime
        .cargo_metadata(false)
        // Pass constants as C defines
        .define("CODSPEED_URI_ENV", uri_env_val.as_str())
        .define("CODSPEED_INTEGRATION_NAME", integration_name_val.as_str())
        .define(
            "CODSPEED_INTEGRATION_VERSION",
            integration_version_val.as_str(),
        )
        .std("gnu11") // need gnu11 instead of just c11 for setenv
        // Suppress warnings from generated Zig code
        .flag("-Wno-format")
        .flag("-Wno-format-security")
        .flag("-Wno-unused-but-set-variable")
        .flag("-Wno-unused-const-variable")
        .flag("-Wno-type-limits")
        .flag("-Wno-uninitialized")
        .flag("-Wno-overflow")
        .flag("-Wno-unused-function")
        .flag("-Wno-unterminated-string-initialization");

    // Compile source files to object files
    let objects = build.compile_intermediates();

    // Link object files into shared library
    let compiler = build.get_compiler();
    let mut link_cmd = compiler.to_command();
    link_cmd
        .arg("-shared")
        .arg("-o")
        .arg(&out_file)
        .args(&objects)
        .arg("-lpthread");

    let status = link_cmd.status().expect("Failed to run linker");
    if !status.success() {
        panic!("Failed to link libcodspeed_preload.so");
    }
}

impl Default for PreloadConstants {
    fn default() -> Self {
        Self {
            uri_env: "CODSPEED_BENCH_URI",
            integration_name: "exec-harness",
            integration_version: env!("CARGO_PKG_VERSION"),
            preload_lib_filename: "libcodspeed_preload.so",
        }
    }
}

/// Paths required to build the preload shared library.
struct PreloadBuildPaths {
    /// Path to the preload C source file (codspeed_preload.c).
    preload_c: PathBuf,
    /// Path to the core C source file from instrument-hooks.
    core_c: PathBuf,
    /// Path to the includes directory from instrument-hooks.
    includes_dir: PathBuf,
}

impl PreloadBuildPaths {
    /// Verify that all required source files and directories exist.
    /// Panics with a descriptive message if any path is missing.
    fn check_sources_exist(&self) {
        if !self.core_c.exists() {
            panic!(
                "core.c not found at {}. Make sure the instrument-hooks-bindings crate is available.",
                self.core_c.display()
            );
        }
        if !self.includes_dir.exists() {
            panic!(
                "includes directory not found at {}. Make sure the instrument-hooks-bindings crate is available.",
                self.includes_dir.display()
            );
        }
        if !self.preload_c.exists() {
            panic!(
                "codspeed_preload.c not found at {}",
                self.preload_c.display()
            );
        }
    }
}
