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
use std::sync::LazyLock;
use tree_sitter::{Language, Parser, Tree};

use super::{extract_references, extract_references_for_lang, FileType, ParsedRef, ParsedSymbol};

/// Trait for tree-sitter based language parsers
pub trait LanguageParser: Send + Sync {
    /// Parse symbols from source code
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>>;

    /// Extract references from source code without a file type.
    /// Default implementation uses the existing regex-based generic logic.
    fn extract_refs(&self, content: &str, defined: &[ParsedSymbol]) -> Result<Vec<ParsedRef>> {
        extract_references(content, defined)
    }

    /// Extract references with language-specific keyword filtering. This is
    /// what indexing calls, and the default never consults `extract_refs`: a
    /// parser with its own extraction must override this method as well.
    fn extract_refs_for_lang(
        &self,
        content: &str,
        defined: &[ParsedSymbol],
        file_type: FileType,
    ) -> Result<Vec<ParsedRef>> {
        match self.non_code() {
            Some(kinds) => {
                let tree = parse_tree(content, kinds.language)?;
                masked_refs(content, &tree, kinds, defined, file_type)
            }
            None => extract_references_for_lang(content, defined, Some(file_type)),
        }
    }

    /// The comment and string kinds of this parser's grammar. With them, the
    /// default reference extraction reads the file with comments and strings
    /// blanked out (see [`mask_non_code`]).
    fn non_code(&self) -> Option<&'static NonCode> {
        None
    }

    /// Symbols and references of one file, as indexing reads them. A parser
    /// whose reference extraction walks the syntax tree overrides this to
    /// parse the file once instead of once per step.
    fn parse_symbols_and_refs(
        &self,
        content: &str,
        file_type: FileType,
    ) -> Result<(Vec<ParsedSymbol>, Vec<ParsedRef>)> {
        forget_last_tree();
        let symbols = self.parse_symbols(content)?;
        let refs = match self.non_code() {
            Some(kinds) => {
                let tree = match reuse_tree(content, kinds.language) {
                    Some(tree) => tree,
                    None => parse_tree(content, kinds.language)?,
                };
                masked_refs(content, &tree, kinds, &symbols, file_type)?
            }
            None => self.extract_refs_for_lang(content, &symbols, file_type)?,
        };
        Ok((symbols, refs))
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
    forget_line_starts();
    PARSER.with(|p| {
        let mut parser = p.borrow_mut();
        parser
            .set_language(language)
            .map_err(|e| anyhow::anyhow!("Failed to set language: {}", e))?;
        let tree = parser
            .parse(content, None)
            .ok_or_else(|| anyhow::anyhow!("tree-sitter parse returned None"))?;
        LAST_TREE.with(|last| {
            *last.borrow_mut() = Some((text_key(content), tree.clone()));
        });
        Ok(tree)
    })
}

/// The tree [`parse_tree`] last built, when it was built from `content` itself
/// with `language`: symbol extraction parses a file, and reference
/// extraction reads the same tree instead of parsing the file again.
fn reuse_tree(content: &str, language: &Language) -> Option<Tree> {
    LAST_TREE.with(|last| {
        let mut last = last.borrow_mut();
        let fits = last
            .as_ref()
            .is_some_and(|(key, tree)| *key == text_key(content) && *tree.language() == *language);
        if fits {
            last.take().map(|(_, tree)| tree)
        } else {
            None
        }
    })
}

/// Drop the tree [`reuse_tree`] would hand out: a freed text's address can
/// come back with a new text of the same length.
fn forget_last_tree() {
    LAST_TREE.with(|last| last.borrow_mut().take());
}

/// A text by its address and length, as long as it is alive.
fn text_key(content: &str) -> (usize, usize) {
    (content.as_ptr() as usize, content.len())
}

