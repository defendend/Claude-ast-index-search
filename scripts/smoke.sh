#!/usr/bin/env bash
# ============================================================================
# scripts/smoke.sh — end-to-end CLI smoke tests for ast-index
# ============================================================================
#
# WHAT
#   The CLI equivalent of a Playwright suite. Six independent scenarios
#   exercise the real release binary against synthetic Rust projects, then
#   assert on stdout / stderr / exit code / SQLite state.
#
# WHY
#   Catches regressions in real user flows that unit tests miss: project
#   detection, incremental update, extra-root path resolution (PathResolver
#   regression), JSON output schema, and MCP stdio handshake.
#
# SCENARIOS
#   1. fresh-project      Rebuild a tiny Rust project, search a known symbol,
#                         verify project=Rust + file count + symbol present.
#   2. incremental-update After rebuild, add a new .rs file, run update,
#                         verify both new and existing symbols are searchable.
#   3. extra-roots        Rebuild primary, add-root pointing at a sibling
#                         tempdir, update, verify search returns an absolute
#                         path resolved against the extra root (PathResolver
#                         regression).
#   4. json-format        `search --format json` and `stats --format json`
#                         must be parseable JSON with expected top-level keys.
#   5. mcp-stdio          Spawn ast-index-mcp with AST_INDEX_ROOT, send a
#                         JSON-RPC `initialize` then a `tools/call stats`,
#                         verify well-formed JSON-RPC responses.
#   6. perf-budget        Index a copy of this repo's `src/`, then verify
#                         rebuild, search, and no-op update stay within loose
#                         latency budgets.
#
# OUTPUT
#   On failure, the offending command + output is printed and a per-scenario
#   log is written under $WORKDIR/logs/<scenario>.log. Final line is
#   "N/6 scenarios passed". Exit 0 iff N == 6.
#
# USAGE
#   bash scripts/smoke.sh                 # run all scenarios
#   SMOKE_KEEP=1 bash scripts/smoke.sh    # do not delete $WORKDIR on exit
#   SMOKE_BIN_DIR=path bash scripts/smoke.sh   # use prebuilt binaries here
#
# ADDING A NEW SCENARIO
#   1. Define a function `scenario_<name>` that takes one arg (its workdir).
#      Inside it: create fixtures, run `$AST_INDEX` commands, assert with
#      the helpers `assert_eq`, `assert_contains`, `assert_json_key`, etc.
#      Return 0 on success, non-zero on failure. Use `fail "msg"` to abort.
#   2. Append the function name to the SCENARIOS array near the bottom.
#   3. Re-run `bash scripts/smoke.sh` and confirm `N+1/N+1 scenarios passed`.
#
# REQUIREMENTS
#   bash, python3 (for JSON validation), sqlite3 (optional — only for the
#   sqlite probe in fresh-project; gracefully skipped if missing).
# ============================================================================

set -euo pipefail

# ---------------------------------------------------------------------------
# Globals
# ---------------------------------------------------------------------------
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SMOKE_BIN_DIR="${SMOKE_BIN_DIR:-$ROOT/target/release}"
AST_INDEX="$SMOKE_BIN_DIR/ast-index"
AST_INDEX_MCP="$SMOKE_BIN_DIR/ast-index-mcp"

WORKDIR="${TMPDIR:-/tmp}/ast-index-smoke-$$"
LOGDIR="$WORKDIR/logs"
mkdir -p "$LOGDIR"

CURRENT_LOG=""   # set per scenario by run_scenario
PASSED=0
FAILED=0
TOTAL=0
FAILED_NAMES=()

