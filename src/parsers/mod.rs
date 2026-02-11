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

/// Truncate context to avoid storing huge minified lines
fn truncate_context(s: &str) -> String {
    if s.len() <= MAX_CONTEXT_LEN {
        s.to_string()
    } else {
        let mut end = MAX_CONTEXT_LEN;
        while end < s.len() && !s.is_char_boundary(end) {
            end += 1;
        }
        format!("{}...", &s[..end.min(s.len())])
    }
}

use std::collections::HashSet;
use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

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
        if i + 2 < len
            && bytes[i] == b'"' && bytes[i + 1] == b'"' && bytes[i + 2] == b'"'
        {
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
        } else if i + 2 < len
            && bytes[i] == b'\'' && bytes[i + 1] == b'\'' && bytes[i + 2] == b'\''
        {
            // Triple single-quoted string
            result.push(b' ');
            result.push(b' ');
            result.push(b' ');
            i += 3;
            while i < len {
                if i + 2 < len && bytes[i] == b'\'' && bytes[i + 1] == b'\'' && bytes[i + 2] == b'\'' {
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
        if trimmed.starts_with("=pod") || trimmed.starts_with("=head")
            || trimmed.starts_with("=over") || trimmed.starts_with("=item")
            || trimmed.starts_with("=begin") || trimmed.starts_with("=for")
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

/// Strip XML comments (<!-- ... -->) while preserving line numbers.
pub fn strip_xml_comments(content: &str) -> String {
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut result = Vec::with_capacity(len);
    let mut i = 0;

    while i < len {
        if i + 3 < len && bytes[i] == b'<' && bytes[i + 1] == b'!' && bytes[i + 2] == b'-' && bytes[i + 3] == b'-' {
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
pub use typescript::{parse_typescript_symbols, extract_vue_script, extract_svelte_script};
pub use wsdl::parse_wsdl_symbols;

/// File type for parser dispatch
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Kotlin,
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
}

impl FileType {
    /// Determine file type from extension, returns None for unsupported extensions
    pub fn from_extension(ext: &str) -> Option<FileType> {
        match ext {
            "kt" | "java" => Some(FileType::Kotlin),
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
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => Some(FileType::TypeScript),
            "vue" => Some(FileType::Vue),
            "svelte" => Some(FileType::Svelte),
            "scala" | "sc" => Some(FileType::Scala),
            _ => None,
        }
    }
}

/// Check if file extension is supported for indexing
pub fn is_supported_extension(ext: &str) -> bool {
    FileType::from_extension(ext).is_some()
}

/// Strip comments from content based on file type, preserving line numbers
fn strip_comments(content: &str, file_type: FileType) -> String {
    match file_type {
        // C-style comments (no nesting)
        FileType::Kotlin | FileType::ObjC | FileType::Go |
        FileType::CSharp | FileType::Proto | FileType::TypeScript |
        FileType::Dart | FileType::Cpp | FileType::Scala => strip_c_comments(content, false),
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
        // Vue/Svelte: comments stripped after script extraction
        FileType::Vue | FileType::Svelte => content.to_string(),
    }
}

pub mod treesitter;

/// Parse symbols and references from file content using FileType enum.
/// Tries tree-sitter first for supported languages, falls back to regex.
pub fn parse_file_symbols(content: &str, file_type: FileType) -> Result<(Vec<ParsedSymbol>, Vec<ParsedRef>)> {
    // Try tree-sitter parser first
    if let Some(ts_parser) = treesitter::get_treesitter_parser(file_type) {
        let symbols = ts_parser.parse_symbols(content)?;
        let refs = ts_parser.extract_refs(content, &symbols)?;
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

/// Extract references/usages from file content
pub fn extract_references(content: &str, defined_symbols: &[ParsedSymbol]) -> Result<Vec<ParsedRef>> {
    let mut refs = Vec::new();

    // Build set of locally defined symbol names (to skip them)
    let defined_names: HashSet<&str> = defined_symbols.iter().map(|s| s.name.as_str()).collect();

    // Regex for identifiers that might be references:
    // - CamelCase identifiers (types, classes) like PaymentRepository, String
    // - Function calls like getCards(, process(
    static IDENTIFIER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([A-Z][a-zA-Z0-9]*)\b").unwrap());

    let identifier_re = &*IDENTIFIER_RE; // CamelCase types
    static FUNC_CALL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([a-z][a-zA-Z0-9]*)\s*\(").unwrap());

    let func_call_re = &*FUNC_CALL_RE; // function calls

    // Keywords to skip (static to avoid re-creating on every call)
    static KEYWORDS: LazyLock<HashSet<&str>> = LazyLock::new(|| {
        [
            "if", "else", "when", "while", "for", "do", "try", "catch", "finally",
            "return", "break", "continue", "throw", "is", "in", "as", "true", "false",
            "null", "this", "super", "class", "interface", "object", "fun", "val", "var",
            "import", "package", "private", "public", "protected", "internal", "override",
            "abstract", "final", "open", "sealed", "data", "inner", "enum", "companion",
            "lateinit", "const", "suspend", "inline", "crossinline", "noinline", "reified",
            "annotation", "typealias", "get", "set", "init", "constructor", "by", "where",
            // Common standard library that would create too much noise
            "String", "Int", "Long", "Double", "Float", "Boolean", "Byte", "Short", "Char",
            "Unit", "Any", "Nothing", "List", "Map", "Set", "Array", "Pair", "Triple",
            "MutableList", "MutableMap", "MutableSet", "HashMap", "ArrayList", "HashSet",
            "Exception", "Error", "Throwable", "Result", "Sequence",
        ].into_iter().collect()
    });
    let keywords = &*KEYWORDS;

    for (line_num, line) in content.lines().enumerate() {
        let line_num = line_num + 1;
        let trimmed = line.trim();

        // Skip very long lines (minified code, generated files)
        if trimmed.len() > 2000 {
            continue;
        }

        // Skip import/package declarations
        if trimmed.starts_with("import ") || trimmed.starts_with("package ") {
            continue;
        }

        // Skip comments
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with("*") {
            continue;
        }

        // Extract CamelCase types (classes, interfaces, etc.)
        for caps in identifier_re.captures_iter(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            if !name.is_empty() && !keywords.contains(name) && !defined_names.contains(name) {
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
            if !name.is_empty() && !keywords.contains(name) && !defined_names.contains(name) {
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
        assert!(is_supported_extension("kt"));
        assert!(is_supported_extension("java"));
        assert!(is_supported_extension("swift"));
        assert!(is_supported_extension("ts"));
        assert!(is_supported_extension("tsx"));
        assert!(is_supported_extension("py"));
        assert!(is_supported_extension("go"));
        assert!(is_supported_extension("rs"));
        assert!(is_supported_extension("rb"));
        assert!(is_supported_extension("cs"));
        assert!(is_supported_extension("dart"));
        assert!(is_supported_extension("proto"));
        assert!(is_supported_extension("cpp"));
        assert!(is_supported_extension("pm"));
        assert!(is_supported_extension("vue"));
        assert!(is_supported_extension("svelte"));
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
    fn test_extract_references_skips_defined_symbols() {
        let content = "class MyClass {\n    val other: OtherClass\n}\n";
        let symbols = vec![
            ParsedSymbol {
                name: "MyClass".to_string(),
                kind: SymbolKind::Class,
                line: 1,
                signature: "class MyClass".to_string(),
                parents: vec![],
            },
        ];
        let refs = extract_references(content, &symbols).unwrap();
        assert!(!refs.iter().any(|r| r.name == "MyClass"), "should skip locally defined symbols");
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
        let code = "class Foo:\n    \"\"\"This is a docstring\"\"\"\n    def bar(self):\n        pass\n";
        let stripped = strip_python_docstrings(code);
        assert!(!stripped.contains("This is a docstring"));
        assert!(stripped.contains("def bar(self):"));
    }

    #[test]
    fn test_strip_xml_comments() {
        let code = "<types>\n<!-- <type name=\"Commented\"/> -->\n<type name=\"Real\"/>\n</types>\n";
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
        assert!(!symbols.iter().any(|s| s.name == "FakeClass"), "commented class should not be indexed");
        assert!(!symbols.iter().any(|s| s.name == "AnotherFake"), "block-commented class should not be indexed");
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
        assert_eq!(FileType::from_extension("kt"), Some(FileType::Kotlin));
        assert_eq!(FileType::from_extension("java"), Some(FileType::Kotlin));
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
        assert_eq!(FileType::from_extension("vue"), Some(FileType::Vue));
        assert_eq!(FileType::from_extension("svelte"), Some(FileType::Svelte));
        assert_eq!(FileType::from_extension("proto"), Some(FileType::Proto));
        assert_eq!(FileType::from_extension("wsdl"), Some(FileType::Wsdl));
        assert_eq!(FileType::from_extension("cpp"), Some(FileType::Cpp));
        assert_eq!(FileType::from_extension("pm"), Some(FileType::Perl));
        assert_eq!(FileType::from_extension("txt"), None);
        assert_eq!(FileType::from_extension(""), None);
    }
}
