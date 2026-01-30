//! C/C++ symbol parser
//!
//! Parses C and C++ source files (.cpp, .cc, .c, .h, .hpp) to extract:
//! - Classes and structs
//! - Template classes
//! - Functions (including JNI exports and userver handlers)
//! - Namespaces (including nested C++17 syntax)
//! - Includes
//!
//! Supports:
//! - JNI bindings (JNIEXPORT ... JNICALL Java_*)
//! - userver microservices (handlers, views, components)
//! - Modern C++ (C++11/14/17/20)

use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

use crate::db::SymbolKind;
use super::ParsedSymbol;

/// Parse C/C++ source code and extract symbols
pub fn parse_cpp_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();

    // Class declaration: class ClassName or class ClassName : public Base
    static CLASS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^(?:\s*)(?:template\s*<[^>]*>\s*)?(?:class|struct)\s+(\w+)(?:\s*:\s*(?:public|private|protected)\s+(\w+))?"
    ).unwrap());
    let class_re = &*CLASS_RE;

    // Standalone function definition (not inside class)
    static FUNC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^(?:\s*)(?:inline\s+)?(?:static\s+)?(?:virtual\s+)?(?:explicit\s+)?(?:constexpr\s+)?(?:const\s+)?(?:[\w:]+(?:<[^>]*>)?\s*[*&]?\s+)+(\w+)\s*\([^)]*\)\s*(?:const)?\s*(?:noexcept)?\s*(?:override)?\s*[{;]"
    ).unwrap());
    let func_re = &*FUNC_RE;

    // JNI export function: JNIEXPORT type JNICALL Java_package_Class_method
    static JNI_FUNC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^JNIEXPORT\s+\w+\s+JNICALL\s+(Java_[\w_]+)"
    ).unwrap());
    let jni_func_re = &*JNI_FUNC_RE;

    // Template function
    static TEMPLATE_FUNC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^(?:\s*)template\s*<[^>]*>\s*(?:inline\s+)?(?:auto|[\w:]+(?:<[^>]*>)?\s*[*&]?\s+)+(\w+)\s*\([^)]*\)"
    ).unwrap());
    let template_func_re = &*TEMPLATE_FUNC_RE;

    // Namespace - including C++17 nested namespaces (namespace a::b::c {)
    static NAMESPACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^namespace\s+([\w:]+)\s*\{"
    ).unwrap());
    let namespace_re = &*NAMESPACE_RE;

    // Method definition in .cpp: ReturnType ClassName::MethodName(...)
    static METHOD_DEF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^(?:[\w:]+(?:<[^>]*>)?\s*[*&]?\s+)?([\w]+)::([\w]+)\s*\([^)]*\)\s*(?:const)?\s*(?:noexcept)?\s*\{"
    ).unwrap());
    let method_def_re = &*METHOD_DEF_RE;

    // Include - used in parse_cpp_includes, not in symbol extraction
    static INCLUDE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r#"(?m)^#include\s+[<"]([^>"]+)[>"]"#
    ).unwrap());
    let _include_re = &*INCLUDE_RE;

    // Macro define (function-like)
    static MACRO_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^#define\s+(\w+)\s*\([^)]*\)"
    ).unwrap());
    let macro_re = &*MACRO_RE;

    // Typedef
    static TYPEDEF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^typedef\s+.+\s+(\w+)\s*;"
    ).unwrap());
    let typedef_re = &*TYPEDEF_RE;

    // Using alias: using TypeName = SomeType;
    static USING_ALIAS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^using\s+(\w+)\s*="
    ).unwrap());
    let using_alias_re = &*USING_ALIAS_RE;

    // Enum declaration
    static ENUM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^(?:\s*)enum\s+(?:class\s+)?(\w+)"
    ).unwrap());
    let enum_re = &*ENUM_RE;

    let lines: Vec<&str> = content.lines().collect();
    let mut in_class = false;
    let mut class_depth = 0;

    for (line_num, line) in lines.iter().enumerate() {
        let line_num = line_num + 1;
        let trimmed = line.trim();

        // Skip comments
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with("*") {
            continue;
        }

        // Track class scope for method detection
        if trimmed.contains("class ") || trimmed.contains("struct ") {
            if let Some(caps) = class_re.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
                let parent = caps.get(2).map(|m| m.as_str().to_string());

                if !name.is_empty() && !is_forward_declaration(trimmed) {
                    let parents = parent
                        .map(|p| vec![(p, "extends".to_string())])
                        .unwrap_or_default();

                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Class,
                        line: line_num,
                        signature: trimmed.to_string(),
                        parents,
                    });
                    in_class = true;
                }
            }
        }

        // Track brace depth for class scope
        for c in trimmed.chars() {
            match c {
                '{' => {
                    if in_class {
                        class_depth += 1;
                    }
                }
                '}' => {
                    if in_class {
                        class_depth -= 1;
                        if class_depth == 0 {
                            in_class = false;
                        }
                    }
                }
                _ => {}
            }
        }

        // JNI functions (highest priority)
        if let Some(caps) = jni_func_re.captures(line) {
            let full_name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            // Extract method name from Java_package_Class_method
            let name = full_name.rsplit('_').next().unwrap_or(full_name).to_string();
            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Function,
                line: line_num,
                signature: trimmed.to_string(),
                parents: vec![],
            });
            continue;
        }

        // Template functions
        if let Some(caps) = template_func_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() && !is_reserved_word(&name) {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Function,
                    line: line_num,
                    signature: trimmed.to_string(),
                    parents: vec![],
                });
            }
            continue;
        }

        // Regular functions (only at file scope, not in class)
        if !in_class {
            if let Some(caps) = func_re.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
                if !name.is_empty() && !is_reserved_word(&name) && !is_constructor_like(&name, &lines) {
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Function,
                        line: line_num,
                        signature: trimmed.to_string(),
                        parents: vec![],
                    });
                }
            }
        }

        // Namespaces (including nested like views::feeds)
        if let Some(caps) = namespace_re.captures(line) {
            let full_name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            if !full_name.is_empty() {
                for part in full_name.split("::") {
                    if !part.is_empty() {
                        symbols.push(ParsedSymbol {
                            name: part.to_string(),
                            kind: SymbolKind::Package,
                            line: line_num,
                            signature: trimmed.to_string(),
                            parents: vec![],
                        });
                    }
                }
                if full_name.contains("::") {
                    symbols.push(ParsedSymbol {
                        name: full_name.to_string(),
                        kind: SymbolKind::Package,
                        line: line_num,
                        signature: trimmed.to_string(),
                        parents: vec![],
                    });
                }
            }
        }

        // Method definitions in .cpp files (ClassName::MethodName)
        if let Some(caps) = method_def_re.captures(line) {
            let class_name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let method_name = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            if !method_name.is_empty() && !is_reserved_word(method_name) {
                symbols.push(ParsedSymbol {
                    name: method_name.to_string(),
                    kind: SymbolKind::Function,
                    line: line_num,
                    signature: trimmed.to_string(),
                    parents: vec![(class_name.to_string(), "member".to_string())],
                });
            }
        }

        // Enums
        if let Some(caps) = enum_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Enum,
                    line: line_num,
                    signature: trimmed.to_string(),
                    parents: vec![],
                });
            }
        }

        // Typedefs
        if let Some(caps) = typedef_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::TypeAlias,
                    line: line_num,
                    signature: trimmed.to_string(),
                    parents: vec![],
                });
            }
        }

        // Using aliases
        if let Some(caps) = using_alias_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::TypeAlias,
                    line: line_num,
                    signature: trimmed.to_string(),
                    parents: vec![],
                });
            }
        }

        // Macros (function-like)
        if let Some(caps) = macro_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Constant,
                    line: line_num,
                    signature: trimmed.to_string(),
                    parents: vec![],
                });
            }
        }
    }

    Ok(symbols)
}

