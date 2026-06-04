#!/usr/bin/env bash
set -euo pipefail

# Custom script to replace node and run with V8 flags that make the execution of the
# benchmarks more predictable.
# Depending on the version of node, some flags may be deprecated.

# a custom bash function to echo debug messages only if CODSPEED_DEBUG is set to true
function echo_debug {
    if [ "${CODSPEED_DEBUG:-}" = "true" ]; then
        echo "::debug::" "$@"
    fi
}

# to avoid setting the variable for the children processes, unset it before running the node command
unset __CODSPEED_NODE_CORE_INTROSPECTION_PATH__

# macOS SIP strips DYLD_* across system-binary execs (/usr/bin/env, /bin/sh, …),
# which removes samply's injection library from this subtree. samply also exposes
# the value under a SAMPLY_-prefixed name that SIP does NOT strip; restore it
# here so the profiler's preload re-arms in the real node process.
if [ -n "${SAMPLY_DYLD_INSERT_LIBRARIES:-}" ]; then
    export DYLD_INSERT_LIBRARIES="$SAMPLY_DYLD_INSERT_LIBRARIES"
fi

# Retrieve the original path by removing the folder containing codspeed_introspected_node from the path.
ORIGINAL_PATH=$(echo "$PATH" | tr ":" "\n" | grep -v "codspeed_introspected_node" | tr "\n" ":")
# Check if node is in the original path.
if ! env PATH="$ORIGINAL_PATH" which node &>/dev/null; then
    echo "Error: node not found in PATH. There might be a problem with the node installation."
    exit 1
fi
# Save the real node path.
REAL_NODE_PATH=$(env PATH="$ORIGINAL_PATH" which node)

# Generate a random file in /tmp to store introspection data
INTROSPECTION_FILE=$(mktemp)
echo_debug "Introspection file: $INTROSPECTION_FILE"

# Temporarily disable 'set -e' for the command and capture its exit code.
echo_debug "Introspection trigger: env __CODSPEED_NODE_CORE_INTROSPECTION_PATH__=$INTROSPECTION_FILE" $REAL_NODE_PATH "$@"
env __CODSPEED_NODE_CORE_INTROSPECTION_PATH__="$INTROSPECTION_FILE" $REAL_NODE_PATH "$@"

# If the introspection file is empty, it means that the introspection didn't happen
if [ ! -s "$INTROSPECTION_FILE" ]; then
    echo_debug "No introspection detected (no introspection file found)"
    exit 0 # the exit code is 0 since otherwise the execution would have been stopped by 'set -e'
fi

# Retrieve the V8 flags from the introspection file
V8_FLAGS=$(jq -r '.flags | join(" ")' "$INTROSPECTION_FILE")
echo_debug "V8 flags harvested from introspection: $V8_FLAGS"

echo_debug "Running the CodSpeed node script, the node command that will be run:"
echo_debug "$REAL_NODE_PATH" $V8_FLAGS "$@"

# Call the real "node" command with any arguments passed to this script.
$REAL_NODE_PATH $V8_FLAGS "$@"
