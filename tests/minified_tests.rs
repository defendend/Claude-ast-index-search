//! Minified JavaScript and CSS stay out of the index, out of the grep-based
//! commands and out of `outline` / `imports`.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use ast_index::{db, indexer};
use rusqlite::Connection;
use tempfile::TempDir;

fn open_fresh_db(project_root: &Path) -> Connection {
    if db::db_exists(project_root) {
        db::delete_db(project_root).unwrap();
    }
    let conn = db::open_db(project_root).unwrap();
    db::init_db(&conn).unwrap();
    conn
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

/// A bundle on one line, as a minifier writes it, calling `call` throughout.
fn minified_js(call: &str) -> String {
    let mut bundle = String::from("!function(e){var t={};");
    for i in 0..150 {
        bundle.push_str(&format!("function n{i}(r){{return {call}(r,t)+e[{i}]}}"));
    }
    bundle.push_str("}([]);\n");
    bundle
}

fn minified_css() -> String {
    let mut sheet = String::from("/*! theme v1 | MIT */\n");
    for i in 0..300 {
        sheet.push_str(&format!(".c{i}{{color:#{i:03};margin:0 {i}px}}"));
    }
    sheet.push('\n');
    sheet
}

/// Hand-written source whose long lines are an inline SVG path and a data URI.
fn source_with_long_lines() -> String {
    let mut source = String::from("export function renderLogo(size) {\n");
    source.push_str(&format!(
        "  const path = \"M0 0{}\";\n",
        " L1 2".repeat(900)
    ));
    source.push_str(&format!(
        "  const image = \"data:image/png;base64,{}\";\n",
        "iVBORw0KGgo".repeat(300)
    ));
    for i in 0..30 {
        source.push_str(&format!("  const step{i} = size * {i};\n"));
    }
    source.push_str("  return { path, image };\n}\n");
    source
}

fn project() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write(&root.join("src/app.js"), &source_with_long_lines());
    write(
        &root.join("src/site.css"),
        ".site-header {\n  color: red;\n}\n",
    );
    write(
        &root.join("src/table.ts"),
        &format!("export const table = [{}];\n", "1,".repeat(3000)),
    );
    write(&root.join("public/vendor.js"), &minified_js("chargeCard"));
    write(&root.join("public/theme.css"), &minified_css());
    write(
        &root.join("public/lib.min.js"),
        "function chargeCard(card) {\n  return card;\n}\n",
    );
    write(
        &root.join("public/widget-min.css"),
        ".widget {\n  color: blue;\n}\n",
    );
    tmp
}

fn indexed_paths(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT path FROM files ORDER BY path")
        .unwrap();
    stmt.query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|row| row.unwrap())
        .collect()
}

const SOURCE_FILES: [&str; 3] = ["src/app.js", "src/site.css", "src/table.ts"];

#[test]
fn rebuild_leaves_minified_files_out_of_the_index() {
    let tmp = project();
    let root = tmp.path();
    let mut conn = open_fresh_db(root);
    indexer::index_directory(&mut conn, root, false, false).unwrap();

    assert_eq!(indexed_paths(&conn), SOURCE_FILES);
    assert_eq!(
        db::find_symbols_by_name(&conn, "renderLogo", None, 10)
            .unwrap()
            .len(),
        1
    );
    assert!(db::find_symbols_by_name(&conn, "chargeCard", None, 10)
        .unwrap()
        .is_empty());
}

#[test]
fn rebuild_judges_files_over_the_size_cap_by_their_start() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let bundle = minified_js("chargeCard").trim_end().repeat(200);
    assert!(bundle.len() > 1_000_000);
    write(&root.join("public/huge.js"), &bundle);
    write(&root.join("src/Huge.kt"), &"fun f() {}\n".repeat(100_000));

    let mut conn = open_fresh_db(root);
    indexer::index_directory(&mut conn, root, false, false).unwrap();

    assert_eq!(indexed_paths(&conn), ["src/Huge.kt"]);
}

/// Inserts `path` as an index written before minified files were skipped
/// would hold it: current mtime and size, one symbol.
fn insert_as_older_index_did(conn: &Connection, root: &Path, path: &str) {
    let metadata = fs::metadata(root.join(path)).unwrap();
    let mtime = metadata
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let root_path: String = conn
        .query_row(
            "SELECT root_path FROM files WHERE path = 'src/app.js'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO files (path, root_path, mtime, size) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![path, root_path, mtime, metadata.len() as i64],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO symbols (file_id, name, kind, line, signature)
         VALUES (last_insert_rowid(), 'chargeCard', 'function', 1, 'chargeCard')",
        [],
    )
    .unwrap();
}

#[test]
fn update_drops_minified_files_an_older_index_kept() {
    let tmp = project();
    let root = tmp.path();
    let mut conn = open_fresh_db(root);
    indexer::index_directory(&mut conn, root, false, false).unwrap();
    insert_as_older_index_did(&conn, root, "public/vendor.js");
    insert_as_older_index_did(&conn, root, "public/lib.min.js");
    db::delete_metadata_value(&conn, "minified_filter").unwrap();

    let (_, _, deleted) =
        indexer::update_directory_incremental(&mut conn, root, false, None, None).unwrap();

    assert_eq!(deleted, 2);
    assert_eq!(indexed_paths(&conn), SOURCE_FILES);
    assert!(db::find_symbols_by_name(&conn, "chargeCard", None, 10)
        .unwrap()
        .is_empty());
}

