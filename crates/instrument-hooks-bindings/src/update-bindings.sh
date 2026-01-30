#!/usr/bin/env bash
set -euo pipefail

bindgen ../instrument-hooks/includes/core.h \
  -o bindings.rs \
  --rust-target 1.74 \
  --allowlist-function "instrument_hooks_.*" \
  --allowlist-var "MARKER_TYPE_.*"
