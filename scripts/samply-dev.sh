#!/bin/sh
# Toggle "samply dev mode" for the runner.
#
# Dev mode redirects the runner's `samply`, `framehop`, and
# `linux-perf-event-reader` dependencies to local sibling checkouts via
# `.cargo/config.toml` patch files, without ever touching the committed
# `Cargo.toml` / `Cargo.lock` / `.gitignore`. This lets you iterate on all
# three crates in place and have the runner pick the changes up immediately.
#
# Redirected dependencies (all resolve to siblings of the runner repo):
#   - samply    (../samply-codspeed)  committed as a git dep in the runner's
#                                     Cargo.toml; patched via [patch."<url>"]
#   - framehop  (../framehop)         committed as a git dep in samply's
#                                     Cargo.toml; patched via [patch."<url>"]
#   - linux-perf-event-reader (../linux-perf-event-reader)
#                                     a crates.io dep pulled in transitively via
#                                     linux-perf-data, so it is overridden with
#                                     [patch.crates-io] rather than a git-url patch
#
# The patch files are kept out of git locally via each repo's
# `.git/info/exclude` (a per-clone, uncommitted ignore list) — nothing is added
# to the tracked `.gitignore`.
#
# Building with the patch in place rewrites the tracked `Cargo.lock` (the
# git-rev `source` lines are dropped for path deps). `.git/info/exclude` only
# hides untracked files, so the lock is instead masked with
# `git update-index --skip-worktree`, which tells git to ignore local edits to
# a tracked file. `off` clears the flag and restores the lock to HEAD.
#
# Two patch files are managed (both locally excluded):
#   - <runner>/.cargo/config.toml   patches samply + framehop + reader -> local
#   - <samply>/.cargo/config.toml   patches framehop + reader -> local (so samply
#                                   standalone builds also use them)
#
# The committed manifests stay pinned to git revs, so all repos remain
# committable at any time. Each repo's tracked revision is read straight from
# the committed manifests (the `rev = "..."` in the runner's and samply's
# Cargo.toml) — revisions are never hardcoded in this script. `sync` checks out
# those tracked revisions in each local checkout, skipping any checkout that has
# uncommitted changes so in-progress work is never clobbered. Bumping a rev for
# release is a separate, manual step.
#
# Every command ends by printing a recap of which local checkout each
# dependency resolves to, its current HEAD, the manifest-tracked revision, and
# whether the two agree.
#
# Usage:
#   scripts/samply-dev.sh on       enable dev mode (write patch files)
#   scripts/samply-dev.sh off      disable dev mode (remove patch files)
#   scripts/samply-dev.sh status   show current state
#   scripts/samply-dev.sh sync     check out each repo's manifest-tracked revision
set -eu

# Resolve repo roots relative to this script, not the cwd.
RUNNER_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SAMPLY_ROOT=$(CDPATH= cd -- "$RUNNER_ROOT/../samply-codspeed" 2>/dev/null && pwd || true)
FRAMEHOP_ROOT=$(CDPATH= cd -- "$RUNNER_ROOT/../framehop" 2>/dev/null && pwd || true)
READER_ROOT=$(CDPATH= cd -- "$RUNNER_ROOT/../linux-perf-event-reader" 2>/dev/null && pwd || true)

SAMPLY_URL="https://github.com/CodSpeedHQ/samply-codspeed"
FRAMEHOP_URL="https://github.com/CodSpeedHQ/framehop"
READER_URL="https://github.com/AvalancheHQ/linux-perf-event-reader"

RUNNER_CONFIG="$RUNNER_ROOT/.cargo/config.toml"
SAMPLY_CONFIG="$SAMPLY_ROOT/.cargo/config.toml"

# Manifests that pin each repo's tracked git revision (single source of truth;
# revs are never hardcoded in this script). `sync` reads `rev = "..."` from them.
#   framehop -> a git dep in samply's Cargo.toml
#   reader   -> a [patch.crates-io] git entry in samply's Cargo.toml
#     (linux-perf-event-reader comes from crates.io transitively via
#      linux-perf-data, so it is overridden with [patch.crates-io], not a
#      git-url patch)
#   samply   -> a git dep in the runner's Cargo.toml
SAMPLY_MANIFEST="$SAMPLY_ROOT/samply/Cargo.toml"
RUNNER_MANIFEST="$RUNNER_ROOT/Cargo.toml"

