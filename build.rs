fn main() {
    // Force a rebuild of the test target to be able to run the full test suite locally just by
    // setting GITHUB_ACTIONS=1 in the environment.
    // This is because `test_with` is evaluated at build time
    println!("cargo::rerun-if-env-changed=GITHUB_ACTIONS");
}
