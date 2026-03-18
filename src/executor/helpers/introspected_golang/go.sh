#!/usr/bin/env bash

set -euo pipefail

debug_log() {
    if [ "${CODSPEED_LOG:-}" = "debug" ]; then
        echo "[DEBUG go.sh] $*" >&2
    fi
}

debug_log "Called with arguments: $*"
debug_log "Number of arguments: $#"


# Currently only walltime is supported
if [ "${CODSPEED_RUNNER_MODE:-}" != "walltime" ]; then
    echo "CRITICAL: Go benchmarks can only be run with the walltime instrument"
    exit 1
fi

# Find the real go binary, so that we don't end up in infinite recursion
REAL_GO=$(which -a go | grep -v "$(realpath "$0")" | head -1)
if [ -z "$REAL_GO" ]; then
    echo "ERROR: Could not find real go binary" >&2
    exit 1
fi

# Check if we have any arguments
if [ $# -eq 0 ]; then
    debug_log "No arguments provided, using standard go binary"
    "$REAL_GO"
    exit $?
fi

# On arm64, warn about setting CODSPEED_PERF_UNWINDING_MODE to "fp" for correct flamegraphs
if [ "$(uname -m)" = "aarch64" ] && [ "${CODSPEED_GO_SUPPRESS_PERF_UNWINDING_MODE_WARNING:-}" != "true" ]; then
    echo "::warning::Go profiling on arm64 require frame pointer unwinding. Set CODSPEED_PERF_UNWINDING_MODE=fp for better profiling." >&2
fi

# Route command based on first argument
case "$1" in
    test)
        debug_log "Detected 'test' command, routing to go-runner"

        # Find go-runner or install if not found
        GO_RUNNER=$(which codspeed-go-runner 2>/dev/null || true)
        if [ -z "$GO_RUNNER" ]; then
            # Build the installer URL with the specified version or use latest
            INSTALLER_VERSION="${CODSPEED_GO_RUNNER_VERSION:-latest}"
            if [ "$INSTALLER_VERSION" = "latest" ]; then
                DOWNLOAD_URL="http://github.com/CodSpeedHQ/codspeed-go/releases/latest/download/codspeed-go-runner-installer.sh"
                echo "::warning::Installing the latest version of codspeed-go-runner. This can silently introduce breaking changes. We recommend pinning a specific version via the \`go-runner-version\` option in the action." >&2
            else
                DOWNLOAD_URL="http://github.com/CodSpeedHQ/codspeed-go/releases/download/v${INSTALLER_VERSION}/codspeed-go-runner-installer.sh"
            fi

            debug_log "Installing go-runner from: $DOWNLOAD_URL"
            curl -fsSL "$DOWNLOAD_URL" | bash -s -- --quiet
            GO_RUNNER=$(which codspeed-go-runner 2>/dev/null || true)
        fi

        debug_log "Using go-runner at: $GO_RUNNER"
        debug_log "Full command: RUST_LOG=info $GO_RUNNER $*"

        "$GO_RUNNER" "$@"
        ;;
    run)
        debug_log "Detected 'run' command, injecting -work -ldflags flags"
        # Insert flags after 'run' but before other arguments
        # This is needed because GOFLAGS cannot handle spaces in ldflags...
        debug_log "Full command: $REAL_GO run -work -ldflags=\"-s=false -w=false\" ${*:2}"
        "$REAL_GO" run -work -ldflags="-s=false -w=false" "${@:2}"
        ;;
    *)
        debug_log "Detected non-test command ('$1'), routing to standard go binary"
        debug_log "Full command: $REAL_GO $*"
        "$REAL_GO" "$@"
        ;;
esac
exit $?
