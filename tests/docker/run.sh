#!/usr/bin/env bash
# Runs tests/executors.rs in throwaway Ubuntu containers.
# Usage: tests/docker/run.sh [test filter and libtest args...]
#
# Without arguments the suite is sharded over containers, CODSPEED_TEST_SHARDS (default 4)
# each for walltime and memory; each container has its own /tmp for the runner FIFO.
# memtrack's uprobes are system-wide and break Valgrind, so memory starts after simulation.
set -euo pipefail

repo=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
image=codspeed-executor-tests
rust_version=$(sed -n 's/.*channel = "\(.*\)".*/\1/p' "$repo/rust-toolchain.toml")
rust_components=$(sed -n 's/.*components = \[\(.*\)\].*/\1/p' "$repo/rust-toolchain.toml" | tr -d '" ')

docker build -q -t "$image" --build-arg RUST_VERSION="$rust_version" \
  --build-arg RUST_COMPONENTS="$rust_components" \
  -f "$repo/tests/docker/Dockerfile" "$repo/tests/docker" >/dev/null

# Caps match MEMTRACK_REQUIRED_CAPS (+ SYS_PTRACE for perf). Not --privileged: /proc/sys and
# /sys stay read-only, so host-wide kernel knobs (THP, swap, drop_caches) can't be changed.
# Unlimited memlock lets the unprivileged profiler mmap its perf ring buffers.
# --init forwards Ctrl-C to cargo, which ignores it as PID 1.
docker_flags=(
  --init
  --cap-add BPF --cap-add PERFMON --cap-add SYS_ADMIN --cap-add SYS_RESOURCE
  --cap-add DAC_READ_SEARCH --cap-add SYS_PTRACE --ulimit memlock=-1:-1
  --security-opt seccomp=unconfined --security-opt apparmor=unconfined
  -v "$repo":/workspace
  -v codspeed-tests-target:/home/tester/target
  -v codspeed-tests-cargo:/home/tester/.cargo/registry
  -v codspeed-tests-cargo-git:/home/tester/.cargo/git
)
in_container() {
  docker run --rm "${tty[@]}" "${docker_flags[@]}" "$run_image" "$@"
}
cargo_test=(cargo test -p codspeed-runner --features executor-tests --test executors)

tty=()
run_image=$image
in_container "${cargo_test[@]}" --no-run

# Bakes what `codspeed setup` installs (Valgrind, memtrack) into an image, so test containers
# don't reinstall it. The tag hashes the build inputs because the base image id changes on
# every build. A stale image only costs time: tests still run setup, a no-op once the pins match.
inputs_hash=$(cat "$repo/tests/docker/Dockerfile" "$repo/rust-toolchain.toml" \
  "$repo/src/binary_pins.rs" | sha256sum | cut -c1-12)
run_image=$image:setup-$inputs_hash
if ! docker image inspect "$run_image" >/dev/null 2>&1; then
  setup_container=$image-setup
  docker rm -f "$setup_container" >/dev/null 2>&1 || true
  docker run --name "$setup_container" "${docker_flags[@]}" "$image" \
    /home/tester/target/debug/codspeed setup
  docker commit "$setup_container" "$run_image" >/dev/null
  docker rm "$setup_container" >/dev/null
  docker images --filter "reference=$image:setup-*" --format '{{.Repository}}:{{.Tag}}' \
    | { grep -vxF "$run_image" || true; } | xargs -r docker rmi >/dev/null
fi

if [ $# -gt 0 ]; then
  tty=(-t)
  in_container "${cargo_test[@]}" -- "$@"
  exit
fi

simulation=()
memory=()
walltime=()
while read -r name; do
  case $name in
    *Simulation*) simulation+=("$name") ;;
    *Memory* | memory_*) memory+=("$name") ;;
    *) walltime+=("$name") ;;
  esac
done < <(in_container "${cargo_test[@]}" -q -- --list --format terse | sed -n 's/: test$//p')

# Background jobs ignore SIGINT, so Ctrl-C has to stop them (and their containers) explicitly.
trap 'trap - INT TERM; kill 0' INT TERM
pids=()
shard() {
  local label=$1
  shift
  (in_container "${cargo_test[@]}" -q -- --exact "$@" 2>&1 | sed -u "s/^/[$label] /") &
  pids+=($!)
}

# Round-robins the tests over `shards` containers.
spread() {
  local label=$1 i j
  shift
  local names=("$@")
  for ((i = 0; i < shards; i++)); do
    local part=()
    for ((j = i; j < ${#names[@]}; j += shards)); do
      part+=("${names[j]}")
    done
    if [ ${#part[@]} -gt 0 ]; then
      shard "$label-$i" "${part[@]}"
    fi
  done
}

status=0
shards=${CODSPEED_TEST_SHARDS:-4}
spread walltime "${walltime[@]}"
shard simulation "${simulation[@]}"
wait "${pids[-1]}" || status=1
unset 'pids[-1]'
spread memory "${memory[@]}"

for pid in "${pids[@]}"; do
  wait "$pid" || status=1
done
exit $status
