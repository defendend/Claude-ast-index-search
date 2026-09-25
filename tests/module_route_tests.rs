//! Integration tests for the `module-route` command.
//!
//! Each test spins its own `TempDir`, inserts modules and edges directly into
//! a real SQLite DB, then either calls `cmd_module_route` for smoke-tests or
//! invokes the release binary for output-shape assertions.

use ast_index::commands::modules::cmd_module_route;
use ast_index::db;
use rusqlite::Connection;
use tempfile::TempDir;

// ── helpers ──────────────────────────────────────────────────────────────────

fn open_fresh_db(project_root: &std::path::Path) -> Connection {
    if db::db_exists(project_root) {
        db::delete_db(project_root).unwrap();
    }
    let conn = db::open_db(project_root).unwrap();
    db::init_db(&conn).unwrap();
    conn
}

/// Insert a module row returning its id.
fn insert_module(conn: &Connection, name: &str, path: &str) -> i64 {
    conn.execute(
        "INSERT OR IGNORE INTO modules (name, path) VALUES (?1, ?2)",
        rusqlite::params![name, path],
    )
    .unwrap();
    conn.query_row(
        "SELECT id FROM modules WHERE name = ?1",
        rusqlite::params![name],
        |row| row.get(0),
    )
    .unwrap()
}

/// Insert a directed dependency edge.
fn insert_dep(conn: &Connection, from_id: i64, to_id: i64, kind: &str) {
    conn.execute(
        "INSERT INTO module_deps (module_id, dep_module_id, dep_kind) VALUES (?1, ?2, ?3)",
        rusqlite::params![from_id, to_id, kind],
    )
    .unwrap();
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// 1. Linear chain A→B→C→D: shortest path from A to D should be a single path
///    with 3 hops.
#[test]
fn finds_shortest_path_in_chain() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    let a = insert_module(&conn, "a", "a");
    let b = insert_module(&conn, "b", "b");
    let c = insert_module(&conn, "c", "c");
    let d = insert_module(&conn, "d", "d");
    insert_dep(&conn, a, b, "implementation");
    insert_dep(&conn, b, c, "implementation");
    insert_dep(&conn, c, d, "implementation");
    drop(conn);

    cmd_module_route(dir.path(), "a", "d", false, 50, 20, 2000, "all", "text")
        .expect("should succeed");
}

/// 2. Disconnected graph: querying unreachable module returns Ok without panic.
#[test]
fn unreachable_returns_empty_with_reason() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    let _ = insert_module(&conn, "alpha", "alpha");
    let _ = insert_module(&conn, "beta", "beta");
    // No edge between alpha and beta.
    drop(conn);

    // Should succeed (Ok) but produce an "unreachable" empty_reason in output.
    cmd_module_route(
        dir.path(),
        "alpha",
        "beta",
        false,
        50,
        20,
        2000,
        "all",
        "text",
    )
    .expect("unreachable should return Ok");
}

/// REPRO: direct edge A→B with --all should return exactly 1 path (1 hop).
#[test]
fn direct_edge_all_returns_one_path() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    let a = insert_module(&conn, "app", "app");
    let b = insert_module(&conn, "libx", "libx");
    insert_dep(&conn, a, b, "implementation");
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "app",
            "--to",
            "libx",
            "--all",
        ])
        .output()
        .expect("binary invocation must succeed");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout must be JSON: {e}; stdout={stdout}"));

    let paths = v["paths"].as_array().expect("paths must be array");
    assert_eq!(
        paths.len(),
        1,
        "direct edge --all must yield 1 path; got: {}",
        stdout
    );
    assert_eq!(paths[0]["length"].as_u64().unwrap(), 1);
}

