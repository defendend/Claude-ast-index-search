//! Target and dependency extraction from Swift manifests: SwiftPM
//! `Package.swift` and Tuist `Project.swift`, parsed with tree-sitter-swift.
//!
//! Both declare targets as call expressions inside a `targets` array literal —
//! `.target(name: "Foo", dependencies: ["Bar"])` in SwiftPM, or a Tuist
//! helper such as `.spmSwiftFolderTarget(name: .Foo, dependencies: [.target(.Bar)])`.
//! The array may be the `targets:` argument of `Package(...)`/`Project(...)`
//! or a `let targets = [...]` declaration passed in later. Manifests are not
//! evaluated: targets produced by helper functions are not discovered.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use tree_sitter::{Language, Node, Parser};

static SWIFT: LazyLock<Language> = LazyLock::new(|| tree_sitter_swift::LANGUAGE.into());

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestTarget {
    pub name: String,
    /// Explicit `path:` argument, relative to the manifest directory.
    pub path: Option<String>,
    /// Static directory prefix of the first `sources:` glob, if any.
    pub sources_dir: Option<String>,
    pub dependencies: Vec<ManifestDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestDependency {
    pub name: String,
    /// Declaration form: `target`, `external`, `project`, `product`, ...
    pub kind: String,
}

/// Dependency forms that never refer to a source module of the workspace.
const NON_MODULE_DEPENDENCIES: &[&str] = &["sdk", "system", "xcframework", "framework", "library"];

pub fn parse_manifest(content: &str) -> Vec<ManifestTarget> {
    let mut parser = Parser::new();
    if parser.set_language(&SWIFT).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(content, None) else {
        return Vec::new();
    };
    let mut arrays = Vec::new();
    collect_target_arrays(tree.root_node(), content, &mut arrays);
    arrays
        .into_iter()
        .flat_map(|array| named_children(array))
        .filter_map(|element| parse_target(element, content))
        .collect()
}

/// Source directory of a manifest target: explicit `path:`, then the static
/// prefix of `sources:`, then the SwiftPM conventions `Sources/<name>` and
/// `Tests/<name>`. Falls back to `Sources/<name>` when nothing exists on disk.
pub fn target_dir(manifest_dir: &Path, target: &ManifestTarget) -> PathBuf {
    if let Some(path) = target.path.as_deref().filter(|p| !p.is_empty()) {
        return manifest_dir.join(path);
    }
    if let Some(dir) = target.sources_dir.as_deref().filter(|p| !p.is_empty()) {
        return manifest_dir.join(dir);
    }
    let conventional = [
        manifest_dir.join("Sources").join(&target.name),
        manifest_dir.join("Tests").join(&target.name),
    ];
    conventional
        .iter()
        .find(|dir| dir.is_dir())
        .cloned()
        .unwrap_or_else(|| conventional[0].clone())
}

/// Array literals holding target declarations: the `targets:` argument of a
/// call, or the value of a `let/var ...targets` declaration.
fn collect_target_arrays<'t>(node: Node<'t>, content: &str, out: &mut Vec<Node<'t>>) {
    let array = match node.kind() {
        "value_argument" => node
            .child_by_field_name("name")
            .filter(|label| text(*label, content) == "targets")
            .and_then(|_| node.child_by_field_name("value")),
        "property_declaration" => node
            .child_by_field_name("name")
            .filter(|name| {
                let name = text(*name, content);
                name.ends_with("targets") || name.ends_with("Targets")
            })
            .and_then(|_| node.child_by_field_name("value")),
        _ => None,
    };
    if let Some(array) = array.filter(|v| v.kind() == "array_literal") {
        out.push(array);
        return;
    }
    for child in named_children(node) {
        collect_target_arrays(child, content, out);
    }
}

