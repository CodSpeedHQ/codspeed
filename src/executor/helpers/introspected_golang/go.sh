#!/usr/bin/env bash

set -euo pipefail

debug_log() {
    if [ "${CODSPEED_LOG:-}" = "debug" ]; then
        echo "[DEBUG go.sh] $*" >&2
    fi
}

debug_log "Called with arguments: $*"
debug_log "Number of arguments: $#"


GO_RUNNER_INSTALLER_SHA256S="
0.1.1 f841950fe630dbc5c1bd534eb7cb8acf476b744297081710866092551110a68f
0.1.2 cdbe066adccad7eb11ca53daf57a2c754666966d6ec9ab02e399f6f00a87f3eb
0.2.0 10b75883bd8c09133281d652ea3ae1897f35606dd8a4a722d5c9b304666eaf7e
0.3.0 d50608b9ceff1426badf5cb868bb76eca74428715006029519248d61c4781799
0.4.0 dfb13bd30afef8a430680813b5605f46243962f6e31359f387d7c90ee9018be3
0.4.1 94b8b82a095597e3bf4fea12cccd6bbc2e52e619d7953c3fcb695651ae5e804c
0.4.2 5fca476647e269aedc91e5920265867de56e4dcada5bd83d99a507e437236e06
0.5.0 82c68678904691903567f542117bcbf6aceca87b34db14c946915b988f12cf8f
0.5.1 db2de37e815913ce72f761a8c25aeef4a64fe2407a870e0dc2ea149a3decd904
0.6.0 e7bf7c37b07bc43610cf35140556b51cb03b4355914307f8100bef5f7f37c85d
0.6.1 a38e0e3417abce9260f1b8c3ff7407bb93900a0ad83ee191725a0266f97c797c
0.6.2 616200762cc2fa582fae56ea58e77ad6c056bd658f6907d5be56d39d59ec6616
1.0.0 0540d8abe62357acefb85b9f1a9ff81dcfef70d6be8bea35096bf26a295a91f8
1.0.1 c26f463883a77591e5a2e2f17f0995a989cbada0d4f5115f327900badac07918
1.0.2 4e4ecfb1888ced253f0acbbc132db0b1d7e99351d40f3eff789a518a6130ee35
1.1.0 d16e0e14bdfaea61a6da1d46d7b3b36f940b64335c8affbdc85b802d6e949a97
1.2.0 072876ccd43b8c73c123df206eda4b1f82f9ff03b1330efe35e5eaa5c1b6cefe
"

DEFAULT_GO_RUNNER_VERSION="1.2.0"

get_go_runner_installer_sha256() {
    if ! awk -v v="$1" '$1==v{print $2; f=1} END{exit !f}' <<<"$GO_RUNNER_INSTALLER_SHA256S"; then
        echo "ERROR: No pinned sha256 for codspeed-go-runner version $1" >&2
        exit 1
    fi
}

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{ print $1 }'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{ print $1 }'
    else
        echo "ERROR: Could not find sha256sum or shasum to verify downloaded installer" >&2
        exit 1
    fi
}

install_go_runner() {
    local version="$1"
    local expected_sha256
    expected_sha256=$(get_go_runner_installer_sha256 "$version")
    local download_url="https://github.com/CodSpeedHQ/codspeed-go/releases/download/v${version}/codspeed-go-runner-installer.sh"
    local tmp_dir
    tmp_dir=$(mktemp -d)
    local installer_path="$tmp_dir/codspeed-go-runner-installer.sh"

    cleanup_go_runner_installer() {
        rm -rf "$tmp_dir"
    }
    trap cleanup_go_runner_installer RETURN

    curl -fsSL "$download_url" -o "$installer_path"

    local actual_sha256
    actual_sha256=$(sha256_file "$installer_path")
    if [ "$actual_sha256" != "$expected_sha256" ]; then
        echo "ERROR: Hash mismatch for $download_url: expected $expected_sha256, got $actual_sha256" >&2
        exit 1
    fi

    bash "$installer_path" --quiet
}


# Currently only walltime is supported
if [ "${CODSPEED_RUNNER_MODE:-}" != "walltime" ]; then
    echo "CRITICAL: Go benchmarks can only be run with the walltime instrument"
    exit 1
fi

# Find the real go binary by removing our directory from PATH (same approach as node.sh)
ORIGINAL_PATH=$(echo "$PATH" | tr ":" "\n" | grep -v "codspeed_introspected_go" | tr "\n" ":")
REAL_GO=$(env PATH="$ORIGINAL_PATH" which go 2>/dev/null || true)
debug_log "Real go path: $REAL_GO"
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
            INSTALLER_VERSION="${CODSPEED_GO_RUNNER_VERSION:-$DEFAULT_GO_RUNNER_VERSION}"
            debug_log "Installing go-runner v${INSTALLER_VERSION}"
            install_go_runner "$INSTALLER_VERSION"
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
