#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

bindgen "$SCRIPT_DIR/../instrument-hooks/includes/core.h" \
  -o "$SCRIPT_DIR/bindings.rs" \
  --rust-edition 2024 \
  --rust-target 1.88 \
  --allowlist-function "instrument_hooks_.*" \
  --allowlist-var "MARKER_TYPE_.*" \
  --allowlist-type "instrument_hooks_feature_t"
