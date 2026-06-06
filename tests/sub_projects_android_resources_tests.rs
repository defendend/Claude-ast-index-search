//! Regression: `ast-index rebuild --sub-projects` must index Android
//! resources even when a sub-project has no res/layout/menu/navigation
//! XML files. Previously both XML usages and resource indexing were gated
//! behind `any_android && !all_xml_files.is_empty()`, so a Compose / library
//! module with only res/values/ silently skipped resource indexing.

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

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn resource_names_of_type(conn: &Connection, kind: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT name FROM resources WHERE type = ?1 ORDER BY name")
        .unwrap();
    stmt.query_map([kind], |row| row.get::<_, String>(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
}

#[test]
fn sub_project_with_values_but_no_layouts_still_indexes_resources() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Single Android sub-project with res/values/ only — no layouts/menu/navigation.
    let app = root.join("myapp");
    write(&app.join("build.gradle.kts"), "plugins { id(\"com.android.application\") }\n");
    write(
        &app.join("src/main/res/values/strings.xml"),
        r#"<resources>
    <string name="app_title">Hello</string>
    <string name="ok_button">OK</string>
</resources>
"#,
    );
    write(
        &app.join("src/main/res/values/colors.xml"),
        r#"<resources>
    <color name="brand_primary">#FF0000</color>
</resources>
"#,
    );
    // Add a second top-level dir so find_sub_projects returns ≥ 2 entries
    // and the --sub-projects path is actually exercised.
    let lib = root.join("mylib");
    write(&lib.join("build.gradle.kts"), "plugins { id(\"com.android.library\") }\n");

    // Drive the sub-projects path the way the CLI does.
    let mut conn = open_fresh_db(root);
    let subs = indexer::find_sub_projects(root, None, None);
    assert!(subs.len() >= 2, "expected sub-projects to be detected");

    // Walk each sub-project, accumulate resource paths exactly as cmd_rebuild_sub_projects does.
    let mut all_res = Vec::new();
    let mut all_xml_layouts = Vec::new();
    let mut any_android = false;
    for (path, _) in &subs {
        let walk =
            indexer::index_directory_scoped(&mut conn, root, path, false, false, None).unwrap();
        if indexer::has_android_markers(path) || !walk.res_files.is_empty() {
            any_android = true;
        }
        all_res.extend(walk.res_files);
        all_xml_layouts.extend(walk.xml_layout_files);
    }
    assert!(any_android, "any_android must be true for android sub-project");
    assert!(
        all_xml_layouts.is_empty(),
        "test fixture has no layout/menu/navigation xml on purpose"
    );
    assert!(!all_res.is_empty(), "res/values/*.xml must be collected");

    let (resource_count, _) =
        indexer::index_resources(&mut conn, root, &all_res, false).unwrap();
    assert!(
        resource_count >= 3,
        "expected ≥ 3 resources (app_title, ok_button, brand_primary), got {}: {:?} / {:?}",
        resource_count,
        resource_names_of_type(&conn, "string"),
        resource_names_of_type(&conn, "color"),
    );

    let strings = resource_names_of_type(&conn, "string");
    assert!(strings.contains(&"app_title".to_string()));
    assert!(strings.contains(&"ok_button".to_string()));

    let colors = resource_names_of_type(&conn, "color");
    assert!(colors.contains(&"brand_primary".to_string()));
}

#[test]
fn non_android_res_subdir_does_not_get_picked_up() {
    // Regression: previously /res/ substring matched everything (Python
    // `assets/res/data.json` would be falsely treated as Android resource).
    // After the narrower filter, only known Android subdirs trigger.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Tree with a `/res/` segment but no Android subdir below it.
    write(
        &root.join("ml/res/dataset/labels.xml"),
        "<root><label>not-android</label></root>\n",
    );
    write(
        &root.join("ml/res/raw_inputs/data.bin"),
        "binary",
    );

    let mut conn = open_fresh_db(root);
    let walk =
        indexer::index_directory_scoped(&mut conn, root, root, false, false, None).unwrap();

    // None of these should land in res_files — they are not under an
    // Android-canonical subdir (values, layout, drawable, menu, ...).
    assert!(
        walk.res_files.is_empty(),
        "non-android /res/ tree leaked into res_files: {:?}",
        walk.res_files
    );
    assert!(walk.xml_layout_files.is_empty());
}

#[test]
fn values_qualifier_dirs_are_recognised() {
    // `res/values-pl/` (locale qualifier), `res/drawable-xhdpi/` etc.
    // must still be picked up — narrowing the filter must not break them.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write(&root.join("app/build.gradle.kts"), "");
    write(
        &root.join("app/src/main/res/values-pl/strings.xml"),
        r#"<resources>
    <string name="hello">Cześć</string>
</resources>
"#,
    );
    write(
        &root.join("app/src/main/res/drawable-xhdpi/ic_launcher.xml"),
        "<vector/>",
    );

    let mut conn = open_fresh_db(root);
    let walk =
        indexer::index_directory_scoped(&mut conn, root, root, false, false, None).unwrap();
    assert!(
        walk.res_files.len() >= 2,
        "qualifier res dirs not collected: {:?}",
        walk.res_files
    );
    // strings.xml under res/values-pl/ must NOT be a layout file.
    assert!(walk.xml_layout_files.is_empty());
}
