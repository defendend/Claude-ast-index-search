//! Language-specific parsers for symbol extraction
//!
//! This module contains parsers for different programming languages:
//! - Kotlin/Java (Android)
//! - Swift (iOS)
//! - Objective-C (iOS)
//! - TypeScript/JavaScript (React, Vue, Svelte, Node.js)
//! - Perl
//! - Protocol Buffers (proto2/proto3)
//! - WSDL/XSD (Web Services)
//! - C/C++ (JNI bindings, uservices)
//! - Python (backend services)
//! - Go (backend services)
//! - Rust (systems programming)
//! - Ruby (Rails, RSpec)
//! - C# (.NET, Unity, ASP.NET)
//! - PHP (Laravel, Symfony)
//! - Dart/Flutter

pub mod perl;
pub mod typescript;
pub mod wsdl;

use crate::db::SymbolKind;

/// A parsed symbol from source code
#[derive(Debug, Clone)]
pub struct ParsedSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub line: usize,
    pub signature: String,
    pub parents: Vec<(String, String)>, // (parent_name, inherit_kind)
}

/// A reference/usage of a symbol
#[derive(Debug, Clone)]
pub struct ParsedRef {
    pub name: String,
    pub line: usize,
    pub context: String,
}

/// Max length for context strings stored in DB (characters)
const MAX_CONTEXT_LEN: usize = 500;

/// Max length for symbol signatures stored in DB. Parsers commonly use
/// `line_text(content, line)` as the signature, which copies the *entire*
/// source line. On a minified bundle one line can be the whole file (tens of
/// megabytes), and there are thousands of symbols per file — multiplying out
/// to easily 100+ GB of heap during a single rebuild. Truncating at the DB
/// insert side caps that at `(MAX_SIGNATURE_LEN + 3) bytes per row`.
const MAX_SIGNATURE_LEN: usize = 500;

/// Truncate context to avoid storing huge minified lines
fn truncate_context(s: &str) -> String {
    truncate_to_len(s, MAX_CONTEXT_LEN)
}

/// Public truncation for signatures, called from `indexer::write_batch_to_db`
/// just before INSERT — one chokepoint instead of 220 callsites.
pub fn truncate_signature(s: &str) -> String {
    truncate_to_len(s, MAX_SIGNATURE_LEN)
}

fn truncate_to_len(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while end < s.len() && !s.is_char_boundary(end) {
            end += 1;
        }
        format!("{}...", &s[..end.min(s.len())])
    }
}

use anyhow::Result;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Strip // line comments only (no block comments). Used for BSL.
fn strip_line_comments(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    for line in content.lines() {
        if let Some(pos) = line.find("//") {
            result.push_str(&line[..pos]);
            // Pad with spaces to preserve column positions
            for _ in 0..(line.len() - pos) {
                result.push(' ');
            }
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    result
}

/// Strip C-style comments (// and /* */) while preserving line numbers.
/// Replaces comment content with spaces so line numbers remain correct.
/// Supports nested block comments for Swift and Rust.
pub fn strip_c_comments(content: &str, nested: bool) -> String {
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut result = Vec::with_capacity(len);
    let mut i = 0;

    while i < len {
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            // Single-line comment: replace with spaces until newline
            while i < len && bytes[i] != b'\n' {
                result.push(b' ');
                i += 1;
            }
        } else if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            // Block comment: replace with spaces, preserve newlines
            result.push(b' ');
            result.push(b' ');
            i += 2;
            let mut depth = 1u32;
            while i < len && depth > 0 {
                if nested && i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
                    depth += 1;
                    result.push(b' ');
                    result.push(b' ');
                    i += 2;
                } else if i + 1 < len && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    depth -= 1;
                    result.push(b' ');
                    result.push(b' ');
                    i += 2;
                } else if bytes[i] == b'\n' {
                    result.push(b'\n');
                    i += 1;
                } else {
                    result.push(b' ');
                    i += 1;
                }
            }
        } else if bytes[i] == b'"' {
            // Skip string literals to avoid stripping comments inside strings
            result.push(bytes[i]);
            i += 1;
            while i < len && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < len {
                    result.push(bytes[i]);
                    result.push(bytes[i + 1]);
                    i += 2;
                } else if bytes[i] == b'\n' {
                    result.push(b'\n');
                    i += 1;
                    break; // unterminated string
                } else {
                    result.push(bytes[i]);
                    i += 1;
                }
            }
            if i < len {
                result.push(bytes[i]);
                i += 1;
            }
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }

    String::from_utf8(result).unwrap_or_else(|_| content.to_string())
}

