#!/usr/bin/env bash
# Run the divan `otel_example` of the codspeed-rust checkout next to this
# repository through `codspeed run --experimental-otel` in walltime mode, and
# upload the results with the collected OTLP data.
#
# The upload goes to the CodSpeed instance of the selected runner profile: pick
# one with `CODSPEED_PROFILE` (see `codspeed profile list`). Arguments are
# passed to `codspeed run`, e.g. `--skip-upload` or `--profile-folder <dir>`.
set -euo pipefail

RUNNER_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CODSPEED_RUST_DIR="$RUNNER_DIR/../codspeed-rust"

if [ ! -d "$CODSPEED_RUST_DIR" ]; then
  echo "ERROR: codspeed-rust must be checked out at $CODSPEED_RUST_DIR." >&2
  exit 1
fi

cargo build --manifest-path "$RUNNER_DIR/Cargo.toml" --bin codspeed
CODSPEED="$RUNNER_DIR/target/debug/codspeed"

cd "$CODSPEED_RUST_DIR"
CARGO_CODSPEED="cargo run -q -p cargo-codspeed -- codspeed"
BENCH_ARGS="-p codspeed-divan-compat --bench otel_example"

$CARGO_CODSPEED build -m walltime $BENCH_ARGS --features otel

# The local provider: the GitHub Actions one needs a workflow event. Isolation
# needs sudo.
env -u GITHUB_ACTIONS CODSPEED_ISOLATION=false \
  "$CODSPEED" run --mode walltime --experimental-otel "$@" -- \
  "$CARGO_CODSPEED run $BENCH_ARGS"