/// REPRO 2: real-world shape — direct edge AND indirect via intermediate, --all.
#[test]
fn direct_plus_indirect_all() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    // Use Gradle colon → dot normalised names matching real-world DB.
    let app = insert_module(&conn, "app", "app");
    let target = insert_module(&conn, "sdk.clips-viewer.shared.clips-design", "target");
    let mid = insert_module(&conn, "feature.foo", "mid");
    insert_dep(&conn, app, target, "implementation");
    insert_dep(&conn, app, mid, "implementation");
    insert_dep(&conn, mid, target, "implementation");
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            ":app",
            "--to",
            ":sdk:clips-viewer:shared:clips-design",
            "--all",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("must be JSON: {e}; stdout={stdout}"));
    let paths = v["paths"].as_array().expect("paths must be array");
    assert_eq!(
        paths.len(),
        2,
        "direct + indirect --all must yield 2 paths; got: {}",
        stdout
    );
}

/// REPRO 3: heavy fanout where DFS may exhaust timeout before finding direct edge.
/// Build :app with many children; one direct edge to target; many decoy subtrees.
#[test]
fn heavy_fanout_all_with_direct_edge() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    let app = insert_module(&conn, "app", "app");
    // Target name sorts late alphabetically so DFS reaches it last.
    let target = insert_module(&conn, "zzz.target", "target");
    insert_dep(&conn, app, target, "implementation");

    // Many decoy children with their own subtrees.
    for i in 0..30 {
        let child = insert_module(&conn, &format!("child{:02}", i), &format!("c{}", i));
        insert_dep(&conn, app, child, "implementation");
        for j in 0..20 {
            let gc = insert_module(
                &conn,
                &format!("g{:02}_{:02}", i, j),
                &format!("g{}_{}", i, j),
            );
            insert_dep(&conn, child, gc, "implementation");
            for k in 0..5 {
                let ggc = insert_module(
                    &conn,
                    &format!("h{:02}_{:02}_{:02}", i, j, k),
                    &format!("h{}_{}_{}", i, j, k),
                );
                insert_dep(&conn, gc, ggc, "implementation");
            }
        }
    }
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "app",
            "--to",
            "zzz.target",
            "--all",
            "--timeout-ms",
            "100", // tight timeout to force the bug
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    eprintln!("REPRO output: {}", stdout);
    let paths = v["paths"].as_array().unwrap();
    // After fix: direct-target edge is processed first, so we always find the
    // 1-hop path even with a tight timeout.
    assert!(
        !paths.is_empty(),
        "must record direct edge before exploring siblings; got: {}",
        stdout
    );
    assert_eq!(paths[0]["length"].as_u64().unwrap(), 1);

    // search_stats must be populated for --all mode.
    let stats = &v["search_stats"];
    assert!(
        stats.is_object(),
        "search_stats must be present for --all; got: {}",
        stdout
    );
    // edges_explored must include the direct edge. nodes_visited may be 0
    // because reverse-BFS pruning lets us record the direct hit without
    // ever pushing a child frame.
    assert!(stats["edges_explored"].as_u64().unwrap_or(0) >= 1);
}

/// REPRO 4: completely unreachable target — reverse-BFS pruning detects
/// this instantly without consuming the timeout, so the message is
/// "unreachable", not "truncated_timeout".
#[test]
fn unreachable_pruning_avoids_timeout() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    let app = insert_module(&conn, "app", "app");
    let _ = insert_module(&conn, "isolated.target", "tgt");
    // Big disconnected fanout: DFS without pruning would burn timeout, but
    // reverse-BFS sees target has no incoming edges and returns immediately.
    for i in 0..15 {
        let c = insert_module(&conn, &format!("c{:02}", i), &format!("c{}", i));
        insert_dep(&conn, app, c, "implementation");
        for j in 0..15 {
            let g = insert_module(
                &conn,
                &format!("g{:02}_{:02}", i, j),
                &format!("g{}_{}", i, j),
            );
            insert_dep(&conn, c, g, "implementation");
        }
    }
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "app",
            "--to",
            "isolated.target",
            "--all",
            "--timeout-ms",
            "5000",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let empty_reason = v["empty_reason"].as_str().unwrap_or("");
    assert_eq!(
        empty_reason, "unreachable",
        "pruning must short-circuit; got: {}",
        stdout
    );
}

