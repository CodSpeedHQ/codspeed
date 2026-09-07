//! Build script for exec-harness
//!
//! Exports the constants shared between the crate's modules as environment
//! variables, so `src/constants.rs` can read them through `env!()` and there is
//! a single source of truth for the integration identity reported to CodSpeed.

/// Integration name reported to CodSpeed.
const INTEGRATION_NAME: &str = "exec-harness";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    println!("cargo:rustc-env=CODSPEED_INTEGRATION_NAME={INTEGRATION_NAME}");
    println!(
        "cargo:rustc-env=CODSPEED_INTEGRATION_VERSION={}",
        env!("CARGO_PKG_VERSION")
    );
}
