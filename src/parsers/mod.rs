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

pub mod cpp;
pub mod csharp;
pub mod dart;
pub mod go;
pub mod kotlin;
pub mod objc;
pub mod perl;
pub mod proto;
pub mod python;
pub mod ruby;
pub mod rust;
pub mod swift;
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

// Re-export parser functions
pub use cpp::parse_cpp_symbols;
pub use csharp::parse_csharp_symbols;
pub use dart::parse_dart_symbols;
pub use go::parse_go_symbols;
pub use kotlin::{parse_kotlin_symbols, parse_parents};
pub use objc::parse_objc_symbols;
pub use perl::parse_perl_symbols;
pub use proto::parse_proto_symbols;
pub use python::parse_python_symbols;
pub use ruby::parse_ruby_symbols;
pub use rust::parse_rust_symbols;
pub use swift::parse_swift_symbols;
pub use typescript::{parse_typescript_symbols, extract_vue_script, extract_svelte_script};
pub use wsdl::parse_wsdl_symbols;

/// Check if file extension is supported for indexing
pub fn is_supported_extension(ext: &str) -> bool {
    matches!(ext,
        // Kotlin/Java
        "kt" | "java" |
        // Swift/ObjC
        "swift" | "m" | "h" |
        // TypeScript/JavaScript
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "vue" | "svelte" |
        // Perl
        "pm" | "pl" | "t" |
        // Protocol Buffers
        "proto" |
        // WSDL/XSD
        "wsdl" | "xsd" |
        // C/C++
        "cpp" | "cc" | "c" | "hpp" |
        // Python
        "py" |
        // Go
        "go" |
        // Rust
        "rs" |
        // Ruby
        "rb" |
        // C#
        "cs" |
        // Dart/Flutter
        "dart"
    )
}

/// Parse symbols and references from file content
pub fn parse_symbols_and_refs(
    content: &str,
    is_swift: bool,
    is_objc: bool,
    is_perl: bool,
    is_proto: bool,
    is_wsdl: bool,
    is_cpp: bool,
    is_python: bool,
    is_go: bool,
    is_rust: bool,
    is_ruby: bool,
    is_csharp: bool,
    is_dart: bool,
    is_typescript: bool,
    is_vue: bool,
    is_svelte: bool,
) -> Result<(Vec<ParsedSymbol>, Vec<ParsedRef>)> {
    let symbols = if is_swift {
        parse_swift_symbols(content)?
    } else if is_objc {
        parse_objc_symbols(content)?
    } else if is_perl {
        parse_perl_symbols(content)?
    } else if is_proto {
        parse_proto_symbols(content)?
    } else if is_wsdl {
        parse_wsdl_symbols(content)?
    } else if is_cpp {
        parse_cpp_symbols(content)?
    } else if is_python {
        parse_python_symbols(content)?
    } else if is_go {
        parse_go_symbols(content)?
    } else if is_rust {
        parse_rust_symbols(content)?
    } else if is_ruby {
        parse_ruby_symbols(content)?
    } else if is_csharp {
        parse_csharp_symbols(content)?
    } else if is_dart {
        parse_dart_symbols(content)?
    } else if is_typescript {
        parse_typescript_symbols(content)?
    } else if is_vue {
        // Extract script from Vue SFC and parse as TypeScript
        let script = extract_vue_script(content);
        parse_typescript_symbols(&script)?
    } else if is_svelte {
        // Extract script from Svelte and parse as TypeScript
        let script = extract_svelte_script(content);
        parse_typescript_symbols(&script)?
    } else {
        parse_kotlin_symbols(content)?
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
}
