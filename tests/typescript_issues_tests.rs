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