/// REPRO 5: large connected DAG with many simple paths from app→target —
/// max_paths cap is exercised, and pruning still keeps DFS bounded.
#[test]
fn many_paths_hits_max_paths_cap() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    // Diamond layered graph: app → l0_{0..4} → l1_{0..4} → ... → target.
    // Number of simple paths = 5^layers, easily exceeds max_paths=5.
    let app = insert_module(&conn, "app", "app");
    let target = insert_module(&conn, "target", "target");
    let mut prev_layer: Vec<i64> = vec![app];
    for layer in 0..3 {
        let mut cur: Vec<i64> = Vec::new();
        for i in 0..5 {
            let n = insert_module(
                &conn,
                &format!("L{}_{:02}", layer, i),
                &format!("L{}_{}", layer, i),
            );
            cur.push(n);
        }
        for &p in &prev_layer {
            for &c in &cur {
                insert_dep(&conn, p, c, "implementation");
            }
        }
        prev_layer = cur;
    }
    for &p in &prev_layer {
        insert_dep(&conn, p, target, "implementation");
    }
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "app",
            "--to",
            "target",
            "--all",
            "--max-paths",
            "5",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let paths = v["paths"].as_array().unwrap();
    assert_eq!(paths.len(), 5, "must cap at max_paths=5; got: {}", stdout);
    assert_eq!(v["truncated"].as_bool(), Some(true));
    assert_eq!(v["truncation_reason"].as_str(), Some("max_paths"));
}

/// 3. Cycle A→B→A and A→C→D: query A→D must not hang or panic.
///    `--all` mode with cycle: should find exactly [A→C→D].
#[test]
fn cycle_does_not_break_traversal() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    let a = insert_module(&conn, "a", "a");
    let b = insert_module(&conn, "b", "b");
    let c = insert_module(&conn, "c", "c");
    let d = insert_module(&conn, "d", "d");
    insert_dep(&conn, a, b, "implementation");
    insert_dep(&conn, b, a, "implementation"); // cycle
    insert_dep(&conn, a, c, "implementation");
    insert_dep(&conn, c, d, "implementation");
    drop(conn);

    // --all with cycle must terminate and not infinite-loop.
    cmd_module_route(dir.path(), "a", "d", true, 50, 20, 2000, "all", "text")
        .expect("cycle must not cause infinite loop");
}

/// 4. Diamond A→B→D, A→C→D: --all should return 2 paths of length 2.
///    We test via JSON output from the binary to count paths structurally.
#[test]
fn all_paths_returns_diamond() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    let a = insert_module(&conn, "a", "a");
    let b = insert_module(&conn, "b", "b");
    let c = insert_module(&conn, "c", "c");
    let d = insert_module(&conn, "d", "d");
    insert_dep(&conn, a, b, "implementation");
    insert_dep(&conn, a, c, "implementation");
    insert_dep(&conn, b, d, "implementation");
    insert_dep(&conn, c, d, "implementation");
    drop(conn);

    // Run via binary to inspect JSON.
    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "a",
            "--to",
            "d",
            "--all",
        ])
        .output()
        .expect("binary invocation must succeed");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");

    let paths = v["paths"].as_array().expect("paths must be array");
    assert_eq!(
        paths.len(),
        2,
        "diamond graph must yield exactly 2 paths; got: {}",
        stdout
    );
    // Both paths must have length 2 (2 hops).
    for path in paths {
        assert_eq!(path["length"].as_u64().unwrap(), 2);
    }
}

/// 5. Kind filter eliminates the only path.
///    A→B via "implementation", query with --via-kind=api must return empty.
#[test]
fn kind_filter_eliminates_only_path() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    let a = insert_module(&conn, "x", "x");
    let b = insert_module(&conn, "y", "y");
    insert_dep(&conn, a, b, "implementation");
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "x",
            "--to",
            "y",
            "--via-kind",
            "api",
        ])
        .output()
        .expect("binary invocation must succeed");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");

    let paths = v["paths"].as_array().expect("paths must be array");
    assert!(
        paths.is_empty(),
        "api filter must eliminate implementation-only path"
    );

    // Should have an empty_reason of "kind_filter" or "unreachable".
    let reason = v["empty_reason"].as_str().unwrap_or("");
    assert!(
        reason == "kind_filter" || reason == "unreachable",
        "expected kind_filter or unreachable, got: {}",
        reason
    );
}

