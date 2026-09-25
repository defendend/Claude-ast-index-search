//! Microbenchmarks for the read path against a freshly-built index.
//!
//! The index is built **once** (in a Criterion `setup` closure) over this
//! very repository, then each iteration runs a single query against the
//! shared SQLite connection. Build cost is paid up front and is **not**
//! included in the per-iteration timing.
//!
//! Functions exercised:
//!   * `db::find_files`            — path LIKE '%pattern%' lookup.
//!   * `db::find_symbols_by_name`  — symbol name lookup with kind hint.
//!   * `db::search_refs`           — ref name aggregation by usage count.
//!
//! (`find_symbols` per spec aliases to `find_symbols_by_name`, the actual
//! public entry point in `src/db.rs`.)
//!
//! Run with:
//!     cargo bench --bench db_query
//!     cargo bench --bench db_query -- --quick

use std::path::PathBuf;
use std::sync::OnceLock;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rusqlite::Connection;
use tempfile::TempDir;

use ast_index::{db, indexer};

/// Build an index of this repo once and hold the temp dir for the lifetime
/// of the benchmark process. Each bench fn grabs a fresh connection to the
/// already-populated SQLite file.
struct IndexedRepo {
    _tmp: TempDir,
    db_path: PathBuf,
}

fn indexed_repo() -> &'static IndexedRepo {
    static REPO: OnceLock<IndexedRepo> = OnceLock::new();
    REPO.get_or_init(|| {
        let tmp = TempDir::new().expect("tempdir");
        let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

        // Use the temp dir as the "project root" so the SQLite file lives
        // inside it — but point the indexer at the real repo via the
        // scoped variant. We only want the symbols/refs of *this* crate's
        // sources; we don't want to index the whole workspace target dir.
        let conn = db::open_db(tmp.path()).expect("open_db");
        db::init_db(&conn).expect("init_db");
        drop(conn);

        let mut conn = db::open_db(tmp.path()).expect("reopen_db");
        let src_dir = project_root.join("src");
        let _ =
            indexer::index_directory_scoped(&mut conn, tmp.path(), &src_dir, false, false, None)
                .expect("index_directory_scoped");

        let db_path = db::get_db_path(tmp.path()).expect("db_path");
        IndexedRepo { _tmp: tmp, db_path }
    })
}

fn fresh_conn() -> Connection {
    let repo = indexed_repo();
    Connection::open(&repo.db_path).expect("open conn")
}

fn bench_find_files(c: &mut Criterion) {
    let conn = fresh_conn();
    c.bench_function("db::find_files(\"commands\")", |b| {
        b.iter(|| {
            let r = db::find_files(&conn, black_box("commands"), 50).unwrap();
            black_box(r);
        });
    });
}

fn bench_find_symbols(c: &mut Criterion) {
    let conn = fresh_conn();
    c.bench_function("db::find_symbols_by_name(\"index_directory\")", |b| {
        b.iter(|| {
            let r =
                db::find_symbols_by_name(&conn, black_box("index_directory"), None, 50).unwrap();
            black_box(r);
        });
    });
}

fn bench_search_refs(c: &mut Criterion) {
    let conn = fresh_conn();
    c.bench_function("db::search_refs(\"Connection\")", |b| {
        b.iter(|| {
            let r = db::search_refs(&conn, black_box("Connection"), 50).unwrap();
            black_box(r);
        });
    });
}

criterion_group!(
    benches,
    bench_find_files,
    bench_find_symbols,
    bench_search_refs
);
criterion_main!(benches);
