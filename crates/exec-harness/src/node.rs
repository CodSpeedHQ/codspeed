use std::process::Command;

/// `--interpreted-frames-native-stack`: without it, all interpreted JS frames
/// share V8's interpreter trampoline, so stacks cannot name the JS function.
const NODE_OPTIONS_TO_ADD: &[&str] = &["--perf-basic-prof", "--interpreted-frames-native-stack"];

/// Appends CodSpeed-required Node.js options to `NODE_OPTIONS` on a [`Command`],
/// preserving any existing value from the environment.
pub fn set_node_options(cmd: &mut Command) {
    let existing = std::env::var("NODE_OPTIONS").unwrap_or_default();
    let mut parts: Vec<&str> = existing.split_whitespace().collect();

    for opt in NODE_OPTIONS_TO_ADD {
        if !parts.contains(opt) {
            parts.push(opt);
        }
    }

    cmd.env("NODE_OPTIONS", parts.join(" "));
}
