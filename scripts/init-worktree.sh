#!/bin/sh
# Initialize a new worktree when it is first created.
#
# Runs from pre-commit's `post-checkout` stage. Git passes the all-zero SHA as
# the previous HEAD only during `git worktree add`, which is how we detect this
# case (pre-commit forwards it as PRE_COMMIT_FROM_REF).
set -eu

ZERO_SHA="0000000000000000000000000000000000000000"

if [ "${PRE_COMMIT_FROM_REF:-}" != "$ZERO_SHA" ]; then
  exit 0
fi

MAIN_REPO=$(git worktree list --porcelain | awk '/^worktree /{print $2; exit}')
CURRENT_DIR=$(pwd)

if [ "$MAIN_REPO" = "$CURRENT_DIR" ]; then
  exit 0
fi

printf "\n🌿 Initializing worktree from %s\n\n" "$MAIN_REPO"

printf "📦 Initializing submodules...\n"
git submodule update --init --recursive

if command -v pre-commit >/dev/null 2>&1; then
  printf "\n🪝 Installing pre-commit hooks...\n"
  pre-commit install --hook-type pre-commit --hook-type post-checkout
fi
