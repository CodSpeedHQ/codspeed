#!/usr/bin/env bash
# CodSpeed `node` wrapper.
#
# Installed on PATH in front of the real node. Runs the real node with the V8
# flags that the codspeed-node integration would request, based on
# CODSPEED_RUNNER_MODE and the node major version.
#
# Mirrors getV8Flags() and the mode mapping of getInstrumentMode():
# https://github.com/CodSpeedHQ/codspeed-node/blob/main/packages/core/src/introspection.ts
# https://github.com/CodSpeedHQ/codspeed-node/blob/main/packages/core/src/runnerMode.ts
set -euo pipefail

# In simulation mode this script runs under valgrind with the CodSpeed preload
# library in LD_PRELOAD, which reports a benchmark result from every process
# that loads it. Helper processes must not load it, so it is removed here and
# restored for the final exec. For the same reason the script avoids command
# substitutions: a forked subshell would report a result when it exits.
codspeed_preload="${LD_PRELOAD:-}"
unset LD_PRELOAD

wrapper_script="${BASH_SOURCE[0]}"

# Sets `real_node` to the first `node` on PATH that is not this script.
find_real_node() {
    local entry
    local IFS=':'
    set -o noglob
    for entry in $PATH; do
        if [[ -z "$entry" || ! -x "$entry/node" || "$entry/node" -ef "$wrapper_script" ]]; then
            continue
        fi
        real_node="$entry/node"
        return 0
    done
    return 1
}

# Sets `major` from the given node binary's version ("v22.12.0" -> 22).
node_major_version() {
    local version
    # The only subprocess: node runs without the preload library (see above).
    version="$("$1" --version)"
    version="${version#v}"
    major="${version%%.*}"
}

# Flags for simulation and memory mode: make execution deterministic.
add_analysis_flags() {
    flags+=(
        --hash-seed=1
        --random-seed=1
        --no-opt
        --predictable
        --predictable-gc-schedule
        --expose-gc
        --no-concurrent-sweeping
        --max-old-space-size=4096
    )
    if (( major < 18 )); then
        flags+=(--no-randomize-hashes)
    fi
    if (( major < 20 )); then
        flags+=(--no-scavenge-task)
    else
        # V8 11.3 renamed --scavenge-task to --minor-gc-task
        flags+=(--no-minor-gc-task)
    fi
    if (( major >= 24 )); then
        # --no-opt only disables TurboFan. Maglev is enabled by default from
        # V8 13.6 and keeps tiering up hot functions.
        flags+=(--no-maglev)
    fi
}

# Flags for walltime mode: emit JIT symbols for the profiler.
add_walltime_flags() {
    flags+=(--perf-prof)
    if [[ -n "${CODSPEED_V8_LOG:-}" ]]; then
        flags+=(
            --log-code
            --no-log-source-code
            --no-logfile-per-isolate
            "--logfile=${CODSPEED_V8_LOG}/codspeed-v8-%p.log"
        )
    else
        flags+=(--perf-basic-prof)
    fi
}

find_real_node || {
    echo "codspeed: node not found in PATH" >&2
    exit 1
}
node_major_version "$real_node"

flags=(
    --interpreted-frames-native-stack
    --allow-natives-syntax
)
case "${CODSPEED_RUNNER_MODE:-walltime}" in
    instrumentation | simulation | memory) add_analysis_flags ;;
    walltime) add_walltime_flags ;;
    *)
        echo "codspeed: unknown CODSPEED_RUNNER_MODE '${CODSPEED_RUNNER_MODE}'" >&2
        exit 1
        ;;
esac

if [[ -n "$codspeed_preload" ]]; then
    export LD_PRELOAD="$codspeed_preload"
fi
exec "$real_node" "${flags[@]}" "$@"