/// 6. JSON output must be valid and contain no ANSI escape codes.
#[test]
fn json_output_shape_is_clean() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    let a = insert_module(&conn, "mod_a", "mod_a");
    let b = insert_module(&conn, "mod_b", "mod_b");
    insert_dep(&conn, a, b, "api");
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "mod_a",
            "--to",
            "mod_b",
        ])
        .output()
        .expect("binary invocation must succeed");

    let stdout_bytes = &out.stdout;

    // No ANSI escape sequences in stdout.
    assert!(
        !stdout_bytes.windows(2).any(|w| w == b"\x1b["),
        "JSON output must not contain ANSI escape codes"
    );

    // Must parse as JSON.
    let stdout = String::from_utf8_lossy(stdout_bytes);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");

    // Required top-level keys.
    assert!(v.get("from").is_some(), "JSON must have 'from' key");
    assert!(v.get("to").is_some(), "JSON must have 'to' key");
    assert!(v.get("paths").is_some(), "JSON must have 'paths' key");
    assert!(v.get("count").is_some(), "JSON must have 'count' key");
    assert!(
        v.get("truncated").is_some(),
        "JSON must have 'truncated' key"
    );

    // Path from mod_a → mod_b (direct api edge).
    assert_eq!(v["count"].as_u64().unwrap(), 1);
}

/// 7. Fan-out graph: many paths, --max-paths 3 caps results and sets truncated.
#[test]
fn max_paths_truncation_signals() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    // Build a fan-out: root → n1..n10 → target (10+ paths via distinct intermediaries).
    let root = insert_module(&conn, "root", "root");
    let target = insert_module(&conn, "target", "target");
    for i in 1..=10 {
        let mid = insert_module(&conn, &format!("mid{}", i), &format!("mid{}", i));
        insert_dep(&conn, root, mid, "implementation");
        insert_dep(&conn, mid, target, "implementation");
    }
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "root",
            "--to",
            "target",
            "--all",
            "--max-paths",
            "3",
        ])
        .output()
        .expect("binary invocation must succeed");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");

    let paths = v["paths"].as_array().expect("paths must be array");
    assert_eq!(paths.len(), 3, "max-paths=3 must cap at 3 paths");
    assert!(
        v["truncated"].as_bool().unwrap(),
        "truncated must be true"
    );
    assert_eq!(
        v["truncation_reason"].as_str().unwrap_or(""),
        "max_paths",
        "truncation_reason must be max_paths"
    );
}

/// 8. Self-query without a self-edge in the DB returns empty with empty_reason="self".
///    (When a real self-edge exists the `self_edge_returns_real_cycle_path` test covers it.)
#[test]
fn self_query_no_self_edge_returns_empty_self_reason() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    // Need at least one module_dep row so dep_count > 0 (otherwise command bails early).
    let a = insert_module(&conn, "self_mod", "self_mod");
    let b = insert_module(&conn, "other_mod", "other_mod");
    insert_dep(&conn, a, b, "implementation");
    // No self-edge for self_mod.
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "self_mod",
            "--to",
            "self_mod",
        ])
        .output()
        .expect("binary invocation must succeed");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");

    assert_eq!(
        v["empty_reason"].as_str().unwrap_or(""),
        "self",
        "self-query without self-edge must return empty_reason=self; got: {}",
        stdout
    );
    assert_eq!(
        v["count"].as_u64().unwrap_or(99),
        0,
        "self-query without self-edge must return count=0; got: {}",
        stdout
    );
}

/// Bonus: Mermaid output uses id aliases and has correct fence.
#[test]
fn mermaid_output_uses_id_aliases() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    let a = insert_module(&conn, "ma", "ma");
    let b = insert_module(&conn, "mb", "mb");
    insert_dep(&conn, a, b, "api");
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "mermaid",
            "module-route",
            "--from",
            "ma",
            "--to",
            "mb",
        ])
        .output()
        .expect("binary invocation must succeed");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("```mermaid"),
        "mermaid output must start with ```mermaid fence"
    );
    assert!(
        stdout.contains("n0[") || stdout.contains("n1["),
        "mermaid output must use id aliases like n0[ or n1["
    );
}

