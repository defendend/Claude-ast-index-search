//! Tree-sitter based parsers for accurate AST parsing
//!
//! Each language module implements `TreeSitterParser` which provides
//! `parse_symbols()` to extract symbols from source code using tree-sitter queries.

pub mod bash;
pub mod bsl;
pub mod common_lisp;
pub mod cpp;
pub mod csharp;
pub mod css;
pub mod dart;
pub mod elixir;
pub mod gdscript;
pub mod go;
pub mod groovy;
pub mod java;
pub mod kotlin;
pub mod less;
pub mod lua;
pub mod matlab;
pub mod objc;
pub mod php;
pub mod proto;
pub mod python;
pub mod r_lang;
pub mod ruby;
pub mod rust_lang;
pub mod scala;
pub mod scss;
pub mod sql;
pub mod swift;
pub mod typescript;
pub mod zig;

use anyhow::Result;
use tree_sitter::{Language, Parser, Tree};

use super::{extract_references, extract_references_for_lang, FileType, ParsedRef, ParsedSymbol};

/// Trait for tree-sitter based language parsers
pub trait LanguageParser: Send + Sync {
    /// Parse symbols from source code
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>>;

    /// Extract references from source code.
    /// Default implementation uses the existing regex-based generic logic.
    fn extract_refs(&self, content: &str, defined: &[ParsedSymbol]) -> Result<Vec<ParsedRef>> {
        extract_references(content, defined)
    }

    /// Extract references with language-specific keyword filtering.
    /// Parsers that override extract_refs get their custom logic; others get language-aware filtering.
    fn extract_refs_for_lang(
        &self,
        content: &str,
        defined: &[ParsedSymbol],
        file_type: FileType,
    ) -> Result<Vec<ParsedRef>> {
        extract_references_for_lang(content, defined, Some(file_type))
    }
}

/// Get a tree-sitter parser for the given file type, if available
pub fn get_treesitter_parser(file_type: FileType) -> Option<&'static dyn LanguageParser> {
    match file_type {
        FileType::Bash => Some(&bash::BASH_PARSER),
        FileType::Bsl => Some(&bsl::BSL_PARSER),
        FileType::CommonLisp => Some(&common_lisp::COMMON_LISP_PARSER),
        FileType::Cpp => Some(&cpp::CPP_PARSER),
        FileType::CSharp => Some(&csharp::CSHARP_PARSER),
        FileType::Css => Some(&css::CSS_PARSER),
        FileType::Dart => Some(&dart::DART_PARSER),
        FileType::Elixir => Some(&elixir::ELIXIR_PARSER),
        FileType::Gdscript => Some(&gdscript::GDSCRIPT_PARSER),
        FileType::Go => Some(&go::GO_PARSER),
        FileType::Groovy => Some(&groovy::GROOVY_PARSER),
        FileType::Java => Some(&java::JAVA_PARSER),
        FileType::Kotlin => Some(&kotlin::KOTLIN_PARSER),
        FileType::Less => Some(&less::LESS_PARSER),
        FileType::Lua => Some(&lua::LUA_PARSER),
        FileType::Matlab => Some(&matlab::MATLAB_PARSER),
        FileType::ObjC => Some(&objc::OBJC_PARSER),
        FileType::Php => Some(&php::PHP_PARSER),
        FileType::Proto => Some(&proto::PROTO_PARSER),
        FileType::Python => Some(&python::PYTHON_PARSER),
        FileType::R => Some(&r_lang::R_PARSER),
        FileType::Ruby => Some(&ruby::RUBY_PARSER),
        FileType::Rust => Some(&rust_lang::RUST_PARSER),
        FileType::Scala => Some(&scala::SCALA_PARSER),
        FileType::Scss => Some(&scss::SCSS_PARSER),
        FileType::Sql => Some(&sql::SQL_PARSER),
        FileType::Swift => Some(&swift::SWIFT_PARSER),
        FileType::TypeScript => Some(&typescript::TYPESCRIPT_PARSER),
        FileType::Zig => Some(&zig::ZIG_PARSER),
        _ => None,
    }
}

/// Helper: parse source code with a tree-sitter language
fn parse_tree(content: &str, language: &Language) -> Result<Tree> {
    PARSER.with(|p| {
        let mut parser = p.borrow_mut();
        parser
            .set_language(language)
            .map_err(|e| anyhow::anyhow!("Failed to set language: {}", e))?;
        parser
            .parse(content, None)
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

/// Helper to get the last line (1-based, inclusive) covered by a node
fn node_end_line(node: &tree_sitter::Node) -> usize {
    node.end_position().row + 1
}

/// Helper to get the full line text for a node (for signature)
fn line_text(content: &str, line: usize) -> &str {
    content.lines().nth(line - 1).unwrap_or("")
}

/// Controls iterative pre-order tree walking.
pub(crate) enum WalkControl {
    Continue,
    SkipChildren,
    Stop,
}

/// Walk a tree in pre-order without recursion.
///
/// Tree-sitter trees from generated or heavily nested sources can be deep enough
/// to overflow the process stack if traversed recursively. This helper keeps the
/// traversal iterative so language parsers can prune or stop safely.
pub(crate) fn walk_tree_preorder<'a, F>(root: &tree_sitter::Node<'a>, mut visit: F)
where
    F: FnMut(tree_sitter::Node<'a>) -> WalkControl,
{
    let mut cursor = root.walk();

    loop {
        let descend = match visit(cursor.node()) {
            WalkControl::Continue => true,
            WalkControl::SkipChildren => false,
            WalkControl::Stop => return,
        };

        if descend && cursor.goto_first_child() {
            continue;
        }

        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}