fn parse_target(element: Node, content: &str) -> Option<ManifestTarget> {
    let (_, args) = parse_call(element, content)?;
    let name = labeled(&args, "name").and_then(|v| simple_name(v, content))?;
    let path = labeled(&args, "path").and_then(|v| string_literal(v, content));
    let sources_dir = labeled(&args, "sources")
        .and_then(|v| first_string_literal(v, content))
        .map(|glob| static_glob_prefix(&glob));
    let dependencies = labeled(&args, "dependencies")
        .filter(|v| v.kind() == "array_literal")
        .map(|deps| {
            named_children(deps)
                .into_iter()
                .filter_map(|dep| parse_dependency(dep, content))
                .collect()
        })
        .unwrap_or_default();
    Some(ManifestTarget {
        name,
        path,
        sources_dir,
        dependencies,
    })
}

fn parse_dependency(element: Node, content: &str) -> Option<ManifestDependency> {
    if let Some(name) = string_literal(element, content) {
        return Some(ManifestDependency {
            name,
            kind: "target".to_string(),
        });
    }
    let (callee, args) = parse_call(element, content)?;
    if NON_MODULE_DEPENDENCIES.contains(&callee.as_str()) {
        return None;
    }
    // `.project(Other.self, target: .Foo)` names the project positionally and the
    // target by label, so the `target:` label wins over positional arguments.
    let name = labeled(&args, "target")
        .or_else(|| labeled(&args, "name"))
        .or_else(|| args.iter().find(|(label, _)| label.is_none()).map(|(_, v)| *v))
        .and_then(|v| simple_name(v, content))?;
    Some(ManifestDependency { name, kind: callee })
}

type Args<'t> = Vec<(Option<String>, Node<'t>)>;

/// Callee name (`target` for `.target(...)`, `Target.target(...)`, `target(...)`)
/// and the labeled/positional arguments of a call expression.
fn parse_call<'t>(node: Node<'t>, content: &str) -> Option<(String, Args<'t>)> {
    if node.kind() != "call_expression" {
        return None;
    }
    let callee = node.named_child(0)?;
    let callee = simple_name(callee, content)?;
    let suffix = named_children(node)
        .into_iter()
        .find(|c| c.kind() == "call_suffix")?;
    let arguments = named_children(suffix)
        .into_iter()
        .find(|c| c.kind() == "value_arguments")?;
    let args = named_children(arguments)
        .into_iter()
        .filter(|arg| arg.kind() == "value_argument")
        .filter_map(|arg| {
            let value = arg.child_by_field_name("value")?;
            let label = arg
                .child_by_field_name("name")
                .map(|label| text(label, content).to_string());
            Some((label, value))
        })
        .collect();
    Some((callee, args))
}

fn labeled<'t>(args: &Args<'t>, label: &str) -> Option<Node<'t>> {
    args.iter()
        .find(|(l, _)| l.as_deref() == Some(label))
        .map(|(_, value)| *value)
}

/// `"Foo"`, `.Foo`, `Foo`, `Foo.self`, or `Namespace.Foo` → `Foo`.
fn simple_name(node: Node, content: &str) -> Option<String> {
    match node.kind() {
        "line_string_literal" => string_literal(node, content),
        "simple_identifier" => Some(text(node, content).to_string()),
        "prefix_expression" => node
            .child_by_field_name("target")
            .filter(|t| t.kind() == "simple_identifier")
            .map(|t| text(t, content).to_string()),
        "navigation_expression" => {
            let suffix = node
                .child_by_field_name("suffix")?
                .child_by_field_name("suffix")?;
            match text(suffix, content) {
                "self" => simple_name(node.child_by_field_name("target")?, content),
                name => Some(name.to_string()),
            }
        }
        _ => None,
    }
}

/// Text of a plain string literal; `None` when it contains interpolation.
fn string_literal(node: Node, content: &str) -> Option<String> {
    if node.kind() != "line_string_literal" {
        return None;
    }
    let parts = named_children(node);
    match parts.as_slice() {
        [part] if part.kind() == "line_str_text" => Some(text(*part, content).to_string()),
        _ => None,
    }
}

fn first_string_literal(node: Node, content: &str) -> Option<String> {
    string_literal(node, content).or_else(|| {
        named_children(node)
            .into_iter()
            .find_map(|child| first_string_literal(child, content))
    })
}

fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn text<'a>(node: Node, content: &'a str) -> &'a str {
    node.utf8_text(content.as_bytes()).unwrap_or("")
}

