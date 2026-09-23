//! Relevance ranking and result ordering for the FTS5 symbol searches.
//!
//! The FTS branches order by `bm25()` with `name` weighted far above
//! `signature`, in front of an exact-name tier and behind a path/line
//! tie-break. These tests pin the three properties that combination has to
//! hold: the exact hit leads, a signature-only hit never outranks a name hit,
//! and the same query returns the same page every time.

use ast_index::db::{self, SearchScope, SymbolKind};
use tempfile::TempDir;

fn open_fresh_db(project_root: &std::path::Path) -> rusqlite::Connection {
    if db::db_exists(project_root) {
        db::delete_db(project_root).unwrap();
    }
    let conn = db::open_db(project_root).unwrap();
    db::init_db(&conn).unwrap();
    conn
}

fn names(results: &[db::SearchResult]) -> Vec<&str> {
    results.iter().map(|r| r.name.as_str()).collect()
}

fn located(results: &[db::SearchResult]) -> Vec<String> {
    results
        .iter()
        .map(|r| format!("{}:{}:{}", r.name, r.path, r.line))
        .collect()
}

fn module_scope(prefix: &str) -> SearchScope<'_> {
    SearchScope {
        in_file: None,
        module: Some(prefix),
        dir_prefix: None,
    }
}

// ----------------------------------------------------------------------
// Exact name wins
// ----------------------------------------------------------------------

#[test]
fn exact_name_leads_every_fts_entry_point() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());

    // A base class plus 30 subclasses that name it only in their signature.
    // Every subclass name is shorter than the base class name, which is what
    // made the pre-bm25 length ordering bury the base class.
    let base = db::upsert_file(&conn, "app/services/application_service.rb", 0, 100).unwrap();
    db::insert_symbol(
        &conn,
        base,
        "ApplicationService",
        SymbolKind::Class,
        5,
        Some("class ApplicationService"),
    )
    .unwrap();
    for i in 0..30 {
        let path = format!("app/services/svc_{i:02}_service.rb");
        let file = db::upsert_file(&conn, &path, 0, 100).unwrap();
        let name = format!("Svc{i:02}Service");
        db::insert_symbol(
            &conn,
            file,
            &name,
            SymbolKind::Class,
            3,
            Some(&format!("class {name} < ApplicationService")),
        )
        .unwrap();
    }

    let none = SearchScope::none();
    let scoped = module_scope("app/services");
    let pages = [
        db::search_symbols(&conn, "ApplicationService", 10).unwrap(),
        db::search_symbol_terms_scoped(&conn, &["ApplicationService"], None, 10, &none, false)
            .unwrap(),
        db::search_symbols_scoped(&conn, "ApplicationService", 10, &scoped).unwrap(),
        db::search_symbols_for_command(&conn, "ApplicationService", None, 10, &none, false, false)
            .unwrap(),
        // A `kind` filter binds a parameter between the match and the
        // ordering, so the ranked page has to survive it too.
        db::search_symbol_terms_scoped(
            &conn,
            &["ApplicationService"],
            Some("class"),
            10,
            &scoped,
            false,
        )
        .unwrap(),
        db::search_symbols_for_command(
            &conn,
            "ApplicationService",
            Some("class"),
            10,
            &scoped,
            false,
            true,
        )
        .unwrap(),
    ];

    for page in &pages {
        assert_eq!(
            names(page).first(),
            Some(&"ApplicationService"),
            "{:?}",
            names(page)
        );
        assert_eq!(page.len(), 10, "{:?}", names(page));
    }
}

#[test]
fn a_capitalised_query_prefers_the_type_over_the_accessor() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());
    // FTS folds case, so both rows match `User`; only the ordering separates
    // the class the user asked for from the accessor that shares its name.
    let model = db::upsert_file(&conn, "app/models/user.rb", 0, 100).unwrap();
    db::insert_symbol(
        &conn,
        model,
        "User",
        SymbolKind::Class,
        3,
        Some("class User < ApplicationRecord # a long trailing comment"),
    )
    .unwrap();
    for i in 0..10 {
        let path = format!("app/services/accessor_{i:02}.rb");
        let file = db::upsert_file(&conn, &path, 0, 100).unwrap();
        db::insert_symbol(
            &conn,
            file,
            "user",
            SymbolKind::Function,
            7,
            Some("def user"),
        )
        .unwrap();
    }

    let results = db::search_symbols(&conn, "User", 5).unwrap();
    assert_eq!(
        results[0].path,
        "app/models/user.rb",
        "{:?}",
        located(&results)
    );
}