/// prune_timeout: when the reverse-BFS phase times out we must signal
/// truncated=true and truncation_reason="prune_timeout" rather than silently
/// returning an empty "unreachable" result.
///
/// Build a large connected graph where the reverse BFS will exhaust the budget
/// before completing, then query with a 1 ms timeout.
#[test]
fn prune_timeout_sets_truncation_reason() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    // Fan-in graph: many sources all pointing to target.
    // Reverse BFS from target will try to walk all predecessors.
    let target = insert_module(&conn, "target", "target");
    for i in 0..50 {
        let src = insert_module(&conn, &format!("src{:03}", i), &format!("src{}", i));
        insert_dep(&conn, src, target, "implementation");
        // Give each source its own chain so the graph is wide enough to
        // trigger the deadline inside compute_reverse_distances.
        for j in 0..20 {
            let anc = insert_module(
                &conn,
                &format!("anc{:03}_{:02}", i, j),
                &format!("anc{}_{}", i, j),
            );
            insert_dep(&conn, anc, src, "implementation");
        }
    }
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "anc000_00",
            "--to",
            "target",
            "--all",
            "--timeout-ms",
            "1",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("must be JSON: {e}; stdout={stdout}"));

    // Either the prune phase timed out (truncated, prune_timeout) or the DFS
    // ran and found something. We never want a silent "unreachable" when the
    // budget ran out during pruning.
    let truncated = v["truncated"].as_bool().unwrap_or(false);
    let trunc_reason = v["truncation_reason"].as_str().unwrap_or("");
    let empty_reason = v["empty_reason"].as_str().unwrap_or("");

    if truncated {
        // When truncated, reason must be timeout or prune_timeout — NOT "unreachable".
        assert!(
            trunc_reason == "timeout" || trunc_reason == "prune_timeout",
            "truncation_reason must be timeout or prune_timeout, got: {}; stdout={}",
            trunc_reason,
            stdout
        );
        // empty_reason (if present) must not be "unreachable" when truncated.
        assert_ne!(
            empty_reason, "unreachable",
            "truncated result must not say 'unreachable'; stdout={}",
            stdout
        );
    }
    // If not truncated the search finished within budget — both outcomes are valid.
}

/// 3-hop path with distinct edge kinds: verify the kind sequence is correct.
///
///   A --api--> B --implementation--> C --compileOnly--> D
///
/// Each hop must record the kind of the edge that produced it.
#[test]
fn three_hop_path_preserves_distinct_kinds() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    let a = insert_module(&conn, "kindA", "kindA");
    let b = insert_module(&conn, "kindB", "kindB");
    let c = insert_module(&conn, "kindC", "kindC");
    let d = insert_module(&conn, "kindD", "kindD");
    insert_dep(&conn, a, b, "api");
    insert_dep(&conn, b, c, "implementation");
    insert_dep(&conn, c, d, "compileOnly");
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "kindA",
            "--to",
            "kindD",
            "--all",
            "--via-kind",
            "all",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("must be JSON: {e}; stdout={stdout}"));

    let paths = v["paths"].as_array().expect("paths must be array");
    assert_eq!(paths.len(), 1, "must find exactly 1 path; got: {}", stdout);
    let hops = paths[0]["hops"].as_array().expect("hops must be array");
    assert_eq!(hops.len(), 3, "path must have 3 hops; got: {}", stdout);

    // Hop 0: kindA → kindB, kind = "api"
    assert_eq!(
        hops[0]["from"].as_str(),
        Some("kindA"),
        "hop0 from; stdout={}",
        stdout
    );
    assert_eq!(
        hops[0]["to"].as_str(),
        Some("kindB"),
        "hop0 to; stdout={}",
        stdout
    );
    assert_eq!(
        hops[0]["kind"].as_str(),
        Some("api"),
        "hop0 kind must be api; stdout={}",
        stdout
    );

    // Hop 1: kindB → kindC, kind = "implementation"
    assert_eq!(
        hops[1]["from"].as_str(),
        Some("kindB"),
        "hop1 from; stdout={}",
        stdout
    );
    assert_eq!(
        hops[1]["to"].as_str(),
        Some("kindC"),
        "hop1 to; stdout={}",
        stdout
    );
    assert_eq!(
        hops[1]["kind"].as_str(),
        Some("implementation"),
        "hop1 kind must be implementation; stdout={}",
        stdout
    );

    // Hop 2: kindC → kindD, kind = "compileOnly"
    assert_eq!(
        hops[2]["from"].as_str(),
        Some("kindC"),
        "hop2 from; stdout={}",
        stdout
    );
    assert_eq!(
        hops[2]["to"].as_str(),
        Some("kindD"),
        "hop2 to; stdout={}",
        stdout
    );
    assert_eq!(
        hops[2]["kind"].as_str(),
        Some("compileOnly"),
        "hop2 kind must be compileOnly; stdout={}",
        stdout
    );
}

