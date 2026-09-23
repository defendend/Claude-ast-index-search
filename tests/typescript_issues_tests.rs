//! Regression tests for TypeScript-specific indexing issues.

use std::fs;
use std::path::Path;

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

#[test]
fn usages_follow_generic_calls_import_aliases_and_local_rebinding() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    fs::create_dir_all(root.join("src/lib/bus")).unwrap();
    fs::create_dir_all(root.join("src/features/a")).unwrap();
    fs::create_dir_all(root.join("src/features/b")).unwrap();
    fs::create_dir_all(root.join("src/features/c")).unwrap();

    fs::write(root.join("package.json"), "{ \"name\": \"ts-issue-38\" }\n").unwrap();
    fs::write(
        root.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "paths": {
      "@lib/*": ["src/lib/*"]
    }
  }
}"#,
    )
    .unwrap();

    fs::write(
        root.join("src/lib/bus/bus.ts"),
        r#"export const targetFn = <T>() => ({
  run(p: T) {
    return p;
  }
});
"#,
    )
    .unwrap();

    fs::write(
        root.join("src/features/a/featureA_baseline.ts"),
        "import { targetFn } from '../../lib/bus/bus';\n\nexport const r = targetFn();\n",
    )
    .unwrap();

    fs::write(
        root.join("src/features/a/featureA_generic.ts"),
        "import { targetFn } from '../../lib/bus/bus';\n\nexport const bare = targetFn<{ id: string }>();\nexport const passed = [targetFn<{ id: string }>()];\n",
    )
    .unwrap();

    fs::write(
        root.join("src/features/b/featureB.ts"),
        "import { targetFn as bus } from '../../lib/bus/bus';\n\nexport const r = bus();\n",
    )
    .unwrap();

    fs::write(
        root.join("src/features/c/featureC.ts"),
        "import { targetFn } from '../../lib/bus/bus';\n\nconst localBus = targetFn<{ id: string }>;\n\nexport const c1 = localBus().run({ id: 'c-1' });\nexport const c2 = localBus().run({ id: 'c-2' });\nexport const c3 = localBus().run({ id: 'c-3' });\n",
    )
    .unwrap();

    let mut conn = open_fresh_db(root);
    let result = indexer::index_directory(&mut conn, root, false, false).unwrap();
    assert!(result.file_count >= 5, "expected ts files to be indexed");

    let refs = db::find_references(&conn, "targetFn", 50).unwrap();
    let hits: Vec<(String, i64)> = refs.iter().map(|r| (r.path.clone(), r.line)).collect();

    for expected in [
        ("src/features/a/featureA_baseline.ts", 3i64),
        ("src/features/a/featureA_generic.ts", 3i64),
        ("src/features/a/featureA_generic.ts", 4i64),
        ("src/features/b/featureB.ts", 3i64),
        ("src/features/c/featureC.ts", 5i64),
        ("src/features/c/featureC.ts", 6i64),
        ("src/features/c/featureC.ts", 7i64),
    ] {
        assert!(
            hits.contains(&(expected.0.to_string(), expected.1)),
            "missing targetFn usage at {:?}; got {:?}",
            expected,
            hits
        );
    }

    assert!(
        refs.len() >= 7,
        "expected at least 7 targetFn usages, got {}: {:?}",
        refs.len(),
        hits
    );
}

fn symbols_in(conn: &Connection, path: &str) -> Vec<(String, String, i64, Option<i64>)> {
    let mut stmt = conn
        .prepare(
            "SELECT s.name, s.kind, s.line, s.end_line FROM symbols s \
             JOIN files f ON s.file_id = f.id WHERE f.path = ?1 ORDER BY s.line",
        )
        .unwrap();
    stmt.query_map([path], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

#[test]
fn anonymous_default_export_is_indexed_under_its_module_name() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let write = |rel: &str, body: &str| {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    };
    write("package.json", "{ \"name\": \"anonymous-default\" }\n");
    write(
        "src/hooks/useMap.js",
        "const searchPath = ({ value }) => {\n  return value;\n};\n\nexport default ({ form }) => {\n  const path = searchPath(form);\n  return path;\n};\n",
    );
    write(
        "src/components/Button/index.jsx",
        "export default ({ label }) => (\n  <button>{label}</button>\n);\n",
    );
    write(
        "src/named.js",
        "export default function useNamed() {\n  return 1;\n}\n",
    );

    let mut conn = Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();
    indexer::index_directory(&mut conn, root, false, false).unwrap();

    assert_eq!(
        symbols_in(&conn, "src/hooks/useMap.js"),
        vec![
            ("searchPath".to_string(), "function".to_string(), 1, Some(3)),
            ("useMap".to_string(), "function".to_string(), 5, Some(8)),
        ]
    );
    assert_eq!(
        symbols_in(&conn, "src/components/Button/index.jsx"),
        vec![("Button".to_string(), "class".to_string(), 1, Some(3))]
    );
    assert_eq!(
        symbols_in(&conn, "src/named.js"),
        vec![("useNamed".to_string(), "function".to_string(), 1, Some(3))]
    );

    let owner = db::find_owning_symbol(&conn, "src/hooks/useMap.js", 6)
        .unwrap()
        .expect("the searchPath call sits inside the default export");
    assert_eq!(owner.name, "useMap");
}

#[test]
fn default_export_of_a_package_build_index_is_named_after_the_package() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let write = |rel: &str, body: &str| {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    };
    write("package.json", "{ \"name\": \"build-index\" }\n");
    let declaration = "export default class {\n  mount(node: Element): void;\n}\n";
    write("node_modules/stylish/dist/index.d.ts", declaration);
    write(
        "node_modules/@scope/toaster/lib/esm/index.d.ts",
        declaration,
    );

    let mut conn = Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();
    indexer::index_node_modules_dts(&mut conn, root, false).unwrap();

    for (path, package) in [
        ("node_modules/stylish/dist/index.d.ts", "stylish"),
        ("node_modules/@scope/toaster/lib/esm/index.d.ts", "toaster"),
    ] {
        assert_eq!(
            symbols_in(&conn, path),
            vec![(package.to_string(), "class".to_string(), 1, Some(3))],
            "{path}"
        );
    }
}