# ---------------------------------------------------------------------------
# Cleanup
# ---------------------------------------------------------------------------
cleanup() {
    local exit_code=$?
    # Kill any leftover background processes (mcp scenario)
    if [ -n "${MCP_PID:-}" ] && kill -0 "$MCP_PID" 2>/dev/null; then
        kill "$MCP_PID" 2>/dev/null || true
        wait "$MCP_PID" 2>/dev/null || true
    fi
    # Clear ast-index project caches we created so we don't pollute
    # ~/Library/Caches/ast-index/ for the user.
    if [ -d "$WORKDIR" ]; then
        for proj in "$WORKDIR"/scenario-*/primary "$WORKDIR"/scenario-*/extra; do
            [ -d "$proj" ] || continue
            (cd "$proj" && "$AST_INDEX" clear >/dev/null 2>&1) || true
        done
    fi
    if [ "${SMOKE_KEEP:-0}" = "1" ]; then
        echo "[smoke] keeping $WORKDIR (SMOKE_KEEP=1)"
    elif [ -d "$WORKDIR" ]; then
        rm -rf "$WORKDIR"
    fi
    exit "$exit_code"
}
trap cleanup EXIT INT TERM

# ---------------------------------------------------------------------------
# Logging / assertion helpers
# ---------------------------------------------------------------------------
log() { printf '%s\n' "$*" | tee -a "$CURRENT_LOG" >&2; }

# Run a command in a given directory, tee its combined output to the scenario
# log, and return its exit code. First arg is the cwd; rest is the command.
# Sets LAST_OUTPUT for assertions. Avoids subshells so LAST_OUTPUT propagates.
LAST_OUTPUT=""
run_in() {
    local cwd="$1"; shift
    local out rc
    out=$(cd "$cwd" && "$@" 2>&1) ; rc=$?
    LAST_OUTPUT="$out"
    {
        printf '$ (cd %s && %s)\n' "$cwd" "$*"
        printf '%s\n' "$out"
        printf '[exit %d]\n\n' "$rc"
    } >> "$CURRENT_LOG"
    return $rc
}

fail() {
    log "FAIL: $*"
    log "----- last output -----"
    log "$LAST_OUTPUT"
    log "-----------------------"
    return 1
}

assert_eq() {
    local actual="$1" expected="$2" what="$3"
    if [ "$actual" != "$expected" ]; then
        fail "$what: expected '$expected', got '$actual'"
        return 1
    fi
}

assert_contains() {
    local haystack="$1" needle="$2" what="$3"
    case "$haystack" in
        *"$needle"*) : ;;
        *) fail "$what: expected to contain '$needle'"; return 1 ;;
    esac
}

assert_not_contains() {
    local haystack="$1" needle="$2" what="$3"
    case "$haystack" in
        *"$needle"*) fail "$what: expected NOT to contain '$needle'"; return 1 ;;
        *) : ;;
    esac
}

assert_json_valid() {
    local payload="$1" what="$2"
    if ! printf '%s' "$payload" | python3 -m json.tool >/dev/null 2>&1; then
        fail "$what: not valid JSON"
        return 1
    fi
}