#[test]
fn update_skips_new_minified_files_and_drops_files_that_became_minified() {
    let tmp = project();
    let root = tmp.path();
    let mut conn = open_fresh_db(root);
    indexer::index_directory(&mut conn, root, false, false).unwrap();

    write(&root.join("public/extra.js"), &minified_js("refund"));
    write(
        &root.join("public/extra.min.css"),
        ".x {\n  color: red;\n}\n",
    );
    write(&root.join("src/app.js"), &minified_js("renderLogo"));
    write(&root.join("src/helper.js"), "export function helper() {}\n");

    let (updated, changed, deleted) =
        indexer::update_directory_incremental(&mut conn, root, false, None, None).unwrap();

    assert_eq!((updated, changed, deleted), (1, 1, 1));
    assert_eq!(
        indexed_paths(&conn),
        ["src/helper.js", "src/site.css", "src/table.ts"]
    );
    assert!(db::find_symbols_by_name(&conn, "renderLogo", None, 10)
        .unwrap()
        .is_empty());

    let again = indexer::update_directory_incremental(&mut conn, root, false, None, None).unwrap();
    assert_eq!(again, (0, 0, 0));
}

fn run(root: &Path, cache: &Path, envs: &[(&str, &str)], args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ast-index"));
    command
        .current_dir(root)
        .env("AST_INDEX_CACHE_DIR", cache)
        .env("AST_INDEX_DISABLE_GC", "1")
        .env("NO_COLOR", "1")
        .env_remove("AST_INDEX_DB_PATH")
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .env_remove("AST_INDEX_SKIP_MINIFIED");
    for (key, value) in envs {
        command.env(key, value);
    }
    command.args(args).output().unwrap()
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

/// A Ruby caller and a TODO in real source, the same call and TODO in
/// minified files.
fn grep_project() -> TempDir {
    let tmp = project();
    let root = tmp.path();
    write(
        &root.join("lib/billing.rb"),
        "class Billing\n  def pay(card)\n    # TODO: retry declined cards\n    chargeCard(card)\n  end\nend\n",
    );
    write(
        &root.join("public/todo.min.js"),
        "// TODO: from the bundle\nchargeCard(x);\n",
    );
    tmp
}

#[test]
fn grep_commands_do_not_read_minified_files() {
    let project = grep_project();
    let cache = TempDir::new().unwrap();
    let root = project.path();
    stdout(&run(root, cache.path(), &[], &["rebuild"]));

    let callers = stdout(&run(root, cache.path(), &[], &["callers", "chargeCard"]));
    assert!(callers.contains("lib/billing.rb"), "{callers}");
    let todo = stdout(&run(root, cache.path(), &[], &["todo"]));
    assert!(todo.contains("lib/billing.rb"), "{todo}");
    let search = stdout(&run(root, cache.path(), &[], &["search", "chargeCard"]));
    assert!(search.contains("lib/billing.rb"), "{search}");
    let tree = stdout(&run(root, cache.path(), &[], &["call-tree", "chargeCard"]));
    assert!(tree.contains("pay"), "{tree}");

    for output in [&callers, &todo, &search, &tree] {
        assert!(!output.contains("public/"), "{output}");
    }
}

#[test]
fn outline_and_imports_report_a_minified_file_instead_of_parsing_it() {
    let project = project();
    let cache = TempDir::new().unwrap();
    let root = project.path();

    for file in ["public/vendor.js", "public/lib.min.js"] {
        let outline = stdout(&run(root, cache.path(), &[], &["outline", file]));
        assert_eq!(
            outline,
            format!(
                "Outline of {file}:\n  Skipped: minified file, not analysed \
                 (set AST_INDEX_SKIP_MINIFIED=0 to include minified files).\n"
            )
        );
        let imports = stdout(&run(root, cache.path(), &[], &["imports", file]));
        assert!(imports.starts_with(&format!("Imports in {file}:\n  Skipped: minified")));
    }

    let outline = stdout(&run(root, cache.path(), &[], &["outline", "src/app.js"]));
    assert!(outline.contains("renderLogo"), "{outline}");
}

#[test]
fn the_filter_can_be_turned_off_and_update_cleans_up_once_it_is_back_on() {
    let project = grep_project();
    let cache = TempDir::new().unwrap();
    let root = project.path();
    let off = [("AST_INDEX_SKIP_MINIFIED", "0")];

    stdout(&run(root, cache.path(), &off, &["rebuild"]));
    let files = stdout(&run(root, cache.path(), &off, &["file", "public/"]));
    assert!(files.contains("public/vendor.js"), "{files}");
    assert!(files.contains("public/lib.min.js"), "{files}");
    let outline = stdout(&run(
        root,
        cache.path(),
        &off,
        &["outline", "public/lib.min.js"],
    ));
    assert!(outline.contains("chargeCard"), "{outline}");
    let callers = stdout(&run(root, cache.path(), &off, &["callers", "chargeCard"]));
    assert!(callers.contains("public/todo.min.js"), "{callers}");

    stdout(&run(root, cache.path(), &[], &["update"]));
    let files = stdout(&run(root, cache.path(), &[], &["file", "public/"]));
    assert!(files.contains("No files found."), "{files}");
}
