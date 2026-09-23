//! Module graph from Swift manifests: SwiftPM `Package.swift` and Tuist
//! `Project.swift` targets become modules pointing at their source
//! directories, and their `dependencies:` become module_deps edges.

use std::fs;
use std::path::Path;

use ast_index::{db, indexer};
use tempfile::TempDir;

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn index(root: &Path, manifests: &[&str]) -> rusqlite::Connection {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();
    let files: Vec<_> = manifests.iter().map(|m| root.join(m)).collect();
    indexer::index_modules_from_files(&conn, root, &files).unwrap();
    indexer::index_module_dependencies(&mut conn, root, &files, false).unwrap();
    conn
}

fn module_path(conn: &rusqlite::Connection, name: &str) -> String {
    conn.query_row(
        "SELECT path FROM modules WHERE name = ?1",
        [name],
        |row| row.get(0),
    )
    .unwrap_or_else(|_| panic!("module {name} not indexed"))
}

fn deps(conn: &rusqlite::Connection, name: &str) -> Vec<(String, String)> {
    let mut deps: Vec<_> = indexer::get_module_deps(conn, name)
        .unwrap()
        .into_iter()
        .map(|(dep, _path, kind)| (dep, kind))
        .collect();
    deps.sort();
    deps
}

fn pair(name: &str, kind: &str) -> (String, String) {
    (name.to_string(), kind.to_string())
}

#[test]
fn tuist_targets_become_modules_with_cross_project_deps() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(
        root,
        "Modules/Core/Project.swift",
        r#"
let project = Project(
    name: Core.self,
    targets: [
        .spmSwiftFolderTarget(
            name: .CoreRouting,
            dependencies: [
                .external(name: "Foundationish"),
                .target(.CoreMap),
                .external(name: "ThirdPartySDK"),
                .system(.MapKit),
            ]
        ),
        .spmSwiftFolderTarget(name: .CoreMap),
        .spmUnitTestsFolderTarget(name: .CoreRoutingTests, dependencies: [.target(.CoreRouting)]),
    ]
)
"#,
    );
    write(root, "Modules/Core/Sources/CoreRouting/Router.swift", "");
    write(root, "Modules/Core/Sources/CoreMap/Map.swift", "");
    write(root, "Modules/Core/Tests/CoreRoutingTests/RouterTests.swift", "");
    write(
        root,
        "Modules/Foundation/Project.swift",
        r#"let project = Project(name: "Foundation", targets: [.spmSwiftFolderTarget(name: .Foundationish)])"#,
    );
    write(root, "Modules/Foundation/Sources/Foundationish/F.swift", "");

    let conn = index(
        root,
        &["Modules/Core/Project.swift", "Modules/Foundation/Project.swift"],
    );

    assert_eq!(module_path(&conn, "CoreRouting"), "Modules/Core/Sources/CoreRouting");
    assert_eq!(module_path(&conn, "CoreRoutingTests"), "Modules/Core/Tests/CoreRoutingTests");
    assert_eq!(
        deps(&conn, "CoreRouting"),
        [pair("CoreMap", "target"), pair("Foundationish", "external")]
    );
    assert_eq!(deps(&conn, "CoreRoutingTests"), [pair("CoreRouting", "target")]);
}

#[test]
fn swiftpm_targets_point_at_sources_and_resolve_dependencies() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(
        root,
        "Packages/Kit/Package.swift",
        r#"
let package = Package(
    name: "Kit",
    products: [.library(name: "Kit", targets: ["Kit"])],
    targets: [
        .target(name: "Kit", dependencies: ["KitCore"]),
        .target(name: "KitCore", path: "Core"),
        .testTarget(name: "KitTests", dependencies: [.target(name: "Kit")]),
    ]
)
"#,
    );
    write(root, "Packages/Kit/Sources/Kit/Kit.swift", "");
    write(root, "Packages/Kit/Core/Core.swift", "");
    write(root, "Packages/Kit/Tests/KitTests/KitTests.swift", "");

    let conn = index(root, &["Packages/Kit/Package.swift"]);

    assert_eq!(module_path(&conn, "Kit"), "Packages/Kit/Sources/Kit");
    assert_eq!(module_path(&conn, "KitCore"), "Packages/Kit/Core");
    assert_eq!(module_path(&conn, "KitTests"), "Packages/Kit/Tests/KitTests");
    assert_eq!(deps(&conn, "Kit"), [pair("KitCore", "target")]);
    assert_eq!(deps(&conn, "KitTests"), [pair("Kit", "target")]);
}

#[test]
fn colliding_target_names_are_qualified_and_resolved_locally() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    for pkg in ["A", "B"] {
        write(
            root,
            &format!("Packages/{pkg}/Package.swift"),
            &format!(
                r#"let package = Package(name: "{pkg}", targets: [.target(name: "{pkg}Feature", dependencies: ["Utils"]), .target(name: "Utils")])"#
            ),
        );
    }
    write(
        root,
        "App/Project.swift",
        r#"let project = Project(name: "App", targets: [.spmSwiftFolderTarget(name: .App, dependencies: [.external(name: "Utils"), .external(name: "AFeature")])])"#,
    );

    let conn = index(
        root,
        &["Packages/A/Package.swift", "Packages/B/Package.swift", "App/Project.swift"],
    );

    assert_eq!(module_path(&conn, "Packages.A.Utils"), "Packages/A/Sources/Utils");
    assert_eq!(module_path(&conn, "Packages.B.Utils"), "Packages/B/Sources/Utils");
    assert_eq!(deps(&conn, "AFeature"), [pair("Packages.A.Utils", "target")]);
    assert_eq!(deps(&conn, "BFeature"), [pair("Packages.B.Utils", "target")]);
    // `Utils` is ambiguous from outside its package, so only AFeature resolves.
    assert_eq!(deps(&conn, "App"), [pair("AFeature", "external")]);
}

#[test]
fn deps_only_rebuild_finds_manifests_from_module_paths() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(
        root,
        "App/Project.swift",
        r#"let project = Project(name: "App", targets: [.spmSwiftFolderTarget(name: .A, dependencies: [.target(.B)]), .spmSwiftFolderTarget(name: .B)])"#,
    );
    write(root, "App/Sources/A/A.swift", "");
    write(root, "App/Sources/B/B.swift", "");

    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();
    indexer::index_modules_from_files(&conn, root, &[root.join("App/Project.swift")]).unwrap();

    let files = indexer::collect_build_files_from_db(&conn, root).unwrap();
    assert_eq!(files, [root.join("App/Project.swift")]);
    indexer::index_module_dependencies(&mut conn, root, &files, false).unwrap();
    assert_eq!(deps(&conn, "A"), [pair("B", "target")]);
}

#[test]
fn tuist_manifest_marks_ios_project() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("Tuist.swift"), "").unwrap();
    assert!(indexer::has_ios_markers(dir.path()));
}
