#!/usr/bin/env bash
# ============================================================================
# scripts/check-pr.sh
#
# Single entrypoint that runs every gate a PR has to pass before review:
#   1. cargo build --release --workspace        (compiles the entire workspace)
#   2. cargo test  --release --workspace        (unit + integration + proptest)
#   3. scripts/smoke.sh                         (6 CLI end-to-end scenarios)
#   4. scripts/validate-agent-plugins.sh        (plugin payload + command contract)
#   5. cargo bench --no-run                     (benches compile, not run)
#
# Prints a single combined summary at the end. Exits non-zero if any step
# fails. Intended to be the one command both contributors and the CI run.
#
# Usage:
#   bash scripts/check-pr.sh                # default: 4 steps above
#   RUN_BENCHES=1 bash scripts/check-pr.sh  # also run `cargo bench` in --quick mode
#
# Environment overrides for the smoke step are forwarded — see scripts/smoke.sh.
# ============================================================================

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PASS=0
FAIL=0
RESULTS=()
START=$(date +%s)

run_step() {
    local name="$1"; shift
    local t0 t1
    echo
    echo "==================== $name ===================="
    t0=$(date +%s)
    if "$@"; then
        t1=$(date +%s)
        RESULTS+=("PASS  $name  ($((t1-t0))s)")
        PASS=$((PASS+1))
    else
        t1=$(date +%s)
        RESULTS+=("FAIL  $name  ($((t1-t0))s)")
        FAIL=$((FAIL+1))
    fi
}

run_step "cargo build --release --workspace" \
    cargo build --release --workspace

run_step "cargo test  --release --workspace" \
    cargo test --release --workspace

run_step "scripts/smoke.sh (6 CLI scenarios)" \
    bash "$ROOT/scripts/smoke.sh"

run_step "scripts/validate-agent-plugins.sh" \
    bash "$ROOT/scripts/validate-agent-plugins.sh"

if [ "${RUN_BENCHES:-0}" = "1" ]; then
    run_step "cargo bench --bench parser -- --quick" \
        cargo bench --bench parser -- --quick
else
    run_step "cargo bench --no-run (compile only)" \
        cargo bench --no-run
fi

END=$(date +%s)
echo
echo "==================== check-pr summary ===================="
for r in "${RESULTS[@]}"; do echo "  $r"; done
echo "  total: $PASS passed, $FAIL failed in $((END-START))s"
exit "$FAIL"
