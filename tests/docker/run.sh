#!/usr/bin/env bash
# Runs tests/executors.rs in a throwaway Ubuntu container.
# Usage: tests/docker/run.sh [test filter and libtest args...]
set -euo pipefail

repo=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
image=codspeed-executor-tests
rust_version=$(sed -n 's/.*channel = "\(.*\)".*/\1/p' "$repo/rust-toolchain.toml")

docker build -q -t "$image" --build-arg RUST_VERSION="$rust_version" \
  -f "$repo/tests/docker/Dockerfile" "$repo/tests/docker" >/dev/null

# Caps match MEMTRACK_REQUIRED_CAPS (+ SYS_PTRACE for perf). Not --privileged: /proc/sys and
# /sys stay read-only, so host-wide kernel knobs (THP, swap, drop_caches) can't be changed.
# Unlimited memlock lets the unprivileged profiler mmap its perf ring buffers.
exec docker run --rm -t \
  --cap-add BPF --cap-add PERFMON --cap-add SYS_ADMIN --cap-add SYS_RESOURCE \
  --cap-add DAC_READ_SEARCH --cap-add SYS_PTRACE --ulimit memlock=-1:-1 \
  --security-opt seccomp=unconfined --security-opt apparmor=unconfined \
  -v "$repo":/workspace \
  -v codspeed-tests-target:/home/tester/target \
  -v codspeed-tests-cargo:/home/tester/.cargo/registry \
  "$image" cargo test -p codspeed-runner --test executors -- --ignored "$@"
