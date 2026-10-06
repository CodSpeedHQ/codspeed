use crate::measurement_mode::MeasurementMode;

const JAVA_TOOL_OPTIONS: &str = "JAVA_TOOL_OPTIONS";

// Java: Enable frame pointers and perf map generation for flamegraph profiling.
// - UnlockDiagnosticVMOptions must come before DumpPerfMapAtExit (diagnostic option).
// - PreserveFramePointer: Preserves frame pointers for profiling.
// - DumpPerfMapAtExit: Writes /tmp/perf-<pid>.map on JVM exit for symbol resolution.
// - DebugNonSafepoints: Enables debug info for JIT-compiled non-safepoint code.
// - EnableDynamicAgentLoading: Suppresses warning when loading JVMTI agents at runtime.
// - jdk.attach.allowAttachSelf: Allows the JVM to attach a JVMTI agent to itself
//   (used by codspeed-jvm's perf-map agent for @Fork(0) benchmarks).
const WALLTIME_JAVA_TOOL_OPTIONS: &str = "-XX:+PreserveFramePointer -XX:+UnlockDiagnosticVMOptions -XX:+DebugNonSafepoints -XX:+EnableDynamicAgentLoading -Djdk.attach.allowAttachSelf=true";

pub(super) fn env(mode: MeasurementMode) -> Vec<(&'static str, String)> {
    if mode != MeasurementMode::Walltime {
        return Vec::new();
    }
    vec![(JAVA_TOOL_OPTIONS, WALLTIME_JAVA_TOOL_OPTIONS.to_owned())]
}