/// Strip hash comments (Python, Ruby, Perl) while preserving line numbers.
pub fn strip_hash_comments(content: &str) -> String {
    content
        .lines()
        .map(|line| {
            // Find # not inside a string
            let mut in_single = false;
            let mut in_double = false;
            let mut prev_was_escape = false;
            let bytes = line.as_bytes();
            for (idx, &b) in bytes.iter().enumerate() {
                if prev_was_escape {
                    prev_was_escape = false;
                    continue;
                }
                if b == b'\\' {
                    prev_was_escape = true;
                    continue;
                }
                if b == b'\'' && !in_double {
                    in_single = !in_single;
                } else if b == b'"' && !in_single {
                    in_double = !in_double;
                } else if b == b'#' && !in_single && !in_double {
                    // Replace from # to end with spaces
                    let mut result = String::with_capacity(line.len());
                    result.push_str(&line[..idx]);
                    for _ in idx..line.len() {
                        result.push(' ');
                    }
                    return result;
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Strip Python docstrings (""" ... """) while preserving line numbers.
pub fn strip_python_docstrings(content: &str) -> String {
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut result = Vec::with_capacity(len);
    let mut i = 0;

    while i < len {
        if i + 2 < len && bytes[i] == b'"' && bytes[i + 1] == b'"' && bytes[i + 2] == b'"' {
            // Triple-quoted string: replace with spaces, preserve newlines
            result.push(b' ');
            result.push(b' ');
            result.push(b' ');
            i += 3;
            while i < len {
                if i + 2 < len && bytes[i] == b'"' && bytes[i + 1] == b'"' && bytes[i + 2] == b'"' {
                    result.push(b' ');
                    result.push(b' ');
                    result.push(b' ');
                    i += 3;
                    break;
                } else if bytes[i] == b'\n' {
                    result.push(b'\n');
                    i += 1;
                } else {
                    result.push(b' ');
                    i += 1;
                }
            }
        } else if i + 2 < len && bytes[i] == b'\'' && bytes[i + 1] == b'\'' && bytes[i + 2] == b'\''
        {
            // Triple single-quoted string
            result.push(b' ');
            result.push(b' ');
            result.push(b' ');
            i += 3;
            while i < len {
                if i + 2 < len
                    && bytes[i] == b'\''
                    && bytes[i + 1] == b'\''
                    && bytes[i + 2] == b'\''
                {
                    result.push(b' ');
                    result.push(b' ');
                    result.push(b' ');
                    i += 3;
                    break;
                } else if bytes[i] == b'\n' {
                    result.push(b'\n');
                    i += 1;
                } else {
                    result.push(b' ');
                    i += 1;
                }
            }
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }

    String::from_utf8(result).unwrap_or_else(|_| content.to_string())
}

/// Strip Ruby block comments (=begin ... =end) while preserving line numbers.
pub fn strip_ruby_block_comments(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let mut in_block = false;
    for line in content.lines() {
        if line.starts_with("=begin") {
            in_block = true;
            result.push_str(&" ".repeat(line.len()));
            result.push('\n');
        } else if line.starts_with("=end") && in_block {
            in_block = false;
            result.push_str(&" ".repeat(line.len()));
            result.push('\n');
        } else if in_block {
            result.push_str(&" ".repeat(line.len()));
            result.push('\n');
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }
    // Remove trailing newline if original didn't have one
    if !content.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }
    result
}

/// Strip Perl POD documentation (=pod/=head ... =cut) while preserving line numbers.
pub fn strip_perl_pod(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let mut in_pod = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("=pod")
            || trimmed.starts_with("=head")
            || trimmed.starts_with("=over")
            || trimmed.starts_with("=item")
            || trimmed.starts_with("=begin")
            || trimmed.starts_with("=for")
            || trimmed.starts_with("=encoding")
        {
            in_pod = true;
            result.push_str(&" ".repeat(line.len()));
            result.push('\n');
        } else if trimmed.starts_with("=cut") && in_pod {
            in_pod = false;
            result.push_str(&" ".repeat(line.len()));
            result.push('\n');
        } else if in_pod {
            result.push_str(&" ".repeat(line.len()));
            result.push('\n');
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }
    if !content.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }
    result
}

/// Strip Matlab comments (% line and %{ ... %} block) while preserving line numbers.
pub fn strip_matlab_comments(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let mut in_block = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if !in_block && trimmed == "%{" {
            in_block = true;
            result.push_str(&" ".repeat(line.len()));
            result.push('\n');
        } else if in_block && trimmed == "%}" {
            in_block = false;
            result.push_str(&" ".repeat(line.len()));
            result.push('\n');
        } else if in_block {
            result.push_str(&" ".repeat(line.len()));
            result.push('\n');
        } else {
            // Find % not inside a string
            let mut in_single = false;
            let bytes = line.as_bytes();
            let mut found = None;
            for (idx, &b) in bytes.iter().enumerate() {
                if b == b'\'' {
                    in_single = !in_single;
                } else if b == b'%' && !in_single {
                    found = Some(idx);
                    break;
                }
            }
            if let Some(idx) = found {
                result.push_str(&line[..idx]);
                for _ in idx..line.len() {
                    result.push(' ');
                }
            } else {
                result.push_str(line);
            }
            result.push('\n');
        }
    }
    if !content.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }
    result
}

/// Strip XML comments (<!-- ... -->) while preserving line numbers.
pub fn strip_xml_comments(content: &str) -> String {
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut result = Vec::with_capacity(len);
    let mut i = 0;

    while i < len {
        if i + 3 < len
            && bytes[i] == b'<'
            && bytes[i + 1] == b'!'
            && bytes[i + 2] == b'-'
            && bytes[i + 3] == b'-'
        {
            // XML comment: replace with spaces, preserve newlines
            result.push(b' ');
            result.push(b' ');
            result.push(b' ');
            result.push(b' ');
            i += 4;
            while i < len {
                if i + 2 < len && bytes[i] == b'-' && bytes[i + 1] == b'-' && bytes[i + 2] == b'>' {
                    result.push(b' ');
                    result.push(b' ');
                    result.push(b' ');
                    i += 3;
                    break;
                } else if bytes[i] == b'\n' {
                    result.push(b'\n');
                    i += 1;
                } else {
                    result.push(b' ');
                    i += 1;
                }
            }
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }

    String::from_utf8(result).unwrap_or_else(|_| content.to_string())
}

// Re-export parser functions for fallback languages (no tree-sitter support)
pub use perl::parse_perl_symbols;
pub use typescript::{extract_svelte_script, extract_vue_script, parse_typescript_symbols};
pub use wsdl::parse_wsdl_symbols;

/// File type for parser dispatch
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Bsl,
    CommonLisp,
    Kotlin,
    Java,
    Swift,
    ObjC,
    Perl,
    Proto,
    Wsdl,
    Cpp,
    Python,
    Go,
    Rust,
    Ruby,
    CSharp,
    Dart,
    TypeScript,
    Vue,
    Svelte,
    Scala,
    Php,
    Lua,
    Matlab,
    Elixir,
    Gdscript,
    Bash,
    Groovy,
    R,
    Sql,
    Zig,
    Css,
    Scss,
    Less,
}

impl FileType {
    /// Determine file type from extension, returns None for unsupported extensions
    pub fn from_extension(ext: &str) -> Option<FileType> {
        match ext {
            "bsl" | "os" => Some(FileType::Bsl),
            "lisp" | "lsp" | "cl" | "asd" => Some(FileType::CommonLisp),
            "kt" => Some(FileType::Kotlin),
            "java" => Some(FileType::Java),
            "swift" => Some(FileType::Swift),
            "m" => Some(FileType::ObjC),
            "h" => Some(FileType::Cpp), // .h can be ObjC or C++, default to C++
            "pm" | "pl" | "t" => Some(FileType::Perl),
            "proto" => Some(FileType::Proto),
            "wsdl" | "xsd" => Some(FileType::Wsdl),
            "cpp" | "cc" | "c" | "hpp" => Some(FileType::Cpp),
            "py" => Some(FileType::Python),
            "go" => Some(FileType::Go),
            "rs" => Some(FileType::Rust),
            "rb" => Some(FileType::Ruby),
            "cs" => Some(FileType::CSharp),
            "dart" => Some(FileType::Dart),
            "ts" | "tsx" | "mts" | "js" | "jsx" | "mjs" | "cjs" => Some(FileType::TypeScript),
            "vue" => Some(FileType::Vue),
            "svelte" => Some(FileType::Svelte),
            "scala" | "sc" => Some(FileType::Scala),
            "php" | "phtml" => Some(FileType::Php),
            "lua" => Some(FileType::Lua),
            "ex" | "exs" => Some(FileType::Elixir),
            "gd" => Some(FileType::Gdscript),
            "sh" | "bash" | "zsh" => Some(FileType::Bash),
            "sql" => Some(FileType::Sql),
            "groovy" | "gradle" => Some(FileType::Groovy),
            "r" | "R" => Some(FileType::R),
            "zig" | "zon" => Some(FileType::Zig),
            "css" | "pcss" | "postcss" => Some(FileType::Css),
            "scss" => Some(FileType::Scss),
            "less" => Some(FileType::Less),
            _ => None,
        }
    }

    /// Detect whether a `.m` file is Matlab or Objective-C by inspecting content.
    /// Scans up to 50 lines accumulating evidence before deciding.
    pub fn detect_m_file_type(content: &str) -> FileType {
        let mut saw_percent_comment = false;

        for line in content.lines().take(50) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // % comments are Matlab-only (never valid ObjC at line start)
            if trimmed.starts_with('%') {
                saw_percent_comment = true;
                continue;
            }

            // Strong ObjC markers — immediate return
            if trimmed.starts_with("#import")
                || trimmed.starts_with("#include")
                || trimmed.starts_with("#pragma")
                || trimmed.starts_with("@interface")
                || trimmed.starts_with("@implementation")
                || trimmed.starts_with("@protocol")
                || trimmed.starts_with("@property")
                || trimmed.starts_with("@synthesize")
                || trimmed.starts_with("@dynamic")
                || trimmed.starts_with("@end")
            {
                return FileType::ObjC;
            }

            // Strong Matlab markers — immediate return
            // Use word boundaries: "function " not "functionality"
            if trimmed.starts_with("classdef ")
                || trimmed.starts_with("classdef(")
                || trimmed == "classdef"
                || trimmed.starts_with("function ")
                || trimmed.starts_with("function[")
                || trimmed == "function"
            {
                return FileType::Matlab;
            }

            // C-style comments → ObjC
            if trimmed.starts_with("//") || trimmed.starts_with("/*") {
                return FileType::ObjC;
            }
        }

        // If % comments found but no strong markers → Matlab
        if saw_percent_comment {
            return FileType::Matlab;
        }

        // Default to ObjC (backward compatible)
        FileType::ObjC
    }
}

/// Detect whether a `.h` file contains Objective-C by looking for ObjC markers.
pub fn detect_h_file_objc(content: &str) -> bool {
    for line in content.lines().take(50) {
        let trimmed = line.trim();
        if trimmed.starts_with("#import")
            || trimmed.starts_with("@interface")
            || trimmed.starts_with("@protocol")
            || trimmed.starts_with("@property")
            || trimmed.starts_with("@class")
            || trimmed.starts_with("NS_ASSUME_NONNULL_BEGIN")
        {
            return true;
        }
    }
    false
}

/// Check if file extension is supported for indexing
pub fn is_supported_extension(ext: &str) -> bool {
    FileType::from_extension(ext).is_some()
}

/// Strip comments from content based on file type, preserving line numbers
fn strip_comments(content: &str, file_type: FileType) -> String {
    match file_type {
        // C-style comments (no nesting)
        // BSL: only // line comments, no block comments /* */
        FileType::Bsl => strip_line_comments(content),
        // C-style comments (no nesting)
        FileType::Kotlin
        | FileType::Java
        | FileType::ObjC
        | FileType::Go
        | FileType::CSharp
        | FileType::Proto
        | FileType::TypeScript
        | FileType::Dart
        | FileType::Cpp
        | FileType::Scala
        | FileType::Php
        | FileType::Groovy
        | FileType::Lua => strip_c_comments(content, false),
        // C-style comments with nesting support
        FileType::Swift | FileType::Rust => strip_c_comments(content, true),
        // Hash comments + docstrings
        FileType::Python => {
            let stripped = strip_python_docstrings(content);
            strip_hash_comments(&stripped)
        }
        // Hash + =begin/=end blocks
        FileType::Ruby => {
            let stripped = strip_ruby_block_comments(content);
            strip_hash_comments(&stripped)
        }
        // Hash + POD
        FileType::Perl => {
            let stripped = strip_perl_pod(content);
            strip_hash_comments(&stripped)
        }
        // XML comments
        FileType::Wsdl => strip_xml_comments(content),
        // Hash comments
        FileType::Bash | FileType::R | FileType::Elixir | FileType::Gdscript => {
            strip_hash_comments(content)
        }

        // Matlab: % line comments and %{ %} block comments
        FileType::Matlab => strip_matlab_comments(content),
        // SQL: -- line comments and /* */ block comments (C-style)
        FileType::Sql => strip_c_comments(content, false),
        // Zig: only // line comments — no block comments in the language.
        FileType::Zig => strip_line_comments(content),
        // CSS: only /* */ block comments. SCSS / Less also support // line
        // comments. Tree-sitter handles them natively; this fallback runs
        // only for non-tree-sitter paths (refs / search) and is safe.
        FileType::Css | FileType::Scss | FileType::Less => strip_c_comments(content, false),

        // Vue/Svelte: comments stripped after script extraction
        // Common Lisp: tree-sitter handles comments natively (unreachable)
        FileType::Vue | FileType::Svelte | FileType::CommonLisp => content.to_string(),
    }
}

pub mod treesitter;

/// Parse symbols and references from file content using FileType enum.
/// Tries tree-sitter first for supported languages, falls back to regex.
pub fn parse_file_symbols(
    content: &str,
    file_type: FileType,
) -> Result<(Vec<ParsedSymbol>, Vec<ParsedRef>)> {
    // For .h files (mapped to Cpp), sniff content for ObjC markers and re-route
    let effective_type = if file_type == FileType::Cpp && detect_h_file_objc(content) {
        FileType::ObjC
    } else {
        file_type
    };

    // Try tree-sitter parser first
    if let Some(ts_parser) = treesitter::get_treesitter_parser(effective_type) {
        let symbols = ts_parser.parse_symbols(content)?;
        let refs = ts_parser.extract_refs_for_lang(content, &symbols, effective_type)?;
        return Ok((symbols, refs));
    }

    // Fallback: regex-based parsing for unsupported languages
    let stripped = strip_comments(content, file_type);
    let content = &stripped;

    let symbols = match file_type {
        FileType::Perl => parse_perl_symbols(content)?,
        FileType::Wsdl => parse_wsdl_symbols(content)?,
        FileType::Vue => {
            let script = extract_vue_script(content);
            let script_stripped = strip_c_comments(&script, false);
            parse_typescript_symbols(&script_stripped)?
        }
        FileType::Svelte => {
            let script = extract_svelte_script(content);
            let script_stripped = strip_c_comments(&script, false);
            parse_typescript_symbols(&script_stripped)?
        }
        // All other types are handled by tree-sitter above
        _ => return Err(anyhow::anyhow!("No parser for {:?}", file_type)),
    };
    let refs = extract_references(content, &symbols)?;
    Ok((symbols, refs))
}

/// Parse only the symbols of a file, skipping reference extraction.
///
/// Used for inputs indexed purely for their definitions — `node_modules` type
/// declarations — where references would describe a library's own internals.
pub fn parse_file_symbols_only(content: &str, file_type: FileType) -> Result<Vec<ParsedSymbol>> {
    let effective_type = if file_type == FileType::Cpp && detect_h_file_objc(content) {
        FileType::ObjC
    } else {
        file_type
    };

    if let Some(ts_parser) = treesitter::get_treesitter_parser(effective_type) {
        return ts_parser.parse_symbols(content);
    }

    let stripped = strip_comments(content, file_type);
    let content = &stripped;

    match file_type {
        FileType::Perl => parse_perl_symbols(content),
        FileType::Wsdl => parse_wsdl_symbols(content),
        FileType::Vue => {
            let script = extract_vue_script(content);
            let script_stripped = strip_c_comments(&script, false);
            parse_typescript_symbols(&script_stripped)
        }
        FileType::Svelte => {
            let script = extract_svelte_script(content);
            let script_stripped = strip_c_comments(&script, false);
            parse_typescript_symbols(&script_stripped)
        }
        _ => Err(anyhow::anyhow!("No parser for {:?}", file_type)),
    }
}

/// Extract references/usages from file content
pub fn extract_references(
    content: &str,
    defined_symbols: &[ParsedSymbol],
) -> Result<Vec<ParsedRef>> {
    extract_references_for_lang(content, defined_symbols, None)
}

/// Extract references/usages from file content, with optional language-specific filtering
pub fn extract_references_for_lang(
    content: &str,
    defined_symbols: &[ParsedSymbol],
    file_type: Option<FileType>,
) -> Result<Vec<ParsedRef>> {
    let mut refs = Vec::new();

    // Map every locally defined symbol to the line it is declared on. Only that
    // line is skipped: a symbol used further down its own file (a constant fed
    // into the next constant, a local interface passed to defineProps) is a real
    // usage, and skipping the whole name hid it from `usages` entirely.
    let mut declared_at: HashMap<&str, HashSet<usize>> = HashMap::new();
    for symbol in defined_symbols {
        declared_at
            .entry(symbol.name.as_str())
            .or_default()
            .insert(symbol.line);
    }
    let is_declaration_line =
        |name: &str, line: usize| declared_at.get(name).is_some_and(|l| l.contains(&line));

    // Regex for identifiers that might be references:
    // - CamelCase identifiers (types, classes) like PaymentRepository, String
    // - SCREAMING_SNAKE_CASE constants like DEAL_KIND_ICONS, MAX_RETRIES
    // - Function calls like getCards(, process(
    //
    // `_` is a word character, so `\b` never fires inside DEAL_KIND_ICONS: without
    // `_` in the class the name matches neither whole nor in parts, and every
    // reference to an upper-snake constant is silently dropped.
    static IDENTIFIER_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b([A-Z][A-Za-z0-9_]*)\b").unwrap());

    let identifier_re = &*IDENTIFIER_RE; // CamelCase types
    static FUNC_CALL_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b([a-z][a-zA-Z0-9]*)\s*\(").unwrap());

    let func_call_re = &*FUNC_CALL_RE; // function calls

    // Common keywords to skip across all languages
    static BASE_KEYWORDS: LazyLock<HashSet<&str>> = LazyLock::new(|| {
        [
            "if",
            "else",
            "while",
            "for",
            "do",
            "try",
            "catch",
            "finally",
            "return",
            "break",
            "continue",
            "throw",
            "is",
            "in",
            "as",
            "true",
            "false",
            "null",
            "this",
            "super",
            "class",
            "interface",
            "enum",
            "import",
            "package",
            "private",
            "public",
            "protected",
            "abstract",
            "final",
            "typealias",
            "get",
            "set",
            "init",
            // Universal stdlib noise
            "String",
            "Int",
            "Double",
            "Float",
            "Boolean",
            "Array",
            "Map",
            "Set",
            "List",
            "Error",
            "Result",
        ]
        .into_iter()
        .collect()
    });

    // Kotlin/Java-specific keywords and stdlib types
    static KOTLIN_JAVA_KEYWORDS: LazyLock<HashSet<&str>> = LazyLock::new(|| {
        [
            "when",
            "object",
            "fun",
            "val",
            "var",
            "internal",
            "override",
            "open",
            "sealed",
            "data",
            "inner",
            "companion",
            "lateinit",
            "const",
            "suspend",
            "inline",
            "crossinline",
            "noinline",
            "reified",
            "annotation",
            "constructor",
            "by",
            "where",
            "Long",
            "Byte",
            "Short",
            "Char",
            "Unit",
            "Any",
            "Nothing",
            "Pair",
            "Triple",
            "MutableList",
            "MutableMap",
            "MutableSet",
            "HashMap",
            "ArrayList",
            "HashSet",
            "Exception",
            "Throwable",
            "Sequence",
        ]
        .into_iter()
        .collect()
    });

    // Swift-specific keywords and stdlib types
    static SWIFT_KEYWORDS: LazyLock<HashSet<&str>> = LazyLock::new(|| {
        [
            "guard",
            "let",
            "var",
            "func",
            "struct",
            "protocol",
            "extension",
            "where",
            "override",
            "mutating",
            "throws",
            "rethrows",
            "async",
            "await",
            "some",
            "weak",
            "unowned",
            "lazy",
            "static",
            "dynamic",
            "required",
            "convenience",
            "Optional",
            "Void",
            "Data",
            "URL",
            "Date",
            "UUID",
            "Bool",
            "Character",
            "UInt",
            "UInt8",
            "UInt16",
            "UInt32",
            "UInt64",
            "Int8",
            "Int16",
            "Int32",
            "Int64",
            "CGFloat",
            "CGPoint",
            "CGSize",
            "CGRect",
            "NSObject",
            "AnyObject",
            "AnyHashable",
            "Dictionary",
            "IndexPath",
            "DispatchQueue",
            "Codable",
            "Equatable",
            "Hashable",
            "Comparable",
            "Identifiable",
            "Sendable",
            "Decodable",
            "Encodable",
            "Any",
            "Never",
        ]
        .into_iter()
        .collect()
    });

    let base_keywords = &*BASE_KEYWORDS;
    let extra_keywords: &HashSet<&str> = match file_type {
        Some(FileType::Swift) => &SWIFT_KEYWORDS,
        Some(FileType::Kotlin) | Some(FileType::Java) => &KOTLIN_JAVA_KEYWORDS,
        _ => &KOTLIN_JAVA_KEYWORDS, // default for backward compat
    };

    let is_swift = matches!(file_type, Some(FileType::Swift));

    for (line_num, line) in content.lines().enumerate() {
        let line_num = line_num + 1;
        let trimmed = line.trim();

        // Skip very long lines (minified code, generated files)
        if trimmed.len() > 2000 {
            continue;
        }

        // Skip import/package declarations
        // Swift imports can have attribute prefixes: @testable, @_spi(...), @_exported, etc.
        if trimmed.starts_with("import ")
            || trimmed.starts_with("package ")
            || (is_swift && trimmed.starts_with('@') && trimmed.contains("import "))
        {
            continue;
        }

        // Skip comments
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with("*") {
            continue;
        }

        // Extract CamelCase types (classes, interfaces, etc.)
        for caps in identifier_re.captures_iter(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            if !name.is_empty()
                && !base_keywords.contains(name)
                && !extra_keywords.contains(name)
                && !is_declaration_line(name, line_num)
            {
                refs.push(ParsedRef {
                    name: name.to_string(),
                    line: line_num,
                    context: truncate_context(trimmed),
                });
            }
        }

        // Extract function calls
        for caps in func_call_re.captures_iter(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            if !name.is_empty()
                && !base_keywords.contains(name)
                && !extra_keywords.contains(name)
                && !is_declaration_line(name, line_num)
            {
                // Only add if name length > 2 to avoid noise
                if name.len() > 2 {
                    refs.push(ParsedRef {
                        name: name.to_string(),
                        line: line_num,
                        context: truncate_context(trimmed),
                    });
                }
            }
        }
    }

    Ok(refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_supported_extension() {
        assert!(is_supported_extension("bsl"));
        assert!(is_supported_extension("os"));
        assert!(is_supported_extension("kt"));
        assert!(is_supported_extension("java"));
        assert!(is_supported_extension("swift"));
        assert!(is_supported_extension("ts"));
        assert!(is_supported_extension("tsx"));
        assert!(is_supported_extension("mts"));
        assert!(is_supported_extension("py"));
        assert!(is_supported_extension("go"));
        assert!(is_supported_extension("rs"));
        assert!(is_supported_extension("rb"));
        assert!(is_supported_extension("cs"));
        assert!(is_supported_extension("dart"));
        assert!(is_supported_extension("proto"));
        assert!(is_supported_extension("cpp"));
        assert!(is_supported_extension("pm"));
        assert!(is_supported_extension("php"));
        assert!(is_supported_extension("phtml"));
        assert!(is_supported_extension("vue"));
        assert!(is_supported_extension("svelte"));
        assert!(is_supported_extension("gd"));
    }

    #[test]
    fn test_unsupported_extensions() {
        assert!(!is_supported_extension("txt"));
        assert!(!is_supported_extension("md"));
        assert!(!is_supported_extension("json"));
        assert!(!is_supported_extension("xml"));
        assert!(!is_supported_extension("yaml"));
        assert!(!is_supported_extension("toml"));
        assert!(!is_supported_extension(""));
    }

    #[test]
    fn test_truncate_context_short() {
        let short = "short string";
        assert_eq!(truncate_context(short), short);
    }

    #[test]
    fn test_truncate_context_long() {
        let long = "a".repeat(1000);
        let truncated = truncate_context(&long);
        assert!(truncated.len() < long.len());
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn test_extract_references_skips_keywords() {
        let content = "if (true) return String\n";
        let symbols = vec![];
        let refs = extract_references(content, &symbols).unwrap();
        // "String" is in keywords, should be skipped
        assert!(!refs.iter().any(|r| r.name == "String"));
        // "if", "return", "true" are not CamelCase or are keywords
        assert!(!refs.iter().any(|r| r.name == "if"));
    }

    #[test]
    fn test_extract_references_finds_types() {
        let content = "val repo: PaymentRepository = PaymentRepositoryImpl()\n";
        let symbols = vec![];
        let refs = extract_references(content, &symbols).unwrap();
        assert!(refs.iter().any(|r| r.name == "PaymentRepository"));
        assert!(refs.iter().any(|r| r.name == "PaymentRepositoryImpl"));
    }

    #[test]
    fn test_extract_references_finds_usage_in_declaring_file() {
        // A symbol used further down its own file is a real usage; skipping the
        // whole name (instead of just the declaration line) hid it from `usages`.
        let content = "export const ICONS = {}\nexport const ORDER = Object.keys(ICONS)\n";
        let symbols = vec![ParsedSymbol {
            name: "ICONS".to_string(),
            kind: SymbolKind::Constant,
            line: 1,
            signature: "export const ICONS".to_string(),
            parents: vec![],
        }];
        let refs = extract_references(content, &symbols).unwrap();
        assert!(
            refs.iter().any(|r| r.name == "ICONS" && r.line == 2),
            "expected ICONS usage on line 2; got {:?}",
            refs.iter().map(|r| (&r.name, r.line)).collect::<Vec<_>>()
        );
        // The declaration itself is still not a reference.
        assert!(!refs.iter().any(|r| r.name == "ICONS" && r.line == 1));
    }

    #[test]
    fn test_parse_file_symbols_only_skips_refs() {
        let content = "export declare const RESULT_TONE: Record<string, string>;\nexport declare function helper(): void;\n";
        let symbols = parse_file_symbols_only(content, FileType::TypeScript).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RESULT_TONE"));
        // Same input through the full entry point still yields references, so the
        // difference is the skipped extraction, not a parser change.
        let (full_symbols, refs) = parse_file_symbols(content, FileType::TypeScript).unwrap();
        assert_eq!(symbols.len(), full_symbols.len());
        assert!(!refs.is_empty());
    }

    #[test]
    fn test_extract_references_finds_upper_snake_case_constants() {
        // `_` is a word character, so a `[A-Z][a-zA-Z0-9]*` class matches an
        // upper-snake name neither whole nor in parts (issue: `usages` returned
        // zero for every SCREAMING_SNAKE constant).
        let content = "const icon = DEAL_KIND_ICONS[kind]\nuseMediaQuery(COARSE_POINTER_MQ)\n";
        let symbols = vec![];
        let refs = extract_references(content, &symbols).unwrap();
        assert!(
            refs.iter().any(|r| r.name == "DEAL_KIND_ICONS"),
            "expected DEAL_KIND_ICONS; got {:?}",
            refs.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
        assert!(
            refs.iter().any(|r| r.name == "COARSE_POINTER_MQ"),
            "expected COARSE_POINTER_MQ; got {:?}",
            refs.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
        // The name must not be split into fragments.
        assert!(!refs.iter().any(|r| r.name == "DEAL" || r.name == "ICONS"));
    }

    #[test]
    fn test_extract_references_skips_defined_symbols() {
        let content = "class MyClass {\n    val other: OtherClass\n}\n";
        let symbols = vec![ParsedSymbol {
            name: "MyClass".to_string(),
            kind: SymbolKind::Class,
            line: 1,
            signature: "class MyClass".to_string(),
            parents: vec![],
        }];
        let refs = extract_references(content, &symbols).unwrap();
        assert!(
            !refs.iter().any(|r| r.name == "MyClass"),
            "should skip locally defined symbols"
        );
        assert!(refs.iter().any(|r| r.name == "OtherClass"));
    }

    #[test]
    fn test_extract_references_skips_imports() {
        let content = "import com.example.MyClass\npackage com.example\n";
        let symbols = vec![];
        let refs = extract_references(content, &symbols).unwrap();
        // import/package lines should be skipped entirely
        assert!(refs.is_empty() || !refs.iter().any(|r| r.line == 1));
    }

    #[test]
    fn test_extract_references_skips_comments() {
        let content = "// MyService is used here\n/* MyOther */\n";
        let symbols = vec![];
        let refs = extract_references(content, &symbols).unwrap();
        assert!(!refs.iter().any(|r| r.line == 1), "should skip // comments");
        assert!(!refs.iter().any(|r| r.line == 2), "should skip /* comments");
    }

    #[test]
    fn test_strip_c_comments() {
        let code = "class Foo {}\n// class Bar {}\nclass Baz {}\n";
        let stripped = strip_c_comments(code, false);
        assert!(stripped.contains("class Foo {}"));
        assert!(!stripped.contains("class Bar"));
        assert!(stripped.contains("class Baz {}"));
        // Line count preserved
        assert_eq!(stripped.lines().count(), code.lines().count());
    }

    #[test]
    fn test_strip_c_block_comments() {
        let code = "class Foo {}\n/* class Bar {} */\nclass Baz {}\n";
        let stripped = strip_c_comments(code, false);
        assert!(stripped.contains("class Foo {}"));
        assert!(!stripped.contains("class Bar"));
        assert!(stripped.contains("class Baz {}"));
    }

    #[test]
    fn test_strip_nested_block_comments() {
        let code = "fn foo() {}\n/* outer /* inner */ still comment */\nfn bar() {}\n";
        let stripped = strip_c_comments(code, true);
        assert!(stripped.contains("fn foo() {}"));
        assert!(!stripped.contains("outer"));
        assert!(!stripped.contains("still comment"));
        assert!(stripped.contains("fn bar() {}"));
    }

    #[test]
    fn test_strip_hash_comments() {
        let code = "def foo():\n    # this is a comment\n    pass\n";
        let stripped = strip_hash_comments(code);
        assert!(stripped.contains("def foo():"));
        assert!(!stripped.contains("this is a comment"));
        assert!(stripped.contains("pass"));
    }

    #[test]
    fn test_strip_python_docstrings() {
        let code =
            "class Foo:\n    \"\"\"This is a docstring\"\"\"\n    def bar(self):\n        pass\n";
        let stripped = strip_python_docstrings(code);
        assert!(!stripped.contains("This is a docstring"));
        assert!(stripped.contains("def bar(self):"));
    }

    #[test]
    fn test_strip_xml_comments() {
        let code =
            "<types>\n<!-- <type name=\"Commented\"/> -->\n<type name=\"Real\"/>\n</types>\n";
        let stripped = strip_xml_comments(code);
        assert!(!stripped.contains("Commented"));
        assert!(stripped.contains("Real"));
    }

    #[test]
    fn test_comment_strip_preserves_line_numbers() {
        let code = "line1\n/* comment\nstill comment */\nline4\n";
        let stripped = strip_c_comments(code, false);
        let lines: Vec<&str> = stripped.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], "line1");
        assert_eq!(lines[3], "line4");
    }

    #[test]
    fn test_kotlin_comment_not_indexed() {
        let code = "class RealClass {}\n// class FakeClass {}\n/* class AnotherFake {} */\n";
        let (symbols, _) = parse_file_symbols(code, FileType::Kotlin).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealClass"));
        assert!(
            !symbols.iter().any(|s| s.name == "FakeClass"),
            "commented class should not be indexed"
        );
        assert!(
            !symbols.iter().any(|s| s.name == "AnotherFake"),
            "block-commented class should not be indexed"
        );
    }

    #[test]
    fn test_python_comment_not_indexed() {
        let code = "class RealClass:\n    pass\n# class FakeClass:\n#     pass\n";
        let (symbols, _) = parse_file_symbols(code, FileType::Python).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealClass"));
        assert!(!symbols.iter().any(|s| s.name == "FakeClass"));
    }

    #[test]
    fn test_go_comment_not_indexed() {
        let code = "type RealStruct struct {}\n// type FakeStruct struct {}\n";
        let (symbols, _) = parse_file_symbols(code, FileType::Go).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealStruct"));
        assert!(!symbols.iter().any(|s| s.name == "FakeStruct"));
    }

    #[test]
    fn test_rust_comment_not_indexed() {
        let code = "struct RealStruct {}\n// struct FakeStruct {}\n/* struct AnotherFake {} */\n";
        let (symbols, _) = parse_file_symbols(code, FileType::Rust).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealStruct"));
        assert!(!symbols.iter().any(|s| s.name == "FakeStruct"));
        assert!(!symbols.iter().any(|s| s.name == "AnotherFake"));
    }

    #[test]
    fn test_swift_comment_not_indexed() {
        let code = "class RealClass {}\n// class FakeClass {}\n/* class AnotherFake {} */\n";
        let (symbols, _) = parse_file_symbols(code, FileType::Swift).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealClass"));
        assert!(!symbols.iter().any(|s| s.name == "FakeClass"));
    }

    #[test]
    fn test_ruby_comment_not_indexed() {
        let code = "class RealClass\nend\n# class FakeClass\n# end\n";
        let (symbols, _) = parse_file_symbols(code, FileType::Ruby).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealClass"));
        assert!(!symbols.iter().any(|s| s.name == "FakeClass"));
    }

    #[test]
    fn test_file_type_from_extension() {
        assert_eq!(FileType::from_extension("bsl"), Some(FileType::Bsl));
        assert_eq!(FileType::from_extension("os"), Some(FileType::Bsl));
        assert_eq!(FileType::from_extension("kt"), Some(FileType::Kotlin));
        assert_eq!(FileType::from_extension("java"), Some(FileType::Java));
        assert_eq!(FileType::from_extension("swift"), Some(FileType::Swift));
        assert_eq!(FileType::from_extension("m"), Some(FileType::ObjC));
        assert_eq!(FileType::from_extension("py"), Some(FileType::Python));
        assert_eq!(FileType::from_extension("go"), Some(FileType::Go));
        assert_eq!(FileType::from_extension("rs"), Some(FileType::Rust));
        assert_eq!(FileType::from_extension("rb"), Some(FileType::Ruby));
        assert_eq!(FileType::from_extension("cs"), Some(FileType::CSharp));
        assert_eq!(FileType::from_extension("dart"), Some(FileType::Dart));
        assert_eq!(FileType::from_extension("ts"), Some(FileType::TypeScript));
        assert_eq!(FileType::from_extension("tsx"), Some(FileType::TypeScript));
        assert_eq!(FileType::from_extension("mts"), Some(FileType::TypeScript));
        assert_eq!(FileType::from_extension("vue"), Some(FileType::Vue));
        assert_eq!(FileType::from_extension("svelte"), Some(FileType::Svelte));
        assert_eq!(FileType::from_extension("php"), Some(FileType::Php));
        assert_eq!(FileType::from_extension("phtml"), Some(FileType::Php));
        assert_eq!(FileType::from_extension("proto"), Some(FileType::Proto));
        assert_eq!(FileType::from_extension("wsdl"), Some(FileType::Wsdl));
        assert_eq!(FileType::from_extension("cpp"), Some(FileType::Cpp));
        assert_eq!(FileType::from_extension("pm"), Some(FileType::Perl));
        assert_eq!(FileType::from_extension("gd"), Some(FileType::Gdscript));
        assert_eq!(FileType::from_extension("txt"), None);
        assert_eq!(FileType::from_extension(""), None);
    }

    #[test]
    fn test_detect_m_file_objc_import() {
        let content = "#import <Foundation/Foundation.h>\n@interface Foo : NSObject\n@end\n";
        assert_eq!(FileType::detect_m_file_type(content), FileType::ObjC);
    }

    #[test]
    fn test_detect_m_file_objc_interface() {
        let content = "@interface MyClass : NSObject\n@end\n";
        assert_eq!(FileType::detect_m_file_type(content), FileType::ObjC);
    }

    #[test]
    fn test_detect_m_file_objc_c_comment() {
        let content = "// This is ObjC\n@implementation Foo\n@end\n";
        assert_eq!(FileType::detect_m_file_type(content), FileType::ObjC);
    }

    #[test]
    fn test_detect_m_file_matlab_function() {
        let content = "function result = myFunc(x)\n    result = x + 1;\nend\n";
        assert_eq!(FileType::detect_m_file_type(content), FileType::Matlab);
    }

    #[test]
    fn test_detect_m_file_matlab_classdef() {
        let content = "classdef MyClass < handle\n    properties\n        Value\n    end\nend\n";
        assert_eq!(FileType::detect_m_file_type(content), FileType::Matlab);
    }

    #[test]
    fn test_detect_m_file_matlab_percent_comment() {
        // Matlab script with % comment followed by plain statement
        let content = "% This is a Matlab script\nx = 5;\ny = x + 1;\n";
        assert_eq!(FileType::detect_m_file_type(content), FileType::Matlab);
    }

    #[test]
    fn test_detect_m_file_matlab_function_with_comments() {
        let content = "% My function\n% Does stuff\nfunction y = helper(x)\n    y = x * 2;\nend\n";
        assert_eq!(FileType::detect_m_file_type(content), FileType::Matlab);
    }

    #[test]
    fn test_detect_m_file_empty() {
        assert_eq!(FileType::detect_m_file_type(""), FileType::ObjC);
    }

    #[test]
    fn test_detect_m_file_word_boundary() {
        // "functionality" should NOT match "function"
        let content = "% comment\nfunctionality = 5;\n";
        assert_eq!(FileType::detect_m_file_type(content), FileType::Matlab);
    }

    // === Issue #2: .h files always parsed as C++, not ObjC ===

    #[test]
    fn test_h_file_with_objc_parsed_as_objc() {
        // In an iOS project, .h files with @interface should be parsed as ObjC
        let content = "#import <Foundation/Foundation.h>\n@interface MyClass : NSObject\n@end\n";
        // FileType::from_extension("h") returns Cpp, but this is clearly ObjC
        // A content-based sniff (like .m files get) should detect ObjC
        let file_type = FileType::from_extension("h").unwrap();
        let (symbols, _) = parse_file_symbols(content, file_type).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "MyClass" && s.kind == SymbolKind::Class),
            "ObjC @interface in .h file should be parsed correctly, got: {:?}",
            symbols
        );
    }

    // === Issue #3: Reference extraction is Kotlin-centric ===

    #[test]
    fn test_extract_references_skips_swift_keywords() {
        // Swift keywords that are not in the skip list should still be skipped
        let content = "guard let value = Optional else { return }\n";
        let symbols = vec![];
        let refs = extract_references_for_lang(content, &symbols, Some(FileType::Swift)).unwrap();
        assert!(
            !refs.iter().any(|r| r.name == "Optional"),
            "Optional should be skipped as Swift stdlib type, got refs: {:?}",
            refs
        );
    }

    #[test]
    fn test_extract_references_skips_swift_stdlib_types() {
        // Common Swift types that generate noise
        let content =
            "let url: URL = URL(string: path)!\nlet data: Data = Data()\nlet void: Void = ()\n";
        let symbols = vec![];
        let refs = extract_references_for_lang(content, &symbols, Some(FileType::Swift)).unwrap();
        // These Swift stdlib types should be filtered out
        assert!(
            !refs.iter().any(|r| r.name == "Void"),
            "Void should be skipped as Swift stdlib type"
        );
    }

    #[test]
    fn test_extract_references_skips_testable_import() {
        // @testable import lines should be skipped like regular imports
        let content = "@testable import MyModule\n";
        let symbols = vec![];
        let refs = extract_references_for_lang(content, &symbols, Some(FileType::Swift)).unwrap();
        assert!(
            !refs.iter().any(|r| r.name == "MyModule"),
            "@testable import should be skipped, got refs: {:?}",
            refs
        );
    }
}
