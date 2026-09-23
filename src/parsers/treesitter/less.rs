//! Tree-sitter based Less parser.
//!
//! Less extends CSS. Indexes class/id selectors, `@keyframes`, `@import`,
//! `.mixin()` definitions and `@variable` declarations.

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query};

use super::css::parse_with_query;
use super::{parse_tree, LanguageParser};
use crate::parsers::{FileType, ParsedRef, ParsedSymbol};

static LESS_LANGUAGE: LazyLock<Language> = LazyLock::new(tree_sitter_less::language);

static LESS_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&LESS_LANGUAGE, include_str!("queries/less.scm"))
        .expect("Failed to compile Less tree-sitter query")
});

pub static LESS_PARSER: LessParser = LessParser;

pub struct LessParser;

impl LanguageParser for LessParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &LESS_LANGUAGE)?;
        parse_with_query(content, &tree, &LESS_QUERY)
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
    fn variables_and_mixins() {
        let src = r#"@brand: #ff0;
.rounded(@radius: 4px) { border-radius: @radius; }
.btn { .rounded(8px); color: @brand; }
@import "reset.less";
#main { color: red; }
"#;
        let symbols = LESS_PARSER.parse_symbols(src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "@brand" && s.kind == SymbolKind::Constant));
        assert!(symbols
            .iter()
            .any(|s| s.name == "rounded" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "btn" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "main" && s.kind == SymbolKind::Object));
        assert!(symbols
            .iter()
            .any(|s| s.name == "reset.less" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn plain_declarations_are_ignored() {
        let src = ".x { color: red; padding: 10px; }\n";
        let symbols = LESS_PARSER.parse_symbols(src).unwrap();
        assert!(symbols
            .iter()
            .all(|s| s.name != "color" && s.name != "padding"));
    }
}
