#!/usr/bin/env bash
# Runs `codspeed setup` in the setup container, after building and installing the
# valgrind-codspeed <valgrind_commit> when one is given.
# Usage: tests/docker/setup.sh <codspeed binary> [valgrind-codspeed valgrind_commit]
set -euo pipefail

codspeed=$1
valgrind_commit=${2:-}

if [ -z "$valgrind_commit" ]; then
  exec "$codspeed" setup
fi

echo "Setting up valgrind-codspeed $valgrind_commit"
src=$(mktemp -d)
trap 'rm -rf "$src"' EXIT
git -C "$src" init -q
git -C "$src" fetch -q --depth 1 https://github.com/CodSpeedHQ/valgrind-codspeed.git "$valgrind_commit"
git -C "$src" checkout -q FETCH_HEAD
(cd "$src" && ./autogen.sh && ./configure --prefix=/usr && make -j"$(nproc)") >"$src/build.log" 2>&1 ||
  { tail -n 50 "$src/build.log"; exit 1; }
sudo make -C "$src" install >/dev/null
# Setup reinstalls valgrind from its package when the libc debug symbols are missing.
sudo apt-get update -q >/dev/null && sudo apt-get install -y -q libc6-dbg >/dev/null

built=$(valgrind --version)
echo "Installed $built from valgrind-codspeed $valgrind_commit"
"$codspeed" setup
# Setup keeps the installed valgrind only when it is at least the pinned version.
if [ "$(valgrind --version)" != "$built" ]; then
  echo "codspeed setup replaced $built with $(valgrind --version): valgrind-codspeed $valgrind_commit is older than the pinned release" >&2
  exit 1
fi
