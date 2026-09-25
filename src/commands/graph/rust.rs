//! Rust modules for the symbol graph: which crate and module a file is, and
//! the names its `use` declarations bring into scope.
//!
//! A Rust path names a module by the file it lives in (`src/db.rs` is `db`,
//! `src/commands/mod.rs` is `commands`) and reaches other modules through
//! `crate::`, `super::`, `self::` or a name a `use` bound, so none of it can
//! be read off the symbol names alone.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use tree_sitter::{Language, Node, Parser};

use crate::parsers::treesitter::{walk_tree_preorder, WalkControl};

static RUST: LazyLock<Language> = LazyLock::new(|| tree_sitter_rust::LANGUAGE.into());

/// Where a Rust source file sits: the crate it belongs to, keyed by the
/// directory the crate's paths start from, and its module path in that crate
/// (`commands::graph` for `src/commands/graph/mod.rs`, empty for the root).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ModuleLocation {
    pub crate_key: String,
    /// The package directory whose `Cargo.toml` names the crate, for a crate
    /// under `src/`.
    pub package: Option<String>,
    pub module: String,
}

fn module_of(components: &[&str]) -> String {
    let mut segments: Vec<&str> = components.to_vec();
    if let Some(last) = segments.pop() {
        let stem = last.strip_suffix(".rs").unwrap_or(last);
        if stem != "mod" {
            segments.push(stem);
        }
    }
    segments.join("::")
}

/// The crate and module of the Rust file at `path` (relative, `/`-separated),
/// following Cargo's layout: `src/lib.rs` and `src/main.rs` are a crate root
/// and every other file under `src/` a module of it, `src/bin/x.rs` is a crate
/// of its own, as is every file directly under `tests/`, `benches/` or
/// `examples/`; a file anywhere else is taken as a module of its directory.
pub(super) fn module_location(path: &str) -> ModuleLocation {
    let components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    let anchor = components
        .iter()
        .rposition(|c| matches!(*c, "src" | "tests" | "benches" | "examples"))
        .filter(|&index| index + 1 < components.len());
    let Some(anchor) = anchor else {
        let (dir, file) = components.split_at(components.len().saturating_sub(1));
        let file = file.first().copied().unwrap_or("");
        let root_file = matches!(file, "lib.rs" | "main.rs" | "mod.rs" | "build.rs");
        return ModuleLocation {
            crate_key: if file == "build.rs" {
                path.to_string()
            } else {
                dir.join("/")
            },
            package: None,
            module: if root_file {
                String::new()
            } else {
                module_of(&[file])
            },
        };
    };
    let rest = &components[anchor + 1..];
    let base = components[..=anchor].join("/");
    if components[anchor] == "src" {
        if rest.len() >= 2 && rest[0] == "bin" {
            let own = rest[1].strip_suffix(".rs").unwrap_or(rest[1]);
            let inner = &rest[2..];
            let module = match inner {
                [] | ["main.rs"] => String::new(),
                _ => module_of(inner),
            };
            return ModuleLocation {
                crate_key: format!("{base}/bin/{own}"),
                package: None,
                module,
            };
        }
        let module = match rest {
            ["lib.rs"] | ["main.rs"] => String::new(),
            _ => module_of(rest),
        };
        return ModuleLocation {
            crate_key: base,
            package: Some(components[..anchor].join("/")),
            module,
        };
    }
    if rest.len() == 1 {
        return ModuleLocation {
            crate_key: path.to_string(),
            package: None,
            module: String::new(),
        };
    }
    ModuleLocation {
        crate_key: base,
        package: None,
        module: module_of(rest),
    }
}

/// The crate name other crates use for the library of `package`: the
/// `[lib] name`, else the `[package] name` with `-` read as `_`.
pub(super) fn crate_name(root: &Path, package: &str) -> Option<String> {
    let manifest = std::fs::read_to_string(root.join(package).join("Cargo.toml")).ok()?;
    let mut section = "";
    let mut package_name = None;
    let mut lib_name = None;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line;
            continue;
        }
        let Some(value) = line
            .strip_prefix("name")
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix('='))
        else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_string();
        match section {
            "[package]" => package_name = package_name.or(Some(value)),
            "[lib]" => lib_name = lib_name.or(Some(value)),
            _ => {}
        }
    }
    lib_name.or(package_name).map(|name| name.replace('-', "_"))
}

/// One name a `use` declaration binds: `local` stands for `path` as written
/// (`crate::db::open_db`, `super::helpers`), from `line` on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UseBinding {
    pub line: i64,
    pub local: String,
    pub path: Vec<String>,
}

/// What the `use` declarations of one file bring into scope: named bindings
/// and glob imports (`use super::*;`, whose path is the module globbed).
#[derive(Debug, Default)]
pub(super) struct FileUses {
    pub bindings: Vec<UseBinding>,
    pub globs: Vec<(i64, Vec<String>)>,
}

fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

/// The segments of a path node: `crate::db::open_db` -> `crate`, `db`,
/// `open_db`.
fn path_segments(node: Node<'_>, source: &str, out: &mut Vec<String>) {
    match node.kind() {
        "scoped_identifier" => {
            if let Some(path) = node.child_by_field_name("path") {
                path_segments(path, source, out);
            }
            if let Some(name) = node.child_by_field_name("name") {
                out.push(text(name, source).to_string());
            }
        }
        _ => out.push(text(node, source).to_string()),
    }
}

