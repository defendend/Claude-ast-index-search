#!/usr/bin/env bash
# PreToolUse(Grep) hook: gently remind the agent that ast-index exists for
# symbol search. NON-BLOCKING — it always allows the Grep call and only injects
# a short reminder when the pattern looks like a code symbol (a class / method /
# function name), where ast-index is more precise and cheaper than a text grep.
#
# Design:
#   * Never blocks. On any uncertainty it allows silently (exit 0, no output) so
#     the normal permission flow applies and the session is never disrupted.
#   * Only nudges for symbol-like patterns: a single bare identifier (optionally
#     namespaced with `::`), no regex metacharacters, no whitespace. Plain-text
#     and regex searches — the legitimate use of Grep — are left alone.
#   * No jq dependency: reads the pattern with jq if present, else a sed fallback.

set -u

input="$(cat)"

# --- extract the Grep pattern from the hook input JSON ---
pattern=""
if command -v jq >/dev/null 2>&1; then
    pattern="$(printf '%s' "$input" | jq -r '.tool_input.pattern // empty' 2>/dev/null)"
fi
if [ -z "$pattern" ]; then
    # Fallback: pull the first "pattern": "..." value. Best-effort; if it fails
    # we simply do not nudge.
    pattern="$(printf '%s' "$input" \
        | sed -n 's/.*"pattern"[[:space:]]*:[[:space:]]*"\([^"\\]*\)".*/\1/p' \
        | head -1)"
fi

# Nothing to judge — allow silently.
[ -z "$pattern" ] && exit 0

# --- heuristic: does the pattern look like a single code symbol? ---
# Must be a bare identifier (>=3 chars), optionally namespaced (Foo::Bar),
# with no regex metacharacters or whitespace. Anything else is treated as a
# legitimate text/regex search and left alone.
case "$pattern" in
    *[!A-Za-z0-9_:]*) exit 0 ;;   # contains a non-identifier char (space, . * [ ] ( ) | \ etc.)
esac
# Length >= 3 and contains at least one letter.
if [ "${#pattern}" -lt 3 ] || ! printf '%s' "$pattern" | grep -q '[A-Za-z]'; then
    exit 0
fi

# --- symbol-like: inject a non-blocking reminder ---
# `pattern` is already constrained to [A-Za-z0-9_:] above, and the text below
# contains no double quotes or backslashes, so it is safe to embed directly in
# a JSON string without an escaper (no jq/python dependency).
reminder="ast-index is indexed for this project. '${pattern}' looks like a code symbol — prefer ast-index over Grep here: 'ast-index usages ${pattern}' (who uses it), 'ast-index refs ${pattern}' (defs+imports+usages), 'ast-index class/symbol ${pattern}' (definition), or 'ast-index explore ${pattern}' (source + callers + tests in one shot). It is precise (no matches inside comments/strings) and cheaper than grep. Grep stays right for plain text, regex, and non-code files."

# Emit allow + additionalContext (non-blocking).
printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","additionalContext":"%s"}}\n' "$reminder"

exit 0