# Extract the `rev = "..."` from the first manifest line mentioning $2 (a repo
# URL slug). Empty if the manifest or the entry is absent.
#   manifest_rev <manifest-file> <needle>
manifest_rev() {
  manifest=$1
  needle=$2
  [ -f "$manifest" ] || return 0
  grep -E "$needle" "$manifest" 2>/dev/null \
    | grep -oE 'rev[[:space:]]*=[[:space:]]*"[0-9a-fA-F]{7,40}"' \
    | grep -oE '[0-9a-fA-F]{7,40}' \
    | head -1 \
    || true
}

framehop_tracked_rev() { manifest_rev "$SAMPLY_MANIFEST" 'CodSpeedHQ/framehop'; }
reader_tracked_rev()   { manifest_rev "$SAMPLY_MANIFEST" 'CodSpeedHQ/linux-perf-event-reader'; }
samply_tracked_rev()   { manifest_rev "$RUNNER_MANIFEST" 'CodSpeedHQ/samply-codspeed'; }

# Pattern stored in .git/info/exclude (relative to each repo root).
EXCLUDE_ENTRY="/.cargo/config.toml"

usage() {
  echo "Usage: $0 {on|off|status|sync}" >&2
  echo "  on      enable dev mode (write patch files)" >&2
  echo "  off     disable dev mode (remove patch files)" >&2
  echo "  status  show dev-mode state" >&2
  echo "  sync    check out each repo's manifest-tracked revision" >&2
  echo "(every command ends by printing the repo-wiring recap)" >&2
  exit 2
}