assert_json_key() {
    local payload="$1" key="$2" what="$3"
    local present
    present=$(printf '%s' "$payload" | python3 -c "
import json, sys
try:
    obj = json.load(sys.stdin)
except Exception as e:
    print('PARSE_ERR'); sys.exit(0)
print('YES' if '$key' in obj else 'NO')
") || true
    if [ "$present" != "YES" ]; then
        fail "$what: missing top-level key '$key' (probe=$present)"
        return 1
    fi
}

# ---------------------------------------------------------------------------
# Build (once)
# ---------------------------------------------------------------------------
ensure_binaries() {
    if [ -x "$AST_INDEX" ] && [ -x "$AST_INDEX_MCP" ]; then
        echo "[smoke] using existing binaries in $SMOKE_BIN_DIR"
        return
    fi
    echo "[smoke] building release binaries (cargo build --release --workspace)"
    (cd "$ROOT" && cargo build --release --workspace) >&2
    if [ ! -x "$AST_INDEX" ] || [ ! -x "$AST_INDEX_MCP" ]; then
        echo "[smoke] FATAL: binaries not present after build" >&2
        exit 2
    fi
}

# ---------------------------------------------------------------------------
# Scenario 1: fresh-project
# ---------------------------------------------------------------------------
scenario_fresh_project() {
    local dir="$1/primary"
    mkdir -p "$dir/src"
    cat > "$dir/Cargo.toml" <<'EOF'
[package]
name = "smoke_fresh"
version = "0.1.0"
edition = "2021"
EOF
    cat > "$dir/src/main.rs" <<'EOF'
fn smoke_fresh_marker() {
    println!("hello smoke");
}

struct SmokeFreshStruct {
    field: i32,
}

fn main() {
    smoke_fresh_marker();
    let _ = SmokeFreshStruct { field: 0 };
}
EOF
    cat > "$dir/src/lib.rs" <<'EOF'
pub fn library_marker() -> i32 { 42 }
EOF

    run_in "$dir" "$AST_INDEX" rebuild || { fail "rebuild exited non-zero"; return 1; }
    assert_contains "$LAST_OUTPUT" "Indexed 2 files" "rebuild file count" || return 1

    run_in "$dir" "$AST_INDEX" detect-stacks || { fail "detect-stacks exited non-zero"; return 1; }
    assert_contains "$LAST_OUTPUT" "Rust (rust)" "detected project stack" || return 1

    run_in "$dir" "$AST_INDEX" search smoke_fresh_marker || { fail "search exited non-zero"; return 1; }
    assert_contains "$LAST_OUTPUT" "smoke_fresh_marker" "search result" || return 1
    assert_contains "$LAST_OUTPUT" "src/main.rs" "search file path" || return 1

    run_in "$dir" "$AST_INDEX" stats --format json || { fail "stats --format json exited non-zero"; return 1; }
    assert_json_valid "$LAST_OUTPUT" "stats json" || return 1
    local files
    files=$(printf '%s' "$LAST_OUTPUT" | python3 -c "import json,sys; print(json.load(sys.stdin)['stats']['file_count'])")
    assert_eq "$files" "2" "stats.file_count" || return 1

    # Optional sqlite probe — symbols table must contain our marker.
    if command -v sqlite3 >/dev/null 2>&1; then
        local db_path
        db_path=$(cd "$dir" && "$AST_INDEX" db-path 2>/dev/null)
        if [ -n "$db_path" ] && [ -f "$db_path" ]; then
            local count
            count=$(sqlite3 "$db_path" "SELECT COUNT(*) FROM symbols WHERE name='smoke_fresh_marker';" 2>/dev/null || echo "0")
            if [ "$count" -lt 1 ]; then
                fail "sqlite probe: symbols table has no smoke_fresh_marker (got $count)"
                return 1
            fi
        fi
    fi
    return 0
}

# ---------------------------------------------------------------------------
# Scenario 2: incremental-update
# ---------------------------------------------------------------------------
scenario_incremental_update() {
    local dir="$1/primary"
    mkdir -p "$dir/src"
    cat > "$dir/Cargo.toml" <<'EOF'
[package]
name = "smoke_incremental"
version = "0.1.0"
edition = "2021"
EOF
    cat > "$dir/src/main.rs" <<'EOF'
fn original_marker() {
    println!("original");
}

fn main() { original_marker(); }
EOF

    run_in "$dir" "$AST_INDEX" rebuild || { fail "initial rebuild"; return 1; }
    assert_contains "$LAST_OUTPUT" "Indexed 1 files" "initial file count" || return 1

    # Add a new .rs file
    cat > "$dir/src/added.rs" <<'EOF'
pub fn newly_added_marker() {
    println!("added");
}
EOF

    run_in "$dir" "$AST_INDEX" update || { fail "update exited non-zero"; return 1; }
    assert_contains "$LAST_OUTPUT" "1 changed" "update should report 1 changed file" || return 1

    run_in "$dir" "$AST_INDEX" search newly_added_marker || { fail "search new symbol"; return 1; }
    assert_contains "$LAST_OUTPUT" "newly_added_marker" "new symbol present" || return 1

    run_in "$dir" "$AST_INDEX" search original_marker || { fail "search original symbol"; return 1; }
    assert_contains "$LAST_OUTPUT" "original_marker" "existing symbol still present" || return 1

    return 0
}

# ---------------------------------------------------------------------------
# Scenario 3: extra-roots (PathResolver regression)
# ---------------------------------------------------------------------------
scenario_extra_roots() {
    local primary="$1/primary"
    local extra="$1/extra"
    mkdir -p "$primary/src" "$extra/src"
    local extra_canonical
    extra_canonical=$(cd "$extra" && pwd -P)

    cat > "$primary/Cargo.toml" <<'EOF'
[package]
name = "smoke_primary"
version = "0.1.0"
edition = "2021"
EOF
    cat > "$primary/src/main.rs" <<'EOF'
fn primary_marker() {}
fn main() { primary_marker(); }
EOF

    cat > "$extra/Cargo.toml" <<'EOF'
[package]
name = "smoke_extra"
version = "0.1.0"
edition = "2021"
EOF
    cat > "$extra/src/lib.rs" <<'EOF'
pub fn extra_root_unique_marker() -> i32 { 7 }
EOF

    run_in "$primary" "$AST_INDEX" rebuild || { fail "primary rebuild"; return 1; }

    run_in "$primary" "$AST_INDEX" add-root "$extra" || { fail "add-root exited non-zero"; return 1; }
    assert_contains "$LAST_OUTPUT" "Added source root" "add-root confirmation" || return 1

    run_in "$primary" "$AST_INDEX" update || { fail "update after add-root"; return 1; }

    run_in "$primary" "$AST_INDEX" search extra_root_unique_marker || { fail "search extra-root symbol"; return 1; }
    assert_contains "$LAST_OUTPUT" "extra_root_unique_marker" "extra-root symbol present" || return 1
    # Regression: the path MUST be absolute (resolved against extra root),
    # not a bare 'src/lib.rs' which would be ambiguous.
    assert_contains "$LAST_OUTPUT" "$extra_canonical/src/lib.rs" "extra-root path is absolute" || return 1

    # The primary symbol should still be searchable.
    run_in "$primary" "$AST_INDEX" search primary_marker || { fail "search primary symbol"; return 1; }
    assert_contains "$LAST_OUTPUT" "primary_marker" "primary symbol still present" || return 1

    return 0
}

# ---------------------------------------------------------------------------
# Scenario 4: json-format
# ---------------------------------------------------------------------------
scenario_json_format() {
    local dir="$1/primary"
    mkdir -p "$dir/src"
    cat > "$dir/Cargo.toml" <<'EOF'
[package]
name = "smoke_json"
version = "0.1.0"
edition = "2021"
EOF
    cat > "$dir/src/main.rs" <<'EOF'
fn json_marker() {}
fn main() { json_marker(); }
EOF

    run_in "$dir" "$AST_INDEX" rebuild || { fail "rebuild"; return 1; }

    run_in "$dir" "$AST_INDEX" search --format json json_marker || { fail "search --format json"; return 1; }
    assert_json_valid "$LAST_OUTPUT" "search json output" || return 1
    for key in symbols files content_matches references; do
        assert_json_key "$LAST_OUTPUT" "$key" "search json" || return 1
    done
    # Sanity: at least one symbol entry with name=json_marker.
    local found
    found=$(printf '%s' "$LAST_OUTPUT" | python3 -c "
import json,sys
obj = json.load(sys.stdin)
syms = obj.get('symbols', [])
print('YES' if any(s.get('name') == 'json_marker' for s in syms) else 'NO')
")
    assert_eq "$found" "YES" "json symbols contains json_marker" || return 1

    run_in "$dir" "$AST_INDEX" stats --format json || { fail "stats --format json"; return 1; }
    assert_json_valid "$LAST_OUTPUT" "stats json output" || return 1
    for key in stats db_path db_size_bytes; do
        assert_json_key "$LAST_OUTPUT" "$key" "stats json" || return 1
    done

    return 0
}

# ---------------------------------------------------------------------------
# Scenario 5: mcp-stdio
# ---------------------------------------------------------------------------
scenario_mcp_stdio() {
    local dir="$1/primary"
    mkdir -p "$dir/src"
    cat > "$dir/Cargo.toml" <<'EOF'
[package]
name = "smoke_mcp"
version = "0.1.0"
edition = "2021"
EOF
    cat > "$dir/src/main.rs" <<'EOF'
fn mcp_marker() {}
fn main() { mcp_marker(); }
EOF

    run_in "$dir" "$AST_INDEX" rebuild || { fail "rebuild for mcp"; return 1; }

    local in_fifo="$1/mcp-in.jsonl"
    local out_file="$1/mcp-out.jsonl"
    cat > "$in_fifo" <<EOF
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"stats","arguments":{}}}
EOF

    # Spawn MCP server in background, feeding it our requests via stdin redirect.
    AST_INDEX_ROOT="$dir" AST_INDEX_BIN="$AST_INDEX" \
        "$AST_INDEX_MCP" < "$in_fifo" > "$out_file" 2>> "$CURRENT_LOG" &
    MCP_PID=$!

    # Wait up to 15s for two response lines to appear (initialize + tools/call).
    local waited=0
    while [ "$waited" -lt 15 ]; do
        if [ -f "$out_file" ]; then
            local lines
            lines=$(wc -l < "$out_file" 2>/dev/null | tr -d ' ')
            if [ "${lines:-0}" -ge 2 ]; then
                break
            fi
        fi
        sleep 1
        waited=$((waited+1))
    done

    # Server should have exited on its own once stdin closed; clean up regardless.
    if kill -0 "$MCP_PID" 2>/dev/null; then
        kill "$MCP_PID" 2>/dev/null || true
    fi
    wait "$MCP_PID" 2>/dev/null || true
    MCP_PID=""

    if [ ! -s "$out_file" ]; then
        LAST_OUTPUT="(mcp produced no output within ${waited}s)"
        fail "mcp server returned no output"
        return 1
    fi

    # Validate response 1: initialize.
    local init_line
    init_line=$(sed -n '1p' "$out_file")
    LAST_OUTPUT="$init_line"
    assert_json_valid "$init_line" "mcp initialize response" || return 1
    local probe
    probe=$(printf '%s' "$init_line" | python3 -c "
import json,sys
obj = json.load(sys.stdin)
ok = (obj.get('jsonrpc') == '2.0'
      and obj.get('id') == 1
      and 'result' in obj
      and obj['result'].get('serverInfo', {}).get('name') == 'ast-index-mcp')
print('YES' if ok else 'NO')
")
    assert_eq "$probe" "YES" "mcp initialize shape" || return 1

    # Validate response 2: tools/call stats.
    local call_line
    call_line=$(sed -n '2p' "$out_file")
    LAST_OUTPUT="$call_line"
    assert_json_valid "$call_line" "mcp tools/call response" || return 1
    local probe2
    probe2=$(printf '%s' "$call_line" | python3 -c "
import json,sys
obj = json.load(sys.stdin)
ok = (obj.get('jsonrpc') == '2.0'
      and obj.get('id') == 2
      and 'result' in obj
      and isinstance(obj['result'].get('content'), list)
      and obj['result'].get('isError') == False)
print('YES' if ok else 'NO')
")
    assert_eq "$probe2" "YES" "mcp tools/call shape" || return 1

    return 0
}

# ---------------------------------------------------------------------------
# Scenario 6: perf-budget
# ---------------------------------------------------------------------------
# Indexes a real-world corpus (this very repo's `src/`) and asserts that
# `rebuild`, a typical `search`, and a no-op `update` finish under loose
# budgets. The budgets are deliberately permissive — meant to catch
# catastrophic regressions (10× slowdown), not microbench drift. Override
# with environment variables if you want to tighten them in CI:
#
#   PERF_REBUILD_MS_MAX  default 30000
#   PERF_SEARCH_MS_MAX   default 500
#   PERF_UPDATE_MS_MAX   default 1000
#
# Uses python3 for sub-second timing because `date +%N` is GNU-only.
ms_now() { python3 -c 'import time; print(int(time.time()*1000))'; }

scenario_perf_budget() {
    local dir="$1/perfproject"
    mkdir -p "$dir"
    # Real-world corpus = the repo's own src/. Copy so the scenario doesn't
    # touch the working tree's index cache.
    cp -R "$ROOT/src" "$dir/src"

    local rebuild_max="${PERF_REBUILD_MS_MAX:-30000}"
    local search_max="${PERF_SEARCH_MS_MAX:-500}"
    local update_max="${PERF_UPDATE_MS_MAX:-1000}"

    local t0 t1 elapsed

    # 1. Rebuild
    t0=$(ms_now)
    run_in "$dir" "$AST_INDEX" rebuild || { fail "rebuild exited non-zero"; return 1; }
    t1=$(ms_now); elapsed=$((t1 - t0))
    log "[perf] rebuild: ${elapsed}ms (budget ${rebuild_max}ms)"
    if [ "$elapsed" -gt "$rebuild_max" ]; then
        fail "rebuild took ${elapsed}ms, exceeds budget ${rebuild_max}ms"
        return 1
    fi

    # 2. Search × 5, take max latency. Queries chosen to hit varied paths.
    local max_search=0
    for q in PathResolver search rebuild SymbolKind ParsedSymbol; do
        t0=$(ms_now)
        run_in "$dir" "$AST_INDEX" search "$q" --format json || { fail "search '$q' failed"; return 1; }
        t1=$(ms_now); elapsed=$((t1 - t0))
        [ "$elapsed" -gt "$max_search" ] && max_search=$elapsed
    done
    log "[perf] search max-latency over 5 queries: ${max_search}ms (budget ${search_max}ms)"
    if [ "$max_search" -gt "$search_max" ]; then
        fail "max search latency ${max_search}ms exceeds budget ${search_max}ms"
        return 1
    fi

    # 3. No-op update — should be near-instant since nothing changed.
    t0=$(ms_now)
    run_in "$dir" "$AST_INDEX" update || { fail "update exited non-zero"; return 1; }
    t1=$(ms_now); elapsed=$((t1 - t0))
    log "[perf] noop update: ${elapsed}ms (budget ${update_max}ms)"
    if [ "$elapsed" -gt "$update_max" ]; then
        fail "noop update took ${elapsed}ms, exceeds budget ${update_max}ms"
        return 1
    fi
    assert_contains "$LAST_OUTPUT" "Index is up to date" "noop update message" || return 1

    return 0
}

# ---------------------------------------------------------------------------
# Scenario runner
# ---------------------------------------------------------------------------
run_scenario() {
    local name="$1"
    local fn="scenario_${name//-/_}"
    local sdir="$WORKDIR/scenario-$name"
    mkdir -p "$sdir"
    CURRENT_LOG="$LOGDIR/$name.log"
    : > "$CURRENT_LOG"

    TOTAL=$((TOTAL+1))
    printf '[smoke] %-22s ... ' "$name"
    if "$fn" "$sdir" >>"$CURRENT_LOG" 2>&1; then
        printf 'PASS\n'
        PASSED=$((PASSED+1))
    else
        printf 'FAIL\n'
        FAILED=$((FAILED+1))
        FAILED_NAMES+=("$name")
        echo "       log: $CURRENT_LOG"
        # Print last 20 lines of the log inline to aid debugging.
        echo "       --- last 20 lines ---"
        tail -n 20 "$CURRENT_LOG" | sed 's/^/       | /'
        echo "       ---------------------"
    fi
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
echo "[smoke] workdir: $WORKDIR"
ensure_binaries
echo "[smoke] binary:  $AST_INDEX ($("$AST_INDEX" version))"

SCENARIOS=(
    fresh-project
    incremental-update
    extra-roots
    json-format
    mcp-stdio
    perf-budget
)

for s in "${SCENARIOS[@]}"; do
    run_scenario "$s"
done

echo
echo "[smoke] $PASSED/$TOTAL scenarios passed"
if [ "$FAILED" -gt 0 ]; then
    echo "[smoke] failed: ${FAILED_NAMES[*]}"
    exit 1
fi
exit 0
