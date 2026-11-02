use crate::executor::config::WalltimeUnwinding;
use crate::executor::helpers::detect_executable::command_has_executable;
use crate::prelude::*;

pub(super) fn resolve_unwinding(
    command: &str,
    explicit: Option<WalltimeUnwinding>,
) -> WalltimeUnwinding {
    let suggested = suggested_unwinding(command);
    let Some(explicit) = explicit else {
        return suggested.unwrap_or(WalltimeUnwinding::Dwarf { stack_size: None });
    };
    if let Some(suggested) = suggested
        && suggested != explicit
    {
        warn!(
            "Overriding suggested unwinding `{suggested}` with `{explicit}`, call stacks may be incomplete"
        );
    }
    explicit
}

/// Unwinding suggested for the benchmarked runtime, if it has a known best setting.
fn suggested_unwinding(command: &str) -> Option<WalltimeUnwinding> {
    if command_has_executable(
        command,
        &["gradle", "gradlew", "java", "maven", "mvn", "mvnw"],
    ) {
        return Some(WalltimeUnwinding::FramePointer);
    }
    if command_has_executable(command, &["go"]) {
        return Some(WalltimeUnwinding::FramePointer);
    }
    if command_has_executable(command, &["pytest", "uv", "python", "python3"]) {
        return Some(WalltimeUnwinding::Dwarf {
            stack_size: Some(32 * 1024),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_unwinding_overrides_language_inference() {
        let explicit = WalltimeUnwinding::Dwarf {
            stack_size: Some(16 * 1024),
        };
        for (command, inferred) in [
            (
                "python bench.py",
                WalltimeUnwinding::Dwarf {
                    stack_size: Some(32 * 1024),
                },
            ),
            ("cargo bench", WalltimeUnwinding::Dwarf { stack_size: None }),
            ("go test -bench=.", WalltimeUnwinding::FramePointer),
            ("java -jar bench.jar", WalltimeUnwinding::FramePointer),
        ] {
            assert_eq!(resolve_unwinding(command, None), inferred);
            assert_eq!(resolve_unwinding(command, Some(explicit)), explicit);
        }
        assert_eq!(
            resolve_unwinding("python bench.py", Some(WalltimeUnwinding::FramePointer)),
            WalltimeUnwinding::FramePointer,
        );
    }
}
