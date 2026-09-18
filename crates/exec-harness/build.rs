const INTEGRATION_NAME: &str = "exec-harness";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    println!("cargo:rustc-env=CODSPEED_INTEGRATION_NAME={INTEGRATION_NAME}");
    println!(
        "cargo:rustc-env=CODSPEED_INTEGRATION_VERSION={}",
        env!("CARGO_PKG_VERSION")
    );
}