// ----------------------------------------------------------------------
// Project code before third-party code
// ----------------------------------------------------------------------

fn insert_at(conn: &rusqlite::Connection, path: &str, name: &str, kind: SymbolKind) {
    let file = db::upsert_file(conn, path, 0, 100).unwrap();
    db::insert_symbol(conn, file, name, kind, 3, Some(&format!("class {name}"))).unwrap();
}

#[test]
fn project_hits_lead_vendor_hits_of_the_same_tier() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());
    // `node_modules/` sorts before `system/` and `vendor/`, so the path
    // tie-break alone put the library copy first.
    insert_at(
        &conn,
        "node_modules/react-hot-loader/index.d.ts",
        "AppContainer",
        SymbolKind::Class,
    );
    insert_at(
        &conn,
        "node_modules/react-hot-loader/props.d.ts",
        "AppContainerProps",
        SymbolKind::Interface,
    );
    insert_at(
        &conn,
        "system/container.rb",
        "AppContainer",
        SymbolKind::Class,
    );
    insert_at(
        &conn,
        "system/container_factory.rb",
        "AppContainerFactory",
        SymbolKind::Class,
    );
    // A project-owned `vendor/` directory is project code, not a dependency.
    insert_at(
        &conn,
        "vendor/container.rb",
        "AppContainer",
        SymbolKind::Class,
    );

    let exact = [
        "AppContainer:system/container.rb:3",
        "AppContainer:vendor/container.rb:3",
        "AppContainer:node_modules/react-hot-loader/index.d.ts:3",
    ];
    // The CLI searches by prefix, so it also reaches the partial hits, where the
    // exact vendor hit stays above the project's partial one.
    let none = SearchScope::none();
    assert_eq!(
        located(
            &db::search_symbol_terms_scoped(&conn, &["AppContainer"], None, 10, &none, false)
                .unwrap()
        ),
        [
            &exact[..],
            &[
                "AppContainerFactory:system/container_factory.rb:3",
                "AppContainerProps:node_modules/react-hot-loader/props.d.ts:3",
            ],
        ]
        .concat()
    );
    let token_pages = [
        db::search_symbols(&conn, "AppContainer", 10).unwrap(),
        db::search_symbols_scoped(&conn, "AppContainer", 10, &module_scope("")).unwrap(),
        db::search_symbols_for_command(&conn, "AppContainer", None, 10, &none, false, false)
            .unwrap(),
        db::search_symbol_terms_scoped(&conn, &["AppContainer"], None, 3, &none, true).unwrap(),
    ];
    for page in &token_pages {
        assert_eq!(located(page), exact);
    }
}

#[test]
fn a_library_only_name_keeps_its_exact_hit_first() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());
    // The project never defines `useState`; its own hits only contain it.
    for i in 0..5 {
        insert_at(
            &conn,
            &format!("frontend/hooks/use_state_{i}.js"),
            &format!("useStateModal{i}"),
            SymbolKind::Function,
        );
    }
    insert_at(
        &conn,
        "node_modules/react-use/lib/useStateList.d.ts",
        "useStateList",
        SymbolKind::Function,
    );
    insert_at(
        &conn,
        "node_modules/@types/react/index.d.ts",
        "useState",
        SymbolKind::Function,
    );

    let results =
        db::search_symbol_terms_scoped(&conn, &["useState"], None, 10, &SearchScope::none(), false)
            .unwrap();
    assert_eq!(
        names(&results),
        vec![
            "useState",
            "useStateModal0",
            "useStateModal1",
            "useStateModal2",
            "useStateModal3",
            "useStateModal4",
            "useStateList",
        ]
    );
}

// ----------------------------------------------------------------------
// bm25 column weights
// ----------------------------------------------------------------------

#[test]
fn a_name_hit_outranks_a_signature_only_hit() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());
    // The `w00`…`w19` rows carry `retry` in their signature only, and their
    // names are far shorter, so without the `name` column weight they lead.
    for i in 0..20 {
        let path = format!("app/workers/w{i:02}.rb");
        let file = db::upsert_file(&conn, &path, 0, 100).unwrap();
        let name = format!("w{i:02}");
        db::insert_symbol(
            &conn,
            file,
            &name,
            SymbolKind::Function,
            3,
            Some(&format!("def {name} retry")),
        )
        .unwrap();
    }
    let file = db::upsert_file(&conn, "lib/retry_policy.rb", 0, 100).unwrap();
    db::insert_symbol(
        &conn,
        file,
        "retry_policy",
        SymbolKind::Function,
        1,
        Some("def retry_policy"),
    )
    .unwrap();

    let results = db::search_symbols(&conn, "retry", 5).unwrap();
    assert_eq!(
        names(&results).first(),
        Some(&"retry_policy"),
        "{:?}",
        names(&results)
    );
}