// Thread-local parser for reuse (tree-sitter Parser is not Send)
thread_local! {
    static PARSER: std::cell::RefCell<Parser> = std::cell::RefCell::new(Parser::new());
    static LAST_TREE: std::cell::RefCell<Option<((usize, usize), Tree)>> =
        const { std::cell::RefCell::new(None) };
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

/// Last line (1-based, inclusive) that holds text of `node`.
///
/// Some grammars end a node after the whitespace that terminates it: a C
/// preprocessor directive owns its newline, a Groovy statement the blank lines
/// after it. The end position then points at a line the definition does not
/// reach, which would let it claim the next definition's first line.
fn text_end_line(content: &str, node: &tree_sitter::Node) -> usize {
    let text = &content.as_bytes()[node.byte_range()];
    let text_end = text
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(0, |last| last + 1);
    let trailing_newlines = text[text_end..].iter().filter(|&&b| b == b'\n').count();
    (node.end_position().row + 1)
        .saturating_sub(trailing_newlines)
        .max(node_line(node))
}

/// Where each line of one text starts, keyed by the text's address and
/// length.
struct LineStarts {
    text: (usize, usize),
    starts: Vec<usize>,
}

thread_local! {
    static LINE_STARTS: std::cell::RefCell<LineStarts> = const {
        std::cell::RefCell::new(LineStarts { text: (0, 0), starts: Vec::new() })
    };
}

/// Drop the line table [`line_text`] keeps. A freed text's address can come
/// back with a new text of the same length, so the table must not outlive
/// the parse it was built for; every parse starts in [`parse_tree`].
fn forget_line_starts() {
    LINE_STARTS.with(|cell| cell.borrow_mut().text = (0, 0));
}

/// Helper to get the full line text for a node (for signature)
///
/// The same as `content.lines().nth(line - 1).unwrap_or("")`. That scan from
/// the top of the file on every call made signature extraction quadratic in
/// file length, and it was the largest single cost of a rebuild; the lines
/// are indexed once per text instead.
fn line_text(content: &str, line: usize) -> &str {
    let Some(index) = line.checked_sub(1) else {
        return "";
    };
    let key = (content.as_ptr() as usize, content.len());
    let (start, end) = LINE_STARTS.with(|cell| {
        let mut table = cell.borrow_mut();
        if table.text != key {
            table.text = key;
            table.starts.clear();
            table.starts.push(0);
            table
                .starts
                .extend(content.match_indices('\n').map(|(at, _)| at + 1));
        }
        let start = table.starts.get(index).copied();
        let next = table.starts.get(index + 1).copied();
        (start, next)
    });
    // `lines()` yields no line after a final newline, and strips a `\r` only
    // together with the `\n` that follows it.
    match (start, end) {
        (Some(start), _) if start >= content.len() => "",
        (Some(start), Some(next)) => {
            let line = &content[start..next - 1];
            line.strip_suffix('\r').unwrap_or(line)
        }
        (Some(start), None) => &content[start..],
        (None, _) => "",
    }
}

/// `line_text(content, line).trim()` as an owned signature, cut short where
/// the insert-time [`crate::parsers::truncate_signature`] would cut it anyway.
///
/// A minified stylesheet is one line holding thousands of selectors; copying
/// that whole line into every selector's signature took gigabytes of memory
/// before the insert truncated each copy. The cut keeps the first char
/// boundary at or past `MAX + 4` bytes, so the text is still over the limit
/// and truncating it gives the same bytes as truncating the full line.
fn signature_line(content: &str, line: usize) -> String {
    let text = line_text(content, line).trim();
    let keep = crate::parsers::MAX_SIGNATURE_LEN + 4;
    if text.len() <= keep {
        return text.to_string();
    }
    let mut end = keep;
    while !text.is_char_boundary(end) {
        end += 1;
    }
    text[..end].to_string()
}

/// Syntax node kinds of one grammar whose text is not code.
pub struct NonCode {
    pub language: &'static LazyLock<Language>,
    /// Blanked whole: comments, heredoc openers, JSX text.
    pub prose: &'static [&'static str],
    /// String-like literals, blanked except for the code nested in them.
    pub strings: &'static [&'static str],
    /// That nested code: `#{...}`, `${...}`, an f-string's `{...}`.
    pub code: &'static [&'static str],
    /// The byte ranges of a string that stay as written, because the string
    /// names code (`class_name: 'Invoice'`, `import('./Page')`). The string's
    /// own range keeps it whole; an empty list blanks it.
    pub keep: fn(&str, tree_sitter::Node) -> Vec<std::ops::Range<usize>>,
    /// The name a declaration node declares (a C prototype's function), which
    /// is no reference to itself.
    pub declared: fn(&str, tree_sitter::Node) -> Option<std::ops::Range<usize>>,
}

/// A [`NonCode::declared`] for grammars whose declarations the symbol lines
/// already cover.
pub(crate) fn declares_nothing(_: &str, _: tree_sitter::Node) -> Option<std::ops::Range<usize>> {
    None
}

/// A [`NonCode::keep`] for grammars whose strings never name code.
pub(crate) fn keep_no_string(_: &str, _: tree_sitter::Node) -> Vec<std::ops::Range<usize>> {
    Vec::new()
}