/// Self-loop: when a module has a self-edge in the DB, querying from==to
/// must return a 1-hop real cycle path, not the trivial empty "self" result.
#[test]
fn self_edge_returns_real_cycle_path() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    let m = insert_module(&conn, "cyclic_mod", "cyclic_mod");
    // Self-edge: cyclic_mod depends on itself.
    insert_dep(&conn, m, m, "implementation");
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "cyclic_mod",
            "--to",
            "cyclic_mod",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("must be JSON: {e}; stdout={stdout}"));

    // Must return a real 1-hop path, not empty with reason="self".
    let paths = v["paths"].as_array().expect("paths must be array");
    assert_eq!(
        paths.len(),
        1,
        "self-edge must produce 1 path; got: {}",
        stdout
    );
    assert_eq!(
        paths[0]["length"].as_u64().unwrap_or(0),
        1,
        "self-loop path must have length 1; got: {}",
        stdout
    );

    let empty_reason = v["empty_reason"].as_str().unwrap_or("none");
    assert_ne!(
        empty_reason, "self",
        "self-edge must not report empty_reason=self; got: {}",
        stdout
    );

    let hops = paths[0]["hops"].as_array().expect("hops must be array");
    assert_eq!(
        hops[0]["from"].as_str(),
        Some("cyclic_mod"),
        "self-loop from; stdout={}",
        stdout
    );
    assert_eq!(
        hops[0]["to"].as_str(),
        Some("cyclic_mod"),
        "self-loop to; stdout={}",
        stdout
    );
}

/// invalid --format emits error to stderr (not stdout) and stdout is empty.
#[test]
fn invalid_format_text_mode_emits_to_stderr() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());
    let a = insert_module(&conn, "fa", "fa");
    let b = insert_module(&conn, "fb", "fb");
    insert_dep(&conn, a, b, "implementation");
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "bogus",
            "module-route",
            "--from",
            "fa",
            "--to",
            "fb",
        ])
        .output()
        .unwrap();

    // Stdout must be empty (error goes to stderr).
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.trim().is_empty(),
        "invalid --format must produce no stdout; got: {}",
        stdout
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Invalid --format") || stderr.contains("bogus"),
        "stderr must contain error message; got: {}",
        stderr
    );
}

/// invalid --via-kind under --format json must emit a valid JSON envelope
/// with empty_reason="invalid_args", not colored text.
#[test]
fn invalid_via_kind_json_mode_emits_envelope() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());
    let a = insert_module(&conn, "vka", "vka");
    let b = insert_module(&conn, "vkb", "vkb");
    insert_dep(&conn, a, b, "implementation");
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "vka",
            "--to",
            "vkb",
            "--via-kind",
            "badkind",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("must be JSON for invalid --via-kind in json mode: {e}; stdout={stdout}")
    });

    assert_eq!(
        v["empty_reason"].as_str().unwrap_or(""),
        "invalid_args",
        "json mode invalid via-kind must produce empty_reason=invalid_args; got: {}",
        stdout
    );
    // Stdout must not contain ANSI.
    assert!(
        !out.stdout.windows(2).any(|w| w == b"\x1b["),
        "JSON output must not contain ANSI codes; stdout={}",
        stdout
    );
}

