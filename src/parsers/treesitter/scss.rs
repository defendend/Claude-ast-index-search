//! Tree-sitter based SCSS parser.
//!
//! SCSS extends CSS, so we reuse the shared query runner from `css.rs`. The
//! `scss.scm` query adds `@mixin`, `@function`, `%placeholder`, `$variable`,
//! `@use`, `@forward` on top of CSS captures.

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query};

use super::css::parse_with_query;
use super::{parse_tree, LanguageParser};
use crate::parsers::{FileType, ParsedRef, ParsedSymbol};

static SCSS_LANGUAGE: LazyLock<Language> = LazyLock::new(tree_sitter_scss::language);

static SCSS_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&SCSS_LANGUAGE, include_str!("queries/scss.scm"))
        .expect("Failed to compile SCSS tree-sitter query")
});

pub static SCSS_PARSER: ScssParser = ScssParser;

pub struct ScssParser;

impl LanguageParser for ScssParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &SCSS_LANGUAGE)?;
        parse_with_query(content, &tree, &SCSS_QUERY)
    }

    fn extract_refs(&self, _content: &str, _defined: &[ParsedSymbol]) -> Result<Vec<ParsedRef>> {
        Ok(Vec::new())
    }

    fn extract_refs_for_lang(
        &self,
        content: &str,
        defined: &[ParsedSymbol],
        _file_type: FileType,
    ) -> Result<Vec<ParsedRef>> {
        self.extract_refs(content, defined)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::SymbolKind;

    #[test]
    fn variables_mixins_functions() {
        let src = r#"$primary: #ff0;
@mixin button($size: 10px) { font-size: $size; }
@function double($x) { @return $x * 2; }
%card { padding: 10px; }
.btn { @include button(20px); color: $primary; }
@use "variables";
@forward "src/list";
"#;
        let symbols = SCSS_PARSER.parse_symbols(src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "$primary" && s.kind == SymbolKind::Constant));
        assert!(symbols
            .iter()
            .any(|s| s.name == "button" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "double" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "%card" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "btn" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "variables" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "src/list" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn custom_property_inside_root() {
        let src = ":root { --brand: #ff0; }\n";
        let symbols = SCSS_PARSER.parse_symbols(src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "--brand" && s.kind == SymbolKind::Constant));
    }

    #[test]
    fn plain_declarations_are_ignored() {
        let src = ".x { color: red; padding: 10px; }\n";
        let symbols = SCSS_PARSER.parse_symbols(src).unwrap();
        assert!(symbols
            .iter()
            .all(|s| s.name != "color" && s.name != "padding"));
    }
}