fn collect_use_clause(
    node: Node<'_>,
    source: &str,
    prefix: &[String],
    line: i64,
    uses: &mut FileUses,
) {
    match node.kind() {
        "use_list" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_use_clause(child, source, prefix, line, uses);
            }
        }
        "scoped_use_list" => {
            let mut inner = prefix.to_vec();
            if let Some(path) = node.child_by_field_name("path") {
                path_segments(path, source, &mut inner);
            }
            if let Some(list) = node.child_by_field_name("list") {
                collect_use_clause(list, source, &inner, line, uses);
            }
        }
        "use_as_clause" => {
            let (Some(path), Some(alias)) = (
                node.child_by_field_name("path"),
                node.child_by_field_name("alias"),
            ) else {
                return;
            };
            let mut full = prefix.to_vec();
            path_segments(path, source, &mut full);
            if full.last().is_some_and(|last| last == "self") && full.len() > 1 {
                full.pop();
            }
            let local = text(alias, source);
            if local != "_" {
                uses.bindings.push(UseBinding {
                    line,
                    local: local.to_string(),
                    path: full,
                });
            }
        }
        "use_wildcard" => {
            let mut full = prefix.to_vec();
            let mut cursor = node.walk();
            if let Some(path) = node.named_children(&mut cursor).next() {
                path_segments(path, source, &mut full);
            }
            uses.globs.push((line, full));
        }
        "identifier" | "scoped_identifier" | "self" | "crate" | "super" => {
            let mut full = prefix.to_vec();
            path_segments(node, source, &mut full);
            // `use a::b::{self}` binds `b`.
            if full.last().is_some_and(|last| last == "self") && full.len() > 1 {
                full.pop();
            }
            let Some(local) = full.last().cloned() else {
                return;
            };
            if matches!(local.as_str(), "crate" | "super" | "self") {
                return;
            }
            uses.bindings.push(UseBinding {
                line,
                local,
                path: full,
            });
        }
        _ => {}
    }
}

/// Every `use` declaration of a Rust file, nested ones included, as the
/// names it binds and the modules it globs.
pub(super) fn parse_uses(source: &str) -> Option<FileUses> {
    let mut uses = FileUses::default();
    if !source.contains("use ") {
        return Some(uses);
    }
    let mut parser = Parser::new();
    parser.set_language(&RUST).ok()?;
    let tree = parser.parse(source, None)?;
    walk_tree_preorder(&tree.root_node(), |node| {
        if node.kind() != "use_declaration" {
            return WalkControl::Continue;
        }
        if let Some(argument) = node.child_by_field_name("argument") {
            let line = node.start_position().row as i64 + 1;
            collect_use_clause(argument, source, &[], line, &mut uses);
        }
        WalkControl::SkipChildren
    });
    Some(uses)
}

/// The `use` scope of one module: named bindings and globbed modules, each
/// path still as written, relative to that module.
#[derive(Debug, Default)]
pub(super) struct ModuleScope {
    pub bindings: HashMap<String, Vec<String>>,
    pub globs: Vec<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(path: &str) -> (String, Option<String>, String) {
        let found = module_location(path);
        (found.crate_key, found.package, found.module)
    }

    #[test]
    fn files_map_to_their_crate_and_module() {
        let src = |module: &str| ("src".to_string(), Some(String::new()), module.to_string());
        assert_eq!(location("src/lib.rs"), src(""));
        assert_eq!(location("src/main.rs"), src(""));
        assert_eq!(location("src/db.rs"), src("db"));
        assert_eq!(location("src/commands/mod.rs"), src("commands"));
        assert_eq!(
            location("src/commands/graph/resolve.rs"),
            src("commands::graph::resolve")
        );
        assert_eq!(
            location("crates/mcp/src/format.rs"),
            (
                "crates/mcp/src".to_string(),
                Some("crates/mcp".to_string()),
                "format".to_string()
            )
        );
        assert_eq!(
            location("src/bin/tool.rs"),
            ("src/bin/tool".to_string(), None, String::new())
        );
        assert_eq!(
            location("tests/graph_tests.rs"),
            ("tests/graph_tests.rs".to_string(), None, String::new())
        );
        assert_eq!(
            location("tests/common/mod.rs"),
            ("tests".to_string(), None, "common".to_string())
        );
        assert_eq!(
            location("bindings/rust/lib.rs"),
            ("bindings/rust".to_string(), None, String::new())
        );
        assert_eq!(
            location("build.rs"),
            ("build.rs".to_string(), None, String::new())
        );
    }

    #[test]
    fn use_declarations_bind_names_and_globs() {
        let source = concat!(
            "use std::collections::{HashMap, HashSet as Set};\n",
            "use super::{graph::short_name, relative_path, self as parent};\n",
            "use crate::db::{self, SearchResult};\n",
            "pub use test_paths::is_test_path;\n",
            "use super::*;\n",
            "fn f() { use crate::db::open_db; }\n",
            "use regex::Regex as _;\n",
        );
        let uses = parse_uses(source).unwrap();
        let bound: Vec<(i64, &str, String)> = uses
            .bindings
            .iter()
            .map(|b| (b.line, b.local.as_str(), b.path.join("::")))
            .collect();
        assert_eq!(
            bound,
            vec![
                (1, "HashMap", "std::collections::HashMap".to_string()),
                (1, "Set", "std::collections::HashSet".to_string()),
                (2, "short_name", "super::graph::short_name".to_string()),
                (2, "relative_path", "super::relative_path".to_string()),
                (2, "parent", "super".to_string()),
                (3, "db", "crate::db".to_string()),
                (3, "SearchResult", "crate::db::SearchResult".to_string()),
                (4, "is_test_path", "test_paths::is_test_path".to_string()),
                (6, "open_db", "crate::db::open_db".to_string()),
            ]
        );
        assert_eq!(uses.globs, vec![(5, vec!["super".to_string()])]);
    }
}
