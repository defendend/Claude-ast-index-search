#!/usr/bin/env bash
# Post-edit hook: incrementally refresh the ast-index after Edit/Write/MultiEdit.
#
# Behaviour:
#   * No-op when ast-index is not on PATH (plugin must not block sessions).
#   * No-op when this project's `ast-index watch` daemon is already running.
#   * Requests a trailing-debounced coordinator update. Bursts and edits that
#     arrive during an update are not lost.
#   * Skips when no project index exists yet — the SessionStart hook handles
#     the cold-start case.
#   * Returns after the coordinator has safely queued its project worker.

set -u

if ! command -v ast-index >/dev/null 2>&1; then
  exit 0
fi

project_dir="${CLAUDE_PROJECT_DIR:-$PWD}"

if (cd "$project_dir" && ast-index watch-status --quiet >/dev/null 2>&1); then
  exit 0
fi

db_path=$(cd "$project_dir" && ast-index db-path 2>/dev/null) || exit 0
if [ ! -f "$db_path" ]; then
  # No index yet — SessionStart will pick this up.
  exit 0
fi

debounce_ms="${AST_INDEX_HOOK_DEBOUNCE_MS:-5000}"
case "$debounce_ms" in
  ''|*[!0-9]*) debounce_ms=5000 ;;
esac

(cd "$project_dir" && ast-index update --background --debounce-ms "$debounce_ms" >/dev/null)

exit 0