/// `content` with every comment and string literal blanked byte for byte
/// (newlines kept), so the line-based reference extraction reads only code
/// and a name in prose (`# see MergeService`, `"User not found"`, a
/// docstring) is no reference.
pub(crate) fn mask_non_code(content: &str, root: tree_sitter::Node, kinds: &NonCode) -> String {
    let original = content.as_bytes();
    let mut masked = original.to_vec();
    let blank = |bytes: &mut [u8], range: std::ops::Range<usize>| {
        for byte in &mut bytes[range] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    walk_tree_preorder(&root, |node| {
        if let Some(name) = (kinds.declared)(content, node) {
            blank(&mut masked, name.clone());
            // A `;` where the name ended keeps the return type from reading
            // as a call once the name is gone: `size_t        (`.
            if let Some(last) = name.end.checked_sub(1).filter(|&last| last >= name.start) {
                masked[last] = b';';
            }
        }
        let kind = node.kind();
        if kinds.prose.contains(&kind) {
            blank(&mut masked, node.byte_range());
            return WalkControl::SkipChildren;
        }
        if !kinds.strings.contains(&kind) {
            return WalkControl::Continue;
        }
        let mut kept = (kinds.keep)(content, node);
        if kept.first() == Some(&node.byte_range()) {
            return WalkControl::SkipChildren;
        }
        blank(&mut masked, node.byte_range());
        if !kinds.code.is_empty() {
            walk_tree_preorder(&node, |inner| {
                if kinds.code.contains(&inner.kind()) {
                    kept.push(inner.byte_range());
                    return WalkControl::SkipChildren;
                }
                WalkControl::Continue
            });
        }
        for range in kept {
            masked[range.clone()].copy_from_slice(&original[range]);
        }
        // Strings and comments nested in the restored code are blanked when
        // the walk reaches them.
        WalkControl::Continue
    });
    String::from_utf8(masked).unwrap_or_else(|_| content.to_string())
}

/// Generic reference extraction over `masked` (see [`mask_non_code`]), each
/// reference keeping its line of the original text as context.
pub(crate) fn extract_refs_masked(
    content: &str,
    masked: &str,
    defined: &[ParsedSymbol],
    file_type: Option<FileType>,
) -> Result<Vec<ParsedRef>> {
    let mut refs = extract_references_for_lang(masked, defined, file_type)?;
    let lines: Vec<&str> = content.lines().collect();
    for reference in &mut refs {
        if let Some(line) = lines.get(reference.line.saturating_sub(1)) {
            reference.context = super::truncate_context(line.trim());
        }
    }
    Ok(refs)
}

fn masked_refs(
    content: &str,
    tree: &Tree,
    kinds: &NonCode,
    defined: &[ParsedSymbol],
    file_type: FileType,
) -> Result<Vec<ParsedRef>> {
    let masked = mask_non_code(content, tree.root_node(), kinds);
    extract_refs_masked(content, &masked, defined, Some(file_type))
}

/// `Name`, `Scope::Name` or `::Name`: every segment a constant. Excludes
/// Ruby's `Scope::method = value`, which is a setter call, not a constant.
pub(crate) fn is_constant_path(text: &str) -> bool {
    let text = text.strip_prefix("::").unwrap_or(text);
    !text.is_empty()
        && text.split("::").all(|segment| {
            segment.chars().next().is_some_and(char::is_uppercase)
                && segment.chars().all(|c| c.is_alphanumeric() || c == '_')
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_end_line_skips_trailing_whitespace_the_node_owns() {
        let language: Language = tree_sitter_cpp::LANGUAGE.into();
        let content = concat!("#define TWICE(x) \\\n", "    ((x) * 2)\n", "int y;\n");
        let tree = parse_tree(content, &language).unwrap();
        let define = tree.root_node().child(0).unwrap();
        assert_eq!(define.kind(), "preproc_function_def");
        assert_eq!(node_end_line(&define), 3);
        assert_eq!(text_end_line(content, &define), 2);

        let declaration = tree.root_node().child(1).unwrap();
        assert_eq!(text_end_line(content, &declaration), 3);
    }

    #[test]
    fn line_text_matches_lines_nth() {
        let texts = [
            "",
            "one",
            "one\n",
            "one\ntwo",
            "one\r\ntwo\r\n",
            "one\r\ntwo\r",
            "\n\n\nlast",
            "a\rb\nc",
            "\u{442}\u{435}\u{43a}\u{441}\u{442}\n\u{2014}\n",
        ];
        for text in texts {
            for line in 0..6 {
                let expected = if line == 0 {
                    ""
                } else {
                    text.lines().nth(line - 1).unwrap_or("")
                };
                assert_eq!(line_text(text, line), expected, "{text:?} line {line}");
            }
        }
    }

    #[test]
    fn line_text_does_not_reuse_another_texts_lines() {
        let first = "a\nb\nc".to_string();
        assert_eq!(line_text(&first, 2), "b");
        forget_line_starts();
        let second = "xyz\nw".to_string();
        assert_eq!(line_text(&second, 2), "w");
        assert_eq!(line_text(&first, 3), "c");
    }

    #[test]
    fn signature_line_truncates_to_the_same_bytes() {
        let max = crate::parsers::MAX_SIGNATURE_LEN;
        let long_ascii = format!("  {}  ", "x".repeat(max * 3));
        let long_multibyte = format!("\t{}", "\u{44f}".repeat(max));
        let near_limit = format!("{} tail", "y".repeat(max - 2));
        for body in [long_ascii, long_multibyte, near_limit, "short".to_string()] {
            let text = format!("first\n{body}\nlast");
            let full = line_text(&text, 2).trim().to_string();
            let capped = signature_line(&text, 2);
            assert!(capped.len() <= full.len());
            assert_eq!(
                crate::parsers::truncate_signature(&capped),
                crate::parsers::truncate_signature(&full)
            );
        }
    }
}
