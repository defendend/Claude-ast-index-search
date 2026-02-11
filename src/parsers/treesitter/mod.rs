//! Tree-sitter based parsers for accurate AST parsing
//!
//! Each language module implements `TreeSitterParser` which provides
//! `parse_symbols()` to extract symbols from source code using tree-sitter queries.

pub mod cpp;
pub mod csharp;
pub mod dart;
pub mod go;
pub mod kotlin;
pub mod objc;
pub mod proto;
pub mod python;
pub mod ruby;
pub mod rust_lang;
pub mod scala;
pub mod swift;
pub mod typescript;

use anyhow::Result;
use tree_sitter::{Language, Parser, Tree};

use super::{ParsedSymbol, ParsedRef, extract_references, FileType};

/// Trait for tree-sitter based language parsers
pub trait LanguageParser: Send + Sync {
    /// Parse symbols from source code
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>>;

    /// Extract references from source code.
    /// Default implementation uses the existing regex-based generic logic.
    fn extract_refs(&self, content: &str, defined: &[ParsedSymbol]) -> Result<Vec<ParsedRef>> {
        extract_references(content, defined)
    }
}

/// Get a tree-sitter parser for the given file type, if available
pub fn get_treesitter_parser(file_type: FileType) -> Option<&'static dyn LanguageParser> {
    match file_type {
        FileType::Cpp => Some(&cpp::CPP_PARSER),
        FileType::CSharp => Some(&csharp::CSHARP_PARSER),
        FileType::Dart => Some(&dart::DART_PARSER),
        FileType::Go => Some(&go::GO_PARSER),
        FileType::Kotlin => Some(&kotlin::KOTLIN_PARSER),
        FileType::ObjC => Some(&objc::OBJC_PARSER),
        FileType::Proto => Some(&proto::PROTO_PARSER),
        FileType::Python => Some(&python::PYTHON_PARSER),
        FileType::Ruby => Some(&ruby::RUBY_PARSER),
        FileType::Rust => Some(&rust_lang::RUST_PARSER),
        FileType::Scala => Some(&scala::SCALA_PARSER),
        FileType::Swift => Some(&swift::SWIFT_PARSER),
        FileType::TypeScript => Some(&typescript::TYPESCRIPT_PARSER),
        _ => None,
    }
}

/// Helper: parse source code with a tree-sitter language
fn parse_tree(content: &str, language: &Language) -> Result<Tree> {
    PARSER.with(|p| {
        let mut parser = p.borrow_mut();
        parser.set_language(language).map_err(|e| anyhow::anyhow!("Failed to set language: {}", e))?;
        parser.parse(content, None)
            .ok_or_else(|| anyhow::anyhow!("tree-sitter parse returned None"))
    })
}

// Thread-local parser for reuse (tree-sitter Parser is not Send)
thread_local! {
    static PARSER: std::cell::RefCell<Parser> = std::cell::RefCell::new(Parser::new());
}

/// Helper to get text from a node
fn node_text<'a>(content: &'a str, node: &tree_sitter::Node) -> &'a str {
    &content[node.byte_range()]
}

/// Helper to get line number (1-based) from a node
fn node_line(node: &tree_sitter::Node) -> usize {
    node.start_position().row + 1
}

/// Helper to get the full line text for a node (for signature)
fn line_text(content: &str, line: usize) -> &str {
    content.lines().nth(line - 1).unwrap_or("")
}
