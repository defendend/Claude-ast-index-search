#!/bin/bash
# Benchmark: Python vs Rust kotlin-index
# Запуск: cd /Users/defendend/go-client-android && bash /tmp/kotlin-index-rs/benchmark.sh

echo "=== kotlin-index Benchmark ==="
echo "Project: $(pwd)"
echo ""

RUST_BIN="/tmp/kotlin-index-rs/target/release/kotlin-index"

if [ ! -f "$RUST_BIN" ]; then
    echo "ERROR: Rust binary not found. Build first:"
    echo "  cd /tmp/kotlin-index-rs && cargo build --release"
    exit 1
fi

echo "| Command | Python | Rust |"
echo "|---------|--------|------|"

# todo
PY_TIME=$( { time kotlin-index todo >/dev/null 2>&1; } 2>&1 | grep real | awk '{print $2}' )
RS_TIME=$( { time $RUST_BIN todo >/dev/null 2>&1; } 2>&1 | grep real | awk '{print $2}' )
echo "| todo | $PY_TIME | $RS_TIME |"

# callers
PY_TIME=$( { time kotlin-index callers onClick >/dev/null 2>&1; } 2>&1 | grep real | awk '{print $2}' )
RS_TIME=$( { time $RUST_BIN callers onClick >/dev/null 2>&1; } 2>&1 | grep real | awk '{print $2}' )
echo "| callers onClick | $PY_TIME | $RS_TIME |"

# suspend
PY_TIME=$( { time kotlin-index suspend >/dev/null 2>&1; } 2>&1 | grep real | awk '{print $2}' )
RS_TIME=$( { time $RUST_BIN suspend >/dev/null 2>&1; } 2>&1 | grep real | awk '{print $2}' )
echo "| suspend | $PY_TIME | $RS_TIME |"

# provides
PY_TIME=$( { time kotlin-index provides Interactor >/dev/null 2>&1; } 2>&1 | grep real | awk '{print $2}' )
RS_TIME=$( { time $RUST_BIN provides Interactor >/dev/null 2>&1; } 2>&1 | grep real | awk '{print $2}' )
echo "| provides Interactor | $PY_TIME | $RS_TIME |"

echo ""
echo "Binary sizes:"
echo "  Python venv: $(du -sh ~/.cache/kotlin-index 2>/dev/null | cut -f1 || echo 'N/A')"
echo "  Rust binary: $(ls -lh $RUST_BIN | awk '{print $5}')"