/// Check if line is a forward declaration (class Foo;)
fn is_forward_declaration(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.ends_with(';') && !trimmed.contains('{')
}

/// Check if name is a C++ reserved word
fn is_reserved_word(name: &str) -> bool {
    matches!(
        name,
        "if" | "else" | "while" | "for" | "do" | "switch" | "case" | "default"
            | "break" | "continue" | "return" | "goto" | "try" | "catch" | "throw"
            | "new" | "delete" | "this" | "sizeof" | "typeid" | "static_cast"
            | "dynamic_cast" | "const_cast" | "reinterpret_cast" | "nullptr"
            | "true" | "false" | "auto" | "register" | "static" | "extern"
            | "mutable" | "thread_local" | "inline" | "virtual" | "explicit"
            | "friend" | "constexpr" | "decltype" | "noexcept" | "override"
            | "final" | "public" | "private" | "protected" | "using" | "namespace"
            | "class" | "struct" | "union" | "enum" | "typedef" | "template"
            | "typename" | "concept" | "requires" | "co_await" | "co_return"
            | "co_yield" | "operator"
    )
}

/// Check if name looks like a constructor (same as some class name)
fn is_constructor_like(name: &str, _lines: &[&str]) -> bool {
    name.starts_with('T') && name.len() > 1 && name.chars().nth(1).map(|c| c.is_uppercase()).unwrap_or(false)
}

