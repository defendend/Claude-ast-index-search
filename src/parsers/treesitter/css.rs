//! Tree-sitter based CSS parser.
//!
//! Indexes class selectors, id selectors, `@keyframes`, custom properties
//! (`--var`), and `@import` paths. Used for `.css`, `.pcss`, `.postcss`.

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{line_text, node_line, node_text, parse_tree, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::{ParsedRef, ParsedSymbol};

static CSS_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_css::LANGUAGE.into());

static CSS_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&CSS_LANGUAGE, include_str!("queries/css.scm"))
        .expect("Failed to compile CSS tree-sitter query")
});

pub static CSS_PARSER: CssParser = CssParser;

pub struct CssParser;

impl LanguageParser for CssParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &CSS_LANGUAGE)?;
        parse_with_query(content, &tree, &CSS_QUERY)
    }

    fn extract_refs(&self, _content: &str, _defined: &[ParsedSymbol]) -> Result<Vec<ParsedRef>> {
        // Default regex-based ref extraction is tuned for CamelCase types and
        // `name(` calls — it produces only noise for kebab-case CSS selectors.
        // Skip refs entirely for now; revisit when adding cross-file usages.
        Ok(Vec::new())
    }
}

/// Shared query-execution body — also used by SCSS / Less for the inherited
/// CSS captures (class_selector, id_selector, keyframes_statement,
/// import_statement, custom property declarations).
pub(super) fn parse_with_query(
    content: &str,
    tree: &tree_sitter::Tree,
    query: &Query,
) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();
    let mut cursor = QueryCursor::new();

    let cap_idx = |name: &str| -> Option<u32> {
        query
            .capture_names()
            .iter()
            .position(|n| *n == name)
            .map(|i| i as u32)
    };

    let idx_class = cap_idx("class_name");
    let idx_id = cap_idx("id_name");
    let idx_keyframes = cap_idx("keyframes_name");
    let idx_import = cap_idx("import_path");
    let idx_use = cap_idx("use_path");
    let idx_forward = cap_idx("forward_path");
    let idx_custom_property = cap_idx("custom_property");
    let idx_scss_variable = cap_idx("scss_variable");
    let idx_less_variable = cap_idx("less_variable");
    let idx_mixin = cap_idx("mixin_name");
    let idx_function = cap_idx("function_name");
    let idx_placeholder = cap_idx("placeholder_name");
    let idx_less_mixin_def = cap_idx("less_mixin_def");

    let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());
    while let Some(m) = matches.next() {
        if let Some(cap) = find_capture(m, idx_class) {
            push(&mut symbols, content, &cap.node, SymbolKind::Class, None);
            continue;
        }
        if let Some(cap) = find_capture(m, idx_id) {
            push(&mut symbols, content, &cap.node, SymbolKind::Object, None);
            continue;
        }
        if let Some(cap) = find_capture(m, idx_keyframes) {
            push(&mut symbols, content, &cap.node, SymbolKind::Function, None);
            continue;
        }
        if let Some(cap) = find_capture(m, idx_mixin) {
            push(&mut symbols, content, &cap.node, SymbolKind::Function, None);
            continue;
        }
        if let Some(cap) = find_capture(m, idx_function) {
            push(&mut symbols, content, &cap.node, SymbolKind::Function, None);
            continue;
        }
        if let Some(cap) = find_capture(m, idx_less_mixin_def) {
            push(&mut symbols, content, &cap.node, SymbolKind::Function, None);
            continue;
        }
        if let Some(cap) = find_capture(m, idx_placeholder) {
            // %placeholder — keep `%` prefix to distinguish from `.class`.
            let raw = node_text(content, &cap.node);
            let name = format!("%{}", raw);
            push_named(&mut symbols, content, &cap.node, SymbolKind::Class, name);
            continue;
        }
        if let Some(cap) = find_capture(m, idx_custom_property) {
            // CSS custom property — only those starting with `--`.
            let text = node_text(content, &cap.node);
            if text.starts_with("--") {
                push(&mut symbols, content, &cap.node, SymbolKind::Constant, None);
            }
            continue;
        }
        if let Some(cap) = find_capture(m, idx_scss_variable) {
            // SCSS `$var: …;` or CSS custom property `--foo: …;` — both reach
            // the declaration LHS via the SCSS `property_name` alias.
            let text = node_text(content, &cap.node);
            if text.starts_with('$') || text.starts_with("--") {
                push(&mut symbols, content, &cap.node, SymbolKind::Constant, None);
            }
            continue;
        }
        if let Some(cap) = find_capture(m, idx_less_variable) {
            // Less `@brand: …;` — declaration LHS aliased to property_name.
            let text = node_text(content, &cap.node);
            if text.starts_with('@') {
                push(&mut symbols, content, &cap.node, SymbolKind::Constant, None);
            }
            continue;
        }
        if let Some(cap) = find_capture(m, idx_import) {
            push_string_value(&mut symbols, content, &cap.node, "import");
            continue;
        }
        if let Some(cap) = find_capture(m, idx_use) {
            push_string_value(&mut symbols, content, &cap.node, "use");
            continue;
        }
        if let Some(cap) = find_capture(m, idx_forward) {
            push_string_value(&mut symbols, content, &cap.node, "forward");
            continue;
        }
    }

    Ok(symbols)
}

