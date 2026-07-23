#!/usr/bin/env bash
# Post-edit hook: incrementally refresh the ast-index after Edit/Write/MultiEdit.
#
# Behaviour:
#   * No-op when ast-index is not on PATH (plugin must not block sessions).
#   * No-op when an `ast-index watch` daemon is already running for the tree.
#   * Debounced: at most one update per AST_INDEX_HOOK_DEBOUNCE_SEC seconds
#     (default 5). Multiple edits in a burst coalesce into a single update.
#   * Skips when no project index exists yet — the SessionStart hook handles
#     the cold-start case.
#   * Runs the update in the background and detaches stdout/stderr so the
#     hook returns immediately and the agent is never blocked.

set -u

if ! command -v ast-index >/dev/null 2>&1; then
  exit 0
fi

project_dir="${CLAUDE_PROJECT_DIR:-$PWD}"

if pgrep -f "ast-index watch" >/dev/null 2>&1; then
  exit 0
fi

if ! (cd "$project_dir" && ast-index stats >/dev/null 2>&1); then
  # No index yet — SessionStart will pick this up.
  exit 0
fi

cache_dir="${XDG_CACHE_HOME:-$HOME/Library/Caches}/ast-index/hook"
mkdir -p "$cache_dir" 2>/dev/null || cache_dir="/tmp"

# Per-tree debounce marker, hashed so paths don't collide.
hash=$(printf '%s' "$project_dir" | shasum -a 1 2>/dev/null | awk '{print $1}')
if [ -z "$hash" ]; then
  hash=$(printf '%s' "$project_dir" | cksum | awk '{print $1}')
fi
marker="$cache_dir/post-edit-${hash}.stamp"
debounce_sec="${AST_INDEX_HOOK_DEBOUNCE_SEC:-5}"
case "$debounce_sec" in
  ''|*[!0-9]*) debounce_sec=5 ;;
esac

marker_mtime() {
  stat -c %Y "$1" 2>/dev/null && return 0
  stat -f %m "$1" 2>/dev/null && return 0
  printf '0\n'
}

now=$(date +%s)
if [ -f "$marker" ]; then
  last=$(marker_mtime "$marker")
  case "$last" in
    ''|*[!0-9]*) last=0 ;;
  esac
  delta=$((now - last))
  if [ "$delta" -lt "$debounce_sec" ]; then
    exit 0
  fi
fi
touch "$marker" 2>/dev/null || true

(cd "$project_dir" && ast-index update >/dev/null 2>&1 &) >/dev/null 2>&1

exit 0