/// Parse includes from C/C++ file
#[allow(dead_code)]
pub fn parse_cpp_includes(content: &str) -> Result<Vec<(String, usize)>> {
    let mut includes = Vec::new();
    static INCLUDE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?m)^#include\s+[<"]([^>"]+)[>"]"#).unwrap());
    let include_re = &*INCLUDE_RE;

    for (line_num, line) in content.lines().enumerate() {
        if let Some(caps) = include_re.captures(line) {
            let path = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            includes.push((path, line_num + 1));
        }
    }

    Ok(includes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_class() {
        let content = r#"
class TJavaException {
public:
    TJavaException() {}
};
"#;
        let symbols = parse_cpp_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Class && s.name == "TJavaException"));
    }

    #[test]
    fn test_parse_class_with_inheritance() {
        let content = r#"
class TJniClass : public TJniReference<jclass> {
public:
    TJniClass(JNIEnv* env, const char* name);
};
"#;
        let symbols = parse_cpp_symbols(content).unwrap();
        let class = symbols.iter().find(|s| s.name == "TJniClass").unwrap();
        assert_eq!(class.kind, SymbolKind::Class);
        assert!(class.parents.iter().any(|(p, _)| p == "TJniReference"));
    }

    #[test]
    fn test_parse_template_class() {
        let content = r#"
template<class T>
class TJniReference : public TNonCopyable {
    T value_;
public:
    T Get() const { return value_; }
};
"#;
        let symbols = parse_cpp_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Class && s.name == "TJniReference"));
    }

    #[test]
    fn test_parse_jni_function() {
        let content = r#"
JNIEXPORT jobject JNICALL Java_com_example_textprocessing_TextProcessor_analyze
  (JNIEnv *, jclass, jstring, jint, jboolean);
"#;
        let symbols = parse_cpp_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Function && s.name == "analyze"));
    }

    #[test]
    fn test_parse_template_function() {
        let content = r#"
template<class Func>
inline auto jniWrapExceptions(JNIEnv* env, Func&& func) {
    try { return func(); }
    catch (...) { }
}
"#;
        let symbols = parse_cpp_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Function && s.name == "jniWrapExceptions"));
    }

    #[test]
    fn test_parse_namespace() {
        let content = r#"
namespace NDirect {
    class Foo {};
}
"#;
        let symbols = parse_cpp_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Package && s.name == "NDirect"));
    }

    #[test]
    fn test_parse_enum() {
        let content = r#"
enum class Color {
    RED,
    GREEN,
    BLUE
};
"#;
        let symbols = parse_cpp_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Enum && s.name == "Color"));
    }

    #[test]
    fn test_skip_forward_declaration() {
        let content = r#"
class Foo;
struct Bar;
"#;
        let symbols = parse_cpp_symbols(content).unwrap();
        assert!(symbols.is_empty());
    }

    #[test]
    fn test_parse_includes() {
        let content = r#"
#include <jni.h>
#include "util.h"
#include <util/generic/string.h>
"#;
        let includes = parse_cpp_includes(content).unwrap();
        assert_eq!(includes.len(), 3);
        assert!(includes.iter().any(|(p, _)| p == "jni.h"));
        assert!(includes.iter().any(|(p, _)| p == "util.h"));
    }
}