# Resolve <repo>/.git/info/exclude, failing if $1 is not a git checkout.
resolve_exclude_file() {
  repo_root=$1
  git_dir=$(CDPATH= cd -- "$repo_root" && git rev-parse --git-dir 2>/dev/null) || {
    echo "error: $repo_root is not a git repository" >&2
    exit 1
  }
  case "$git_dir" in
    /*) ;;                       # already absolute
    *) git_dir="$repo_root/$git_dir" ;;
  esac
  printf '%s\n' "$git_dir/info/exclude"
}

# Ensure $EXCLUDE_ENTRY is present in <repo>/.git/info/exclude.
add_local_exclude() {
  exclude_file=$(resolve_exclude_file "$1")
  mkdir -p "$(dirname -- "$exclude_file")"
  if [ ! -f "$exclude_file" ] || ! grep -qxF "$EXCLUDE_ENTRY" "$exclude_file"; then
    printf '%s\n' "$EXCLUDE_ENTRY" >> "$exclude_file"
  fi
}

# Remove $EXCLUDE_ENTRY from <repo>/.git/info/exclude if present.
remove_local_exclude() {
  exclude_file=$(resolve_exclude_file "$1")
  [ -f "$exclude_file" ] || return 0
  if grep -qxF "$EXCLUDE_ENTRY" "$exclude_file"; then
    grep -vxF "$EXCLUDE_ENTRY" "$exclude_file" > "$exclude_file.tmp" && mv "$exclude_file.tmp" "$exclude_file"
  fi
}

# Restore Cargo.lock to HEAD, then mask the local build-induced edits to it.
mask_cargo_lock() {
  repo_root=$1
  git -C "$repo_root" checkout -- Cargo.lock
  git -C "$repo_root" update-index --skip-worktree Cargo.lock
}

# Unmask Cargo.lock and restore it to HEAD.
unmask_cargo_lock() {
  repo_root=$1
  git -C "$repo_root" update-index --no-skip-worktree Cargo.lock 2>/dev/null || true
  git -C "$repo_root" checkout -- Cargo.lock 2>/dev/null || true
}

require_dirs() {
  if [ -z "$SAMPLY_ROOT" ]; then
    echo "error: ../samply-codspeed not found next to the runner repo" >&2
    exit 1
  fi
  if [ -z "$FRAMEHOP_ROOT" ]; then
    echo "error: ../framehop not found next to the runner repo" >&2
    exit 1
  fi
  if [ -z "$READER_ROOT" ]; then
    echo "error: ../linux-perf-event-reader not found next to the runner repo" >&2
    exit 1
  fi
}

enable() {
  require_dirs

  add_local_exclude "$RUNNER_ROOT"
  mkdir -p "$RUNNER_ROOT/.cargo"
  cat > "$RUNNER_CONFIG" <<EOF
# Generated by scripts/samply-dev.sh — excluded locally, do not commit.
# Run \`scripts/samply-dev.sh off\` to remove.
[patch."$SAMPLY_URL"]
samply = { path = "../samply-codspeed/samply" }

[patch."$FRAMEHOP_URL"]
framehop = { path = "../framehop" }

[patch.crates-io]
linux-perf-event-reader = { path = "../linux-perf-event-reader" }
EOF
  mask_cargo_lock "$RUNNER_ROOT"

  add_local_exclude "$SAMPLY_ROOT"
  mkdir -p "$SAMPLY_ROOT/.cargo"
  cat > "$SAMPLY_CONFIG" <<EOF
# Generated by scripts/samply-dev.sh (from the runner repo) — excluded locally.
# Lets samply standalone builds also use local framehop + linux-perf-event-reader.
[patch."$FRAMEHOP_URL"]
framehop = { path = "../framehop" }

[patch.crates-io]
linux-perf-event-reader = { path = "../linux-perf-event-reader" }
EOF
  mask_cargo_lock "$SAMPLY_ROOT"

  echo "samply dev mode: ON"
  echo "  wrote $RUNNER_CONFIG"
  echo "  wrote $SAMPLY_CONFIG"
}

disable() {
  removed=0
  if [ -f "$RUNNER_CONFIG" ]; then
    rm -f "$RUNNER_CONFIG"
    echo "  removed $RUNNER_CONFIG"
    removed=1
  fi
  if [ -n "$SAMPLY_ROOT" ] && [ -f "$SAMPLY_CONFIG" ]; then
    rm -f "$SAMPLY_CONFIG"
    echo "  removed $SAMPLY_CONFIG"
    removed=1
  fi

  # Unmask Cargo.lock now the patch is gone (best-effort; safe if never masked).
  unmask_cargo_lock "$RUNNER_ROOT"
  [ -n "$SAMPLY_ROOT" ] && unmask_cargo_lock "$SAMPLY_ROOT"
  # Clean up now-empty .cargo dirs we may have created.
  rmdir "$RUNNER_ROOT/.cargo" 2>/dev/null || true
  [ -n "$SAMPLY_ROOT" ] && rmdir "$SAMPLY_ROOT/.cargo" 2>/dev/null || true

  # Drop the local-exclude entries we added.
  remove_local_exclude "$RUNNER_ROOT"
  [ -n "$SAMPLY_ROOT" ] && remove_local_exclude "$SAMPLY_ROOT"

  if [ "$removed" -eq 1 ]; then
    echo "samply dev mode: OFF"
  else
    echo "samply dev mode: already OFF"
  fi
}

# Current HEAD of a checkout (short), or "-" if missing/not a repo.
repo_head() {
  repo_root=$1
  [ -n "$repo_root" ] || { printf '%s' "-"; return; }
  git -C "$repo_root" rev-parse --short HEAD 2>/dev/null || printf '%s' "-"
}

# Collapse a leading $HOME to ~ to keep paths short.
tilde() {
  case "$1" in
    "$HOME"/*) printf '~%s' "${1#"$HOME"}" ;;
    "$HOME")   printf '~' ;;
    *)         printf '%s' "$1" ;;
  esac
}

# Shorten a git rev to 12 chars for display (enough to identify; sync is still
# computed from the full value). "-" / empty pass through unchanged.
short_rev() {
  case "$1" in
    "" | "-") printf '%s' "${1:--}" ;;
    *)        printf '%s' "$1" | cut -c1-12 ;;
  esac
}

# Print a recap block for one dependency: a status line (glyph + label + state)
# followed by aligned path / head / tracked detail lines.
#   recap_line <label> <repo-root> <tracked-rev>
recap_line() {
  label=$1
  repo_root=$2
  tracked=$3
  head=$(repo_head "$repo_root")
  if [ "$repo_root" = "-" ] || [ -z "$repo_root" ]; then
    glyph="✗"; state="missing checkout"
  elif [ -z "$tracked" ]; then
    glyph="•"; state="no tracked rev"
  # head is a short hash; in sync if it prefixes the (usually full) tracked rev.
  elif [ "$head" != "-" ] && { case "$tracked" in "$head"*) true ;; *) case "$head" in "$tracked"*) true ;; *) false ;; esac ;; esac; }; then
    glyph="✓"; state="in sync"
  else
    glyph="⚠"; state="differs — run: sync"
  fi

  printf '  %s %-9s %s\n' "$glyph" "$label" "$state"
  if [ -n "$repo_root" ] && [ "$repo_root" != "-" ]; then
    printf '      %-8s %s\n' "path"    "$(tilde "$repo_root")"
    printf '      %-8s %s\n' "head"    "$(short_rev "$head")"
    printf '      %-8s %s\n' "tracked" "$(short_rev "$tracked")"
  fi
}

recap() {
  echo "repo wiring (local checkout each dependency resolves to):"
  recap_line "framehop" "$FRAMEHOP_ROOT" "$(framehop_tracked_rev)"
  recap_line "reader"   "$READER_ROOT"   "$(reader_tracked_rev)"
  recap_line "samply"   "$SAMPLY_ROOT"   "$(samply_tracked_rev)"
}

status() {
  if [ -f "$RUNNER_CONFIG" ]; then
    echo "samply dev mode: ON"
    echo "  $(tilde "$RUNNER_CONFIG") present"
    [ -n "$SAMPLY_ROOT" ] && [ -f "$SAMPLY_CONFIG" ] && echo "  $(tilde "$SAMPLY_CONFIG") present"
    case "$(git -C "$RUNNER_ROOT" ls-files -v Cargo.lock)" in
      S*) echo "  Cargo.lock masked (skip-worktree)" ;;
      *)  echo "  warning: Cargo.lock NOT masked — local edits will show in git" >&2 ;;
    esac
  else
    echo "samply dev mode: OFF"
  fi
}

# Check out the tracked revision in each local checkout. Refuses to touch a
# checkout with uncommitted changes so in-progress dev work is never clobbered.
#   checkout_tracked <label> <repo-root> <tracked-rev>
checkout_tracked() {
  label=$1
  repo_root=$2
  tracked=$3
  if [ -z "$repo_root" ]; then
    echo "  $label: skipped (checkout missing)" >&2
    return 0
  fi
  if [ -z "$tracked" ]; then
    echo "  $label: skipped (no tracked rev in manifest)" >&2
    return 0
  fi
  if ! git -C "$repo_root" diff --quiet || ! git -C "$repo_root" diff --cached --quiet; then
    echo "  $label: SKIPPED — uncommitted changes in $repo_root" >&2
    return 0
  fi
  current=$(git -C "$repo_root" rev-parse HEAD 2>/dev/null || echo "")
  case "$current" in
    "$tracked"*) echo "  $label: already at $tracked"; return 0 ;;
  esac
  if git -C "$repo_root" checkout --quiet "$tracked" 2>/dev/null; then
    echo "  $label: checked out $tracked"
  else
    echo "  $label: rev $tracked not found locally; fetching…" >&2
    git -C "$repo_root" fetch --quiet --all 2>/dev/null || true
    if git -C "$repo_root" checkout --quiet "$tracked" 2>/dev/null; then
      echo "  $label: checked out $tracked"
    else
      echo "  $label: FAILED to check out $tracked" >&2
      return 1
    fi
  fi
}

sync() {
  echo "syncing checkouts to tracked revisions:"
  checkout_tracked "framehop" "$FRAMEHOP_ROOT" "$(framehop_tracked_rev)"
  checkout_tracked "reader"   "$READER_ROOT"   "$(reader_tracked_rev)"
  checkout_tracked "samply"   "$SAMPLY_ROOT"   "$(samply_tracked_rev)"
}

[ $# -eq 1 ] || usage

case "$1" in
  on) enable ;;
  off) disable ;;
  status) status ;;
  sync) sync ;;
  *) usage ;;
esac

# Always close with the repo-wiring recap, whatever the command was.
echo
recap
