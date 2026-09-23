# Testing

## Where tests live

- **Integration** — `tests/<concern>_tests.rs`. One file per concern
  (e.g. `path_resolver_tests.rs`, `update_extra_roots_tests.rs`,
  `swift_issues_tests.rs`, `memory_tests.rs`). Each `#[test]` fn spins up
  its own `TempDir`, builds a real SQLite DB, exercises public library
  APIs.
- **Unit** — `#[cfg(test)] mod tests { … }` at the bottom of the module
  under test. Use this for pure helpers (e.g. regex shape, glob →
  `LIKE` translation, tree-sitter query sanity).
- **Parser regressions** — `tests/<lang>_issues_tests.rs`. Each test feeds
  a real-world code snippet and asserts the expected `(symbols, refs)`.

## Good: integration test shape

Pattern: new `TempDir`, explicit DB setup, exercise public API, assert on
observable state.

```rust
use std::fs;
use ast_index::commands::PathResolver;
use ast_index::db;
use tempfile::TempDir;

fn open_fresh_db(project_root: &std::path::Path) -> rusqlite::Connection {
    if db::db_exists(project_root) {
        db::delete_db(project_root).unwrap();
    }
    let conn = db::open_db(project_root).unwrap();
    db::init_db(&conn).unwrap();
    conn
}

#[test]
fn resolve_returns_absolute_path_in_extra_root() {
    let primary = TempDir::new().unwrap();
    let extra   = TempDir::new().unwrap();
    let conn    = open_fresh_db(primary.path());
    db::add_extra_root(&conn, &extra.path().to_string_lossy()).unwrap();

    let rel  = "src/main/java/deep/BClass.java";
    let file = extra.path().join(rel);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "class BClass {}").unwrap();

    let resolver = PathResolver::from_conn(primary.path(), &conn);
    assert_eq!(resolver.resolve(rel), file.to_string_lossy());
}
```

Rules:

- **Every test gets its own `TempDir`.** Shared state across tests is the
  #1 source of flakes here.
- **Clean slate per test.** If a test requires a DB, create one freshly —
  do not assume state from a previous test.
- **Exercise public APIs**, not private helpers. If a private helper is
  worth a test, it's probably worth promoting or unit-testing in-module.
- **Assert on behaviour, not logs.** If the goal is "prints X with --json",
  capture stdout (`std::process::Command::output`) and assert on bytes.

## Good: unit test next to the code

```rust
// src/commands/mod.rs
pub fn glob_like_escape(s: &str) -> String { /* … */ }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_preserves_plain_chars() {
        assert_eq!(glob_like_escape("Foo.kt"), "Foo.kt");
    }

    #[test]
    fn escape_handles_sql_wildcards() {
        assert_eq!(glob_like_escape("a_b%c"), r"a\_b\%c");
    }
}
```

## Running

```bash
cargo test --release --workspace         # everything, ~600 tests
cargo test --release -p ast-index path_resolver   # single file
cargo test --release -- --nocapture      # see println! output for debugging
```

Release-mode is intentional — some tests walk real filesystems and debug
builds are 3× slower. CI runs `cargo test --release --workspace`.

## Anti-patterns

- **Using the user's real cache dir.** `db::open_db(root)` puts the index in
  the cache base (`<base>/<hash of root>/index.db`), not under `root`.
  `.cargo/config.toml` points `AST_INDEX_CACHE_DIR` at
  `target/ast-index-cache` for everything cargo starts, so `cargo test` never
  writes to `~/Library/Caches/ast-index`. Don't remove that variable in a test
  without setting `AST_INDEX_DB_PATH` or another cache dir, and give spawned
  binaries their own `TempDir` cache (`.env("AST_INDEX_CACHE_DIR", …)`) as the
  CLI tests do. Every run leaves ~90 entries in `target/ast-index-cache`, and
  opening a new project costs more the more entries there are; `rm -r
  target/ast-index-cache` resets it.
- **Mocking `rusqlite`.** Tests use a real SQLite file. It's fast
  (milliseconds) and catches schema/migration mistakes that a mock would
  happily ignore.
- **Tests that depend on ordering with other tests.** `cargo test` runs
  in parallel; design each test to be self-contained.
- **`unwrap()` hiding a real bug.** An `.unwrap()` that can never fire is
  fine. One that ignores the actual failure mode of the API under test is
  a test you shouldn't ship.
- **Giant fixture `String`s inline.** If a parser input is >30 lines, put
  it in `tests/fixtures/<name>.<ext>` and `include_str!` it.