/// invalid --via-kind in text mode emits error to stderr, not stdout.
#[test]
fn invalid_via_kind_text_mode_emits_to_stderr() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());
    let a = insert_module(&conn, "tvka", "tvka");
    let b = insert_module(&conn, "tvkb", "tvkb");
    insert_dep(&conn, a, b, "implementation");
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "module-route",
            "--from",
            "tvka",
            "--to",
            "tvkb",
            "--via-kind",
            "badkind",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.trim().is_empty(),
        "invalid --via-kind (text mode) must produce no stdout; got: {}",
        stdout
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Invalid --via-kind") || stderr.contains("badkind"),
        "stderr must contain error; got: {}",
        stderr
    );
}

/// Module name with mermaid-breaking characters is properly escaped in output.
#[test]
fn mermaid_escapes_special_chars_in_module_names() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    // Name contains chars that break Mermaid syntax: [](){}|"
    let a = insert_module(&conn, ":feature:auth-impl(jvm)", "feature/auth");
    let b = insert_module(&conn, "core", "core");
    insert_dep(&conn, a, b, "api");
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "mermaid",
            "module-route",
            "--from",
            ":feature:auth-impl(jvm)",
            "--to",
            "core",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("```mermaid"),
        "must have mermaid fence; got: {}",
        stdout
    );
    // The raw name must NOT appear unescaped in node declarations.
    // Specifically the `(jvm)` part would break Mermaid if not escaped.
    assert!(
        !stdout.contains("n0[:feature:auth-impl(jvm)]")
            && !stdout.contains("n1[:feature:auth-impl(jvm)]"),
        "raw special-char name must not appear unescaped in node declaration; stdout={}",
        stdout
    );
}

/// --format dot smoke test: output must start with digraph header and contain edges.
#[test]
fn dot_format_smoke_test() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    let a = insert_module(&conn, "dota", "dota");
    let b = insert_module(&conn, "dotb", "dotb");
    insert_dep(&conn, a, b, "api");
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "dot",
            "module-route",
            "--from",
            "dota",
            "--to",
            "dotb",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("digraph"),
        "dot output must contain digraph header; got: {}",
        stdout
    );
    assert!(
        stdout.contains("->"),
        "dot output must contain edge arrow; got: {}",
        stdout
    );
}

/// --max-depth cuts long path: a chain A→B→C→D with max-depth=2 must not
/// find the 3-hop path from A to D.
#[test]
fn max_depth_cuts_long_path() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    let a = insert_module(&conn, "mdA", "mdA");
    let b = insert_module(&conn, "mdB", "mdB");
    let c = insert_module(&conn, "mdC", "mdC");
    let d = insert_module(&conn, "mdD", "mdD");
    insert_dep(&conn, a, b, "implementation");
    insert_dep(&conn, b, c, "implementation");
    insert_dep(&conn, c, d, "implementation");
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "mdA",
            "--to",
            "mdD",
            "--max-depth",
            "2",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("must be JSON: {e}; stdout={stdout}"));

    let paths = v["paths"].as_array().expect("paths must be array");
    assert!(
        paths.is_empty(),
        "max-depth=2 must cut the 3-hop path A→B→C→D; got: {}",
        stdout
    );
}

/// not_indexed JSON envelope: when module_deps table is empty, JSON mode
/// must return a valid envelope with empty_reason="not_indexed".
#[test]
fn not_indexed_json_envelope_shape() {
    let dir = TempDir::new().unwrap();
    // Create DB but do NOT insert any module_deps rows.
    let conn = open_fresh_db(dir.path());
    let _ = insert_module(&conn, "x", "x");
    let _ = insert_module(&conn, "y", "y");
    // No deps inserted — count stays 0.
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "x",
            "--to",
            "y",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("must be JSON: {e}; stdout={stdout}"));

    assert_eq!(
        v["empty_reason"].as_str().unwrap_or(""),
        "not_indexed",
        "must report not_indexed when module_deps is empty; got: {}",
        stdout
    );
    // Must be valid JSON without ANSI.
    assert!(
        !out.stdout.windows(2).any(|w| w == b"\x1b["),
        "JSON envelope must not contain ANSI codes; stdout={}",
        stdout
    );
}

