use std::process::Command;

const NODE_OPTIONS_TO_ADD: &[&str] = &["--perf-basic-prof"];

/// Without this flag, all interpreted JS frames share V8's interpreter
/// trampoline, so memory stacks cannot name the allocating JS function.
pub const MEMORY_NODE_OPTIONS: &[&str] = &["--interpreted-frames-native-stack"];

/// Appends CodSpeed-required Node.js options, plus `extra`, to `NODE_OPTIONS`
/// on a [`Command`], preserving any existing value from the environment.
pub fn set_node_options(cmd: &mut Command, extra: &[&str]) {
    let existing = std::env::var("NODE_OPTIONS").unwrap_or_default();
    let mut parts: Vec<&str> = existing.split_whitespace().collect();

    for opt in NODE_OPTIONS_TO_ADD.iter().chain(extra) {
        if !parts.contains(opt) {
            parts.push(opt);
        }
    }

    cmd.env("NODE_OPTIONS", parts.join(" "));
}