/// `Sources/Foo/**/*.swift` → `Sources/Foo`.
fn static_glob_prefix(glob: &str) -> String {
    let cut = glob.find(['*', '{', '?', '[']).unwrap_or(glob.len());
    let prefix = &glob[..cut];
    let prefix = match prefix.rfind('/') {
        Some(slash) if cut < glob.len() => &prefix[..slash],
        _ => prefix,
    };
    prefix.trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dep(name: &str, kind: &str) -> ManifestDependency {
        ManifestDependency {
            name: name.to_string(),
            kind: kind.to_string(),
        }
    }

    #[test]
    fn parses_swiftpm_targets_and_dependencies() {
        let manifest = r#"
// swift-tools-version:5.9
let package = Package(
    name: "Core",
    products: [.library(name: "Core", targets: ["Core", "Utils"])],
    dependencies: [.package(path: "../Other")],
    targets: [
        .target(name: "Core", dependencies: ["Utils", .product(name: "Other", package: "Other")]),
        .target(name: "Utils", path: "Lib/Utils"), // trailing comment, with comma
        /* .target(name: "Commented"), */
        .testTarget(name: "CoreTests", dependencies: [.target(name: "Core")]),
    ]
)
"#;
        let targets = parse_manifest(manifest);
        let names: Vec<_> = targets.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Core", "Utils", "CoreTests"]);
        assert_eq!(
            targets[0].dependencies,
            [dep("Utils", "target"), dep("Other", "product")]
        );
        assert_eq!(targets[1].path.as_deref(), Some("Lib/Utils"));
        assert_eq!(targets[2].dependencies, [dep("Core", "target")]);
    }

    #[test]
    fn parses_tuist_helper_targets_with_enum_names() {
        let manifest = r#"
let project = Project(
    name: Core.self,
    targets: [
        .spmSwiftFolderTarget(
            name: .YandexGoBaseRouting,
            dependencies: [
                .external(name: "YandexGoFoundation"),
                .target(.YandexGoMapViewController),
                .project(Application.self, target: .FLEXWrapper, status: .optional),
                .system(.MapKit),
            ]
        ),
        .spmUnitTestsFolderTarget(name: .YandexGoBaseRoutingTests),
        .target(name: "App", destinations: .iOS, sources: ["App/Sources/**/*.swift"]),
    ]
)
"#;
        let targets = parse_manifest(manifest);
        assert_eq!(targets.len(), 3);
        assert_eq!(targets[0].name, "YandexGoBaseRouting");
        assert_eq!(
            targets[0].dependencies,
            [
                dep("YandexGoFoundation", "external"),
                dep("YandexGoMapViewController", "target"),
                dep("FLEXWrapper", "project"),
            ]
        );
        assert_eq!(targets[1].name, "YandexGoBaseRoutingTests");
        assert_eq!(targets[2].sources_dir.as_deref(), Some("App/Sources"));
    }

    #[test]
    fn target_dir_prefers_existing_conventional_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("Tests/FooTests")).unwrap();
        let target = |name: &str| ManifestTarget {
            name: name.to_string(),
            path: None,
            sources_dir: None,
            dependencies: vec![],
        };
        assert_eq!(
            target_dir(dir.path(), &target("FooTests")),
            dir.path().join("Tests/FooTests")
        );
        assert_eq!(
            target_dir(dir.path(), &target("Foo")),
            dir.path().join("Sources/Foo")
        );
    }

    #[test]
    fn parses_targets_declared_in_a_variable() {
        let manifest = r#"
let targets: [PackageDescription.Target] = [
    .target(name: "Maps", dependencies: [.product(name: "Geo", package: "geo")]),
]
let documentableTargets: [String] = ["Maps"]
let package = Package(name: "Maps", targets: targets)
"#;
        let targets = parse_manifest(manifest);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "Maps");
        assert_eq!(targets[0].dependencies, [dep("Geo", "product")]);
    }

    #[test]
    fn ignores_non_call_elements_and_unbalanced_input() {
        assert!(parse_manifest("let t = Target(targets: [\"A\", \"B\"])").is_empty());
        assert!(parse_manifest("targets: [ .target(name: \"A\"").is_empty());
    }
}