// ----------------------------------------------------------------------
// Determinism
// ----------------------------------------------------------------------

/// Rows sharing a name, a signature and a rank — only the final path/line
/// tie-break can order them.
fn seed_identical_rows(conn: &rusqlite::Connection) {
    for i in 0..25 {
        let path = format!("app/handlers/h{i:02}.rb");
        let file = db::upsert_file(conn, &path, 0, 100).unwrap();
        for line in [4, 9] {
            db::insert_symbol(
                conn,
                file,
                "handle",
                SymbolKind::Function,
                line,
                Some("def handle"),
            )
            .unwrap();
        }
    }
}

#[test]
fn ranked_searches_are_reproducible_across_calls() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());
    seed_identical_rows(&conn);
    let none = SearchScope::none();
    let scoped = module_scope("app/handlers");

    let page = |()| {
        (
            located(&db::search_symbols(&conn, "handle", 12).unwrap()),
            located(
                &db::search_symbol_terms_scoped(&conn, &["handle"], None, 12, &none, false)
                    .unwrap(),
            ),
            located(&db::search_symbols_scoped(&conn, "handle", 12, &scoped).unwrap()),
            located(
                &db::search_symbols_for_command(&conn, "handle", None, 12, &none, false, false)
                    .unwrap(),
            ),
            located(&db::search_symbol_seeds(&conn, "handle", 12).unwrap()),
        )
    };

    let first = page(());
    assert_eq!(first.0.len(), 12);
    for _ in 0..5 {
        assert_eq!(page(()), first);
    }
}

#[test]
fn tied_rows_are_ordered_by_path_then_line() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());
    seed_identical_rows(&conn);

    assert_eq!(
        located(&db::search_symbols(&conn, "handle", 6).unwrap()),
        vec![
            "handle:app/handlers/h00.rb:4",
            "handle:app/handlers/h00.rb:9",
            "handle:app/handlers/h01.rb:4",
            "handle:app/handlers/h01.rb:9",
            "handle:app/handlers/h02.rb:4",
            "handle:app/handlers/h02.rb:9",
        ]
    );
}

// ----------------------------------------------------------------------
// Fallbacks that must keep working
// ----------------------------------------------------------------------

#[test]
fn fuzzy_cascade_still_orders_exact_then_prefix_then_contains() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());
    let file = db::upsert_file(&conn, "app/models/order.rb", 0, 100).unwrap();
    // Inserted worst-match first so a passing test cannot be insertion order.
    db::insert_symbol(&conn, file, "reorder_items", SymbolKind::Function, 30, None).unwrap();
    db::insert_symbol(&conn, file, "order_total", SymbolKind::Function, 20, None).unwrap();
    db::insert_symbol(&conn, file, "order", SymbolKind::Function, 10, None).unwrap();

    let results = db::search_symbols_fuzzy(&conn, "order", 10).unwrap();
    assert_eq!(
        names(&results),
        vec!["order", "order_total", "reorder_items"]
    );
}

#[test]
fn seeds_keep_the_spread_of_matched_names() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());
    // 30 rows literally named `service` would fill a ranked page on their own;
    // `explore` re-ranks candidates itself and needs the other names too.
    for i in 0..30 {
        let path = format!("app/services/plain_{i:02}.rb");
        let file = db::upsert_file(&conn, &path, 0, 100).unwrap();
        db::insert_symbol(
            &conn,
            file,
            "service",
            SymbolKind::Function,
            3,
            Some("def service"),
        )
        .unwrap();
    }
    let file = db::upsert_file(&conn, "app/services/merge_service.rb", 0, 100).unwrap();
    db::insert_symbol(
        &conn,
        file,
        "Applicant::Merge::Service",
        SymbolKind::Class,
        4,
        Some("class Applicant::Merge::Service"),
    )
    .unwrap();

    let seeds = db::search_symbol_seeds(&conn, "service", 40).unwrap();
    assert!(
        names(&seeds).contains(&"Applicant::Merge::Service"),
        "{:?}",
        names(&seeds)
    );

    // The ranked search is the one that deliberately leads with the exact hit.
    let ranked = db::search_symbols(&conn, "service", 5).unwrap();
    assert!(
        ranked.iter().all(|r| r.name == "service"),
        "{:?}",
        names(&ranked)
    );
}
