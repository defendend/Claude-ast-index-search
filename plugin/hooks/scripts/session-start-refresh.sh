#!/usr/bin/env bash
# Session-start hook: keep the ast-index fresh between sessions.
#
# Behaviour:
#   * No-op when ast-index is not on PATH.
#   * No-op when an `ast-index watch` daemon is already running for the tree.
#   * If an index exists: run `ast-index update` synchronously — fast (seconds)
#     and ensures the first searches of the session see external git changes.
#   * If no index exists: print a one-line hint to stderr so the user runs the
#     initial `ast-index rebuild` themselves; we deliberately don't start
#     a multi-minute rebuild on session start.
#   * Bypassable via AST_INDEX_HOOK_SKIP_SESSION_START=1 for opt-out.

set -u

if [ "${AST_INDEX_HOOK_SKIP_SESSION_START:-0}" = "1" ]; then
  exit 0
fi

if ! command -v ast-index >/dev/null 2>&1; then
  exit 0
fi

project_dir="${CLAUDE_PROJECT_DIR:-$PWD}"

if pgrep -f "ast-index watch" >/dev/null 2>&1; then
  exit 0
fi

if (cd "$project_dir" && ast-index stats >/dev/null 2>&1); then
  (cd "$project_dir" && ast-index update >/dev/null 2>&1) || true
else
  echo "ast-index: no index for $project_dir — run 'ast-index rebuild' to enable structural search." >&2
fi

exit 0