fn push(
    symbols: &mut Vec<ParsedSymbol>,
    content: &str,
    node: &tree_sitter::Node,
    kind: SymbolKind,
    name_override: Option<String>,
) {
    let name = name_override.unwrap_or_else(|| node_text(content, node).to_string());
    push_named(symbols, content, node, kind, name);
}

fn push_named(
    symbols: &mut Vec<ParsedSymbol>,
    content: &str,
    node: &tree_sitter::Node,
    kind: SymbolKind,
    name: String,
) {
    if name.is_empty() {
        return;
    }
    let line = node_line(node);
    symbols.push(ParsedSymbol {
        name,
        kind,
        line,
        signature: line_text(content, line).trim().to_string(),
        parents: vec![],
        end_line: None,
    });
}

/// Strip surrounding quotes from a `string_value` node. Some grammar versions
/// expose a child `string_content`; others fold the whole quoted string into
/// the leaf — we handle both by trimming `"` / `'` from the ends.
fn push_string_value(
    symbols: &mut Vec<ParsedSymbol>,
    content: &str,
    node: &tree_sitter::Node,
    inherit_kind: &str,
) {
    let raw = node_text(content, node);
    let unquoted = raw.trim_matches(|c| c == '"' || c == '\'');
    if unquoted.is_empty() {
        return;
    }
    let line = node_line(node);
    symbols.push(ParsedSymbol {
        name: unquoted.to_string(),
        kind: SymbolKind::Import,
        line,
        signature: line_text(content, line).trim().to_string(),
        parents: vec![(unquoted.to_string(), inherit_kind.to_string())],
        end_line: None,
    });
}

pub(super) fn find_capture<'a>(
    m: &'a tree_sitter::QueryMatch<'a, 'a>,
    idx: Option<u32>,
) -> Option<&'a tree_sitter::QueryCapture<'a>> {
    let idx = idx?;
    m.captures.iter().find(|c| c.index == idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_and_id_selectors() {
        let src = ".btn { }\n#main { }\n";
        let symbols = CSS_PARSER.parse_symbols(src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "btn" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "main" && s.kind == SymbolKind::Object));
    }

    #[test]
    fn keyframes() {
        let src = "@keyframes spin { from {} to {} }\n";
        let symbols = CSS_PARSER.parse_symbols(src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "spin" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn custom_property() {
        let src = ":root { --brand-blue: #00f; --pad: 4px; }\n.x { color: red; }\n";
        let symbols = CSS_PARSER.parse_symbols(src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "--brand-blue" && s.kind == SymbolKind::Constant));
        assert!(symbols
            .iter()
            .any(|s| s.name == "--pad" && s.kind == SymbolKind::Constant));
        // Regular `color` declaration must NOT be indexed.
        assert!(!symbols.iter().any(|s| s.name == "color"));
    }

    #[test]
    fn import() {
        let src = "@import \"reset.css\";\n";
        let symbols = CSS_PARSER.parse_symbols(src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "reset.css" && s.kind == SymbolKind::Import));
    }
}
