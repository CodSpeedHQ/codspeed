#!/usr/bin/env bash
# Runs `codspeed setup` in the setup container, after installing a valgrind-codspeed
# other than the pinned release when one is given:
# - `commit <sha>` builds and installs that valgrind-codspeed commit;
# - `deb <path>` installs that valgrind-codspeed package.
# Usage: tests/docker/setup.sh <codspeed binary> [commit <sha> | deb <path>]
set -euo pipefail

codspeed=$1
source=${2:-}
value=${3:-}

case $source in
  "")
    exec "$codspeed" setup
    ;;
  commit)
    echo "Setting up valgrind-codspeed $value"
    src=$(mktemp -d)
    trap 'rm -rf "$src"' EXIT
    git -C "$src" init -q
    git -C "$src" fetch -q --depth 1 https://github.com/CodSpeedHQ/valgrind-codspeed.git "$value"
    git -C "$src" checkout -q FETCH_HEAD
    (cd "$src" && ./autogen.sh && ./configure --prefix=/usr && make -j"$(nproc)") >"$src/build.log" 2>&1 ||
      { tail -n 50 "$src/build.log"; exit 1; }
    sudo make -C "$src" install >/dev/null
    # Setup reinstalls valgrind from its package when the libc debug symbols are missing.
    sudo apt-get update -q >/dev/null && sudo apt-get install -y -q libc6-dbg >/dev/null
    ;;
  deb)
    echo "Setting up valgrind-codspeed from $value"
    # The same packages `codspeed setup` installs along with the pinned package.
    sudo apt-get update -q >/dev/null
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q "$value" libc6-dbg >/dev/null
    ;;
  *)
    echo "Unknown valgrind-codspeed source '$source', expected 'commit' or 'deb'" >&2
    exit 1
    ;;
esac

built=$(valgrind --version)
echo "Installed $built"
"$codspeed" setup
# Setup keeps the installed valgrind only when it is at least the pinned version.
if [ "$(valgrind --version)" != "$built" ]; then
  echo "codspeed setup replaced $built with $(valgrind --version): the installed valgrind-codspeed is older than the pinned release" >&2
  exit 1
fi