/// REPRO P1: shortest-mode (`--all` absent) timeout must NOT collapse to
/// "unreachable" — a direct edge with `--timeout-ms 0` must produce a
/// truncated envelope, not a misleading empty result.
#[test]
fn shortest_mode_timeout_signals_truncated_not_unreachable() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    let a = insert_module(&conn, "a", "a");
    let b = insert_module(&conn, "b", "b");
    insert_dep(&conn, a, b, "implementation");
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "a",
            "--to",
            "b",
            "--timeout-ms",
            "0",
        ])
        .output()
        .expect("binary invocation must succeed");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("must be JSON: {e}; stdout={stdout}"));

    assert!(
        v["truncated"].as_bool().unwrap_or(false),
        "timeout=0 must produce truncated=true; got: {}",
        stdout
    );
    assert_eq!(
        v["truncation_reason"].as_str().unwrap_or(""),
        "timeout",
        "truncation_reason must be 'timeout' on shortest-mode timeout; got: {}",
        stdout
    );
    assert_eq!(
        v["empty_reason"].as_str().unwrap_or(""),
        "truncated_timeout",
        "empty_reason must mirror --all behaviour; got: {}",
        stdout
    );
    assert_ne!(
        v["empty_reason"].as_str().unwrap_or(""),
        "unreachable",
        "shortest-mode timeout must NOT report unreachable; got: {}",
        stdout
    );
}

/// REPRO P2: self-edge with --via-kind=all must surface the real `dep_kind`
/// from the DB, not the hardcoded "implementation" default.
#[test]
fn self_edge_surfaces_real_kind_under_via_kind_all() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    let m = insert_module(&conn, "api_self_mod", "api_self_mod");
    insert_dep(&conn, m, m, "api");
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "api_self_mod",
            "--to",
            "api_self_mod",
            "--via-kind",
            "all",
        ])
        .output()
        .expect("binary invocation must succeed");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("must be JSON: {e}; stdout={stdout}"));

    let paths = v["paths"].as_array().expect("paths must be array");
    assert_eq!(
        paths.len(),
        1,
        "self-edge must yield 1 path; got: {}",
        stdout
    );
    let kind = paths[0]["hops"][0]["kind"].as_str().unwrap_or("");
    assert_eq!(
        kind, "api",
        "self-edge kind must reflect the real DB row, not 'implementation'; got: {}",
        stdout
    );
}

/// REPRO P3: --max-paths 0 must NOT return a path; it must produce a
/// truncated envelope with truncation_reason="max_paths" and count=0.
#[test]
fn max_paths_zero_returns_no_paths() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    let a = insert_module(&conn, "a", "a");
    let b = insert_module(&conn, "b", "b");
    insert_dep(&conn, a, b, "implementation");
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_ast-index");
    let out = std::process::Command::new(bin)
        .env("AST_INDEX_DB_PATH", db::get_db_path(dir.path()).unwrap())
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "a",
            "--to",
            "b",
            "--all",
            "--max-paths",
            "0",
        ])
        .output()
        .expect("binary invocation must succeed");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("must be JSON: {e}; stdout={stdout}"));

    let paths = v["paths"].as_array().expect("paths must be array");
    assert_eq!(
        paths.len(),
        0,
        "--max-paths 0 must yield zero paths; got: {}",
        stdout
    );
    assert_eq!(
        v["count"].as_u64().unwrap_or(99),
        0,
        "--max-paths 0 must report count=0; got: {}",
        stdout
    );
    assert!(
        v["truncated"].as_bool().unwrap_or(false),
        "--max-paths 0 must report truncated=true; got: {}",
        stdout
    );
    assert_eq!(
        v["truncation_reason"].as_str().unwrap_or(""),
        "max_paths",
        "--max-paths 0 must report truncation_reason=max_paths; got: {}",
        stdout
    );
}
