//! Dart/Flutter symbol parser
//!
//! Parses Dart source files (.dart) to extract:
//! - Classes (with Dart 3 modifiers: abstract, sealed, final, base, interface, mixin)
//! - Mixins
//! - Extensions and extension types
//! - Enums (enhanced enums with implements/with)
//! - Functions (top-level and methods)
//! - Constructors (named, factory)
//! - Getters/Setters
//! - Typedefs
//! - Properties (final, const, late, var)
//! - Imports/Exports

use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

use crate::db::SymbolKind;
use super::ParsedSymbol;

/// Parse Dart source code and extract symbols
pub fn parse_dart_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();

    // Class: abstract/sealed/final/base/interface/mixin class ClassName extends/with/implements ...
    static CLASS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:(?:abstract|sealed|final|base|interface|mixin)\s+)*class\s+(\w+)(?:\s*<[^>]*>)?(?:\s+(?:extends|with|implements)\s+[^{]+)?"
    ).unwrap());
    let class_re = &*CLASS_RE;

    // Parents extraction from class line
    static CLASS_PARENTS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"class\s+\w+(?:<[^>]*>)?\s+((?:extends|with|implements)\s+.+?)(?:\s*\{|$)"
    ).unwrap());
    let class_parents_re = &*CLASS_PARENTS_RE;

    // Mixin: mixin MixinName on BaseClass implements Interface
    static MIXIN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*mixin\s+(\w+)(?:\s*<[^>]*>)?(?:\s+on\s+([^{]+))?"
    ).unwrap());
    let mixin_re = &*MIXIN_RE;

    // Extension: extension ExtName on Type
    static EXTENSION_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*extension\s+(\w+)\s+on\s+([^{]+)"
    ).unwrap());
    let extension_re = &*EXTENSION_RE;

    // Extension type (Dart 3.3): extension type Name(Type _) implements ...
    static EXTENSION_TYPE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*extension\s+type\s+(\w+)\s*\([^)]*\)(?:\s+implements\s+([^{]+))?"
    ).unwrap());
    let extension_type_re = &*EXTENSION_TYPE_RE;

    // Enum: enum Name { ... } or enum Name with Mixin implements Interface { ... }
    static ENUM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*enum\s+(\w+)(?:\s*<[^>]*>)?(?:\s+(?:with|implements)\s+([^{]+))?"
    ).unwrap());
    let enum_re = &*ENUM_RE;

    // Top-level function / method: returnType name(params) or void name(params)
    // Also: static/async/Future<T>/Stream<T>
    static FUNC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:@\w+(?:\([^)]*\))?\s+)*(?:(?:external|static|abstract)\s+)*(?:(?:Future|Stream|FutureOr)(?:<[^>]*>)?\s+|(?:void|int|double|bool|String|num|dynamic|Never|Object|Null|Iterable|List|Map|Set|[A-Z]\w*(?:<[^>]*>)?)\s+|(?:[a-z]\w*(?:<[^>]*>)?)\s+)?(\w+)\s*(?:<[^>]*>)?\s*\([^)]*\)\s*(?:async\s*)?(?:\{|=>|;)"
    ).unwrap());
    let func_re = &*FUNC_RE;

    // Constructor: ClassName(params) or ClassName.named(params) or factory ClassName...
    static CONSTRUCTOR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:const\s+)?(?:factory\s+)?([A-Z]\w*)(?:\.(\w+))?\s*\([^)]*\)\s*(?::\s*[^{;]+)?(?:\{|;|=>)"
    ).unwrap());
    let constructor_re = &*CONSTRUCTOR_RE;

    // Getter: get name => ... or get name { ... }
    static GETTER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:(?:external|static)\s+)*(?:\w+(?:<[^>]*>)?\s+)?get\s+(\w+)"
    ).unwrap());
    let getter_re = &*GETTER_RE;

    // Setter: set name(Type value) { ... }
    static SETTER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:(?:external|static)\s+)*set\s+(\w+)\s*\("
    ).unwrap());
    let setter_re = &*SETTER_RE;

    // Typedef: typedef Name = ... or typedef ReturnType Name(params)
    static TYPEDEF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*typedef\s+(?:\w+(?:<[^>]*>)?\s+)?(\w+)(?:\s*<[^>]*>)?\s*(?:=|\()"
    ).unwrap());
    let typedef_re = &*TYPEDEF_RE;

    // Top-level property: final/const/late/var Type? name = ...;
    static PROPERTY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^(?:final|const|late\s+final|late)\s+(?:\w+(?:<[^>]*>)?\??\s+)?(\w+)\s*[=;]"
    ).unwrap());
    let property_re = &*PROPERTY_RE;

    // Import: import 'package:...' or import '...'
    static IMPORT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r#"(?m)^[ \t]*(?:import|export)\s+['"]([^'"]+)['"]"#
    ).unwrap());
    let import_re = &*IMPORT_RE;

    // Track class names for constructor matching
    let mut class_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    // First pass: collect class names
    for line in content.lines() {
        if let Some(caps) = class_re.captures(line) {
            if let Some(name) = caps.get(1) {
                class_names.insert(name.as_str().to_string());
            }
        }
    }

    // Keywords to skip in function detection
    let func_keywords: std::collections::HashSet<&str> = [
        "if", "else", "while", "for", "switch", "catch", "return", "throw",
        "assert", "print", "super", "this", "new", "const", "final", "var",
        "import", "export", "part", "library", "class", "enum", "mixin",
        "extension", "typedef", "abstract", "sealed", "base", "interface",
    ].into_iter().collect();

    for (line_num, line) in content.lines().enumerate() {
        let line_num = line_num + 1;
        let trimmed = line.trim();

        // Skip comments
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with("*") {
            continue;
        }

        // Extension type (check before extension to avoid false match)
        if trimmed.contains("extension type ") {
            if let Some(caps) = extension_type_re.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
                let parents = if let Some(impls) = caps.get(2) {
                    parse_dart_parent_list(impls.as_str(), "implements")
                } else {
                    vec![]
                };

                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Class,
                    line: line_num,
                    signature: trimmed.to_string(),
                    parents,
                });
                continue;
            }
        }

        // Extension (not extension type)
        if trimmed.starts_with("extension ") && !trimmed.contains("extension type ") {
            if let Some(caps) = extension_re.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
                let on_type = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
                let base = on_type.split('<').next().unwrap_or("").trim().to_string();
                let parents = if !base.is_empty() {
                    vec![(base, "extends".to_string())]
                } else {
                    vec![]
                };

                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Object,
                    line: line_num,
                    signature: trimmed.to_string(),
                    parents,
                });
                continue;
            }
        }

        // Mixin (check before class since "mixin class" should match class)
        if trimmed.starts_with("mixin ") && !trimmed.contains("mixin class ") {
            if let Some(caps) = mixin_re.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
                let parents = if let Some(on_part) = caps.get(2) {
                    parse_dart_mixin_parents(on_part.as_str())
                } else {
                    vec![]
                };

                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Interface,
                    line: line_num,
                    signature: trimmed.to_string(),
                    parents,
                });
                continue;
            }
        }

        // Class (including abstract class, sealed class, mixin class, etc.)
        if let Some(caps) = class_re.captures(line) {
            // Make sure it's actually a class declaration
            if trimmed.contains("class ") {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();

                // Collect multiline declaration: if line doesn't contain '{',
                // next lines may have 'with', 'implements', 'extends'
                let mut full_decl = trimmed.to_string();
                if !trimmed.contains('{') {
                    let lines_vec: Vec<&str> = content.lines().collect();
                    let mut next = line_num; // line_num is 1-indexed, so this is next line index
                    while next < lines_vec.len() {
                        let next_trimmed = lines_vec[next].trim();
                        if next_trimmed.starts_with("with ")
                            || next_trimmed.starts_with("implements ")
                            || next_trimmed.starts_with("extends ")
                        {
                            full_decl.push(' ');
                            full_decl.push_str(next_trimmed);
                            next += 1;
                            if next_trimmed.contains('{') {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }

                let parents = if let Some(pcaps) = class_parents_re.captures(&full_decl) {
                    let parents_part = pcaps.get(1).map(|m| m.as_str()).unwrap_or("");
                    parse_dart_class_parents(parents_part)
                } else {
                    vec![]
                };

                // "abstract interface class" or just "interface class" → Interface
                let kind = if trimmed.contains("interface class ") {
                    SymbolKind::Interface
                } else {
                    SymbolKind::Class
                };

                symbols.push(ParsedSymbol {
                    name: name.clone(),
                    kind,
                    line: line_num,
                    signature: trimmed.to_string(),
                    parents,
                });
                continue;
            }
        }

        // Enum
        if let Some(caps) = enum_re.captures(line) {
            if trimmed.starts_with("enum ") {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
                let parents = if let Some(parent_part) = caps.get(2) {
                    parse_dart_enum_parents(parent_part.as_str())
                } else {
                    vec![]
                };

                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Enum,
                    line: line_num,
                    signature: trimmed.to_string(),
                    parents,
                });
                continue;
            }
        }

        // Typedef
        if trimmed.starts_with("typedef ") {
            if let Some(caps) = typedef_re.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::TypeAlias,
                    line: line_num,
                    signature: trimmed.to_string(),
                    parents: vec![],
                });
                continue;
            }
        }

        // Getter
        if let Some(caps) = getter_re.captures(line) {
            if trimmed.contains(" get ") || trimmed.starts_with("get ") {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Property,
                    line: line_num,
                    signature: trimmed.to_string(),
                    parents: vec![],
                });
                continue;
            }
        }

        // Setter
        if let Some(caps) = setter_re.captures(line) {
            if trimmed.contains(" set ") || trimmed.starts_with("set ") {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Property,
                    line: line_num,
                    signature: trimmed.to_string(),
                    parents: vec![],
                });
                continue;
            }
        }

        // Import/Export
        if trimmed.starts_with("import ") || trimmed.starts_with("export ") {
            if let Some(caps) = import_re.captures(line) {
                let path = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
                // Extract short name from import path
                let short_name = path.rsplit('/').next().unwrap_or(&path)
                    .trim_end_matches(".dart");
                symbols.push(ParsedSymbol {
                    name: short_name.to_string(),
                    kind: SymbolKind::Import,
                    line: line_num,
                    signature: trimmed.to_string(),
                    parents: vec![],
                });
                continue;
            }
        }

        // Constructor (ClassName(...) or ClassName.named(...))
        if let Some(caps) = constructor_re.captures(line) {
            let class_name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            if class_names.contains(class_name) {
                let named = caps.get(2).map(|m| m.as_str());
                let name = if let Some(n) = named {
                    format!("{}.{}", class_name, n)
                } else {
                    class_name.to_string()
                };
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Function,
                    line: line_num,
                    signature: trimmed.to_string(),
                    parents: vec![],
                });
                continue;
            }
        }

        // Top-level property (must be before func to avoid conflicts)
        if let Some(caps) = property_re.captures(line) {
            if trimmed.starts_with("final ") || trimmed.starts_with("const ")
                || trimmed.starts_with("late ") {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
                if !name.is_empty() {
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Property,
                        line: line_num,
                        signature: trimmed.to_string(),
                        parents: vec![],
                    });
                    continue;
                }
            }
        }

        // Function/method
        if let Some(caps) = func_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            if !name.is_empty() && !func_keywords.contains(name) && !class_names.contains(name) {
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    line: line_num,
                    signature: trimmed.to_string(),
                    parents: vec![],
                });
            }
        }
    }

    Ok(symbols)
}

/// Parse parents from a Dart class declaration
/// Input: "extends Base with Mixin1, Mixin2 implements Interface1, Interface2"
fn parse_dart_class_parents(parents_str: &str) -> Vec<(String, String)> {
    let mut parents = Vec::new();
    let mut remaining = parents_str.trim();

    // Parse extends
    if let Some(rest) = remaining.strip_prefix("extends") {
        let rest = rest.trim();
        let end = rest.find(|c: char| c == ' ' || c == '{')
            .unwrap_or(rest.len());
        let base = rest[..end].trim().split('<').next().unwrap_or("").trim();
        if !base.is_empty() {
            parents.push((base.to_string(), "extends".to_string()));
        }
        remaining = rest[end..].trim();
    }

    // Parse with
    if let Some(rest) = remaining.strip_prefix("with") {
        let rest = rest.trim();
        let end = rest.find("implements").unwrap_or(rest.len());
        let end = end.min(rest.find('{').unwrap_or(rest.len()));
        for mixin in rest[..end].split(',') {
            let name = mixin.trim().split('<').next().unwrap_or("").trim();
            if !name.is_empty() {
                parents.push((name.to_string(), "with".to_string()));
            }
        }
        remaining = rest[end..].trim();
    }

    // Parse implements
    if let Some(rest) = remaining.strip_prefix("implements") {
        let rest = rest.trim();
        let end = rest.find('{').unwrap_or(rest.len());
        for iface in rest[..end].split(',') {
            let name = iface.trim().split('<').next().unwrap_or("").trim();
            if !name.is_empty() {
                parents.push((name.to_string(), "implements".to_string()));
            }
        }
    }

    parents
}

/// Parse mixin parents from "on" clause
/// Input: "_AppScopeDeps implements AppScope" (the part after "on")
fn parse_dart_mixin_parents(on_str: &str) -> Vec<(String, String)> {
    let mut parents = Vec::new();
    let trimmed = on_str.trim();

    // Split on "implements" keyword
    let (on_part, impl_part) = if let Some(idx) = trimmed.find("implements") {
        (&trimmed[..idx], Some(&trimmed[idx + "implements".len()..]))
    } else {
        (trimmed, None)
    };

    // Parse "on" types (comma-separated)
    for item in on_part.split(',') {
        let name = item.trim().split('<').next().unwrap_or("").trim();
        if !name.is_empty() {
            parents.push((name.to_string(), "extends".to_string()));
        }
    }

    // Parse "implements" types
    if let Some(impls) = impl_part {
        let impls = impls.trim().trim_end_matches('{').trim();
        for item in impls.split(',') {
            let name = item.trim().split('<').next().unwrap_or("").trim();
            if !name.is_empty() {
                parents.push((name.to_string(), "implements".to_string()));
            }
        }
    }

    parents
}

/// Parse parent list for enums (with/implements)
fn parse_dart_enum_parents(parents_str: &str) -> Vec<(String, String)> {
    let mut parents = Vec::new();
    let parts = parents_str.trim();

    // Enums can have "with Mixin implements Interface" or just "implements Interface"
    // The regex captures everything after with/implements
    for item in parts.split(',') {
        let name = item.trim()
            .trim_start_matches("with").trim_start_matches("implements").trim()
            .split('<').next().unwrap_or("").trim();
        if !name.is_empty() {
            parents.push((name.to_string(), "implements".to_string()));
        }
    }

    parents
}

/// Parse a simple comma-separated parent list with a given kind
fn parse_dart_parent_list(list: &str, kind: &str) -> Vec<(String, String)> {
    let mut parents = Vec::new();
    for item in list.split(',') {
        let name = item.trim().split('<').next().unwrap_or("").trim();
        if !name.is_empty() {
            parents.push((name.to_string(), kind.to_string()));
        }
    }
    parents
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_class() {
        let content = "class MyWidget extends StatefulWidget {\n}\n";
        let symbols = parse_dart_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "MyWidget").unwrap();
        assert_eq!(cls.kind, SymbolKind::Class);
        assert!(cls.parents.iter().any(|(p, k)| p == "StatefulWidget" && k == "extends"));
    }

    #[test]
    fn test_parse_abstract_class() {
        let content = "abstract class AppScope extends ScopeContainer {\n}\n";
        let symbols = parse_dart_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "AppScope").unwrap();
        assert_eq!(cls.kind, SymbolKind::Class);
        assert!(cls.parents.iter().any(|(p, k)| p == "ScopeContainer" && k == "extends"));
    }

    #[test]
    fn test_parse_sealed_class() {
        let content = "sealed class Result {\n}\n";
        let symbols = parse_dart_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "Result").unwrap();
        assert_eq!(cls.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_class_with_mixins_and_implements() {
        let content = "class App extends StatefulWidget with WidgetsBindingObserver implements Listenable {\n}\n";
        let symbols = parse_dart_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "App").unwrap();
        assert_eq!(cls.kind, SymbolKind::Class);
        assert!(cls.parents.iter().any(|(p, k)| p == "StatefulWidget" && k == "extends"));
        assert!(cls.parents.iter().any(|(p, k)| p == "WidgetsBindingObserver" && k == "with"));
        assert!(cls.parents.iter().any(|(p, k)| p == "Listenable" && k == "implements"));
    }

    #[test]
    fn test_parse_mixin() {
        let content = "mixin _AppScopeDeps on AppScopeContainer {\n}\n";
        let symbols = parse_dart_symbols(content).unwrap();
        let m = symbols.iter().find(|s| s.name == "_AppScopeDeps").unwrap();
        assert_eq!(m.kind, SymbolKind::Interface);
        assert!(m.parents.iter().any(|(p, k)| p == "AppScopeContainer" && k == "extends"));
    }

    #[test]
    fn test_parse_extension() {
        let content = "extension DateTimeX on DateTime {\n}\n";
        let symbols = parse_dart_symbols(content).unwrap();
        let ext = symbols.iter().find(|s| s.name == "DateTimeX").unwrap();
        assert_eq!(ext.kind, SymbolKind::Object);
        assert!(ext.parents.iter().any(|(p, k)| p == "DateTime" && k == "extends"));
    }

    #[test]
    fn test_parse_extension_type() {
        let content = "extension type UserId(int id) implements int {\n}\n";
        let symbols = parse_dart_symbols(content).unwrap();
        let et = symbols.iter().find(|s| s.name == "UserId").unwrap();
        assert_eq!(et.kind, SymbolKind::Class);
        assert!(et.parents.iter().any(|(p, _)| p == "int"));
    }

    #[test]
    fn test_parse_enum() {
        let content = "enum TroubleLevel {\n  low,\n  high,\n}\n";
        let symbols = parse_dart_symbols(content).unwrap();
        let e = symbols.iter().find(|s| s.name == "TroubleLevel").unwrap();
        assert_eq!(e.kind, SymbolKind::Enum);
    }

    #[test]
    fn test_parse_function() {
        let content = r#"
void main() {
}
Future<int> fetchData() async {
}
String formatName(String first, String last) => '$first $last';
"#;
        let symbols = parse_dart_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "main" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "fetchData" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "formatName" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_constructor() {
        let content = r#"
class MyService {
  MyService(this._dep);
  MyService.fromJson(Map<String, dynamic> json) {
  }
  factory MyService.create() => MyService(Dep());
}
"#;
        let symbols = parse_dart_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "MyService" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "MyService.fromJson" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "MyService.create" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_getter_setter() {
        let content = r#"
  int get count => _count;
  set count(int value) {
    _count = value;
  }
  static String get instance => _instance;
"#;
        let symbols = parse_dart_symbols(content).unwrap();
        let getters: Vec<_> = symbols.iter().filter(|s| s.name == "count" && s.kind == SymbolKind::Property).collect();
        assert!(getters.len() >= 1, "should find getter 'count'");
        let setters: Vec<_> = symbols.iter().filter(|s| s.name == "count" && s.kind == SymbolKind::Property && s.signature.contains("set ")).collect();
        assert!(setters.len() >= 1, "should find setter 'count'");
        assert!(symbols.iter().any(|s| s.name == "instance" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_typedef() {
        let content = r#"
typedef JsonMap = Map<String, dynamic>;
typedef VoidCallback = void Function();
typedef ResponseParser(Response response);
"#;
        let symbols = parse_dart_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "JsonMap" && s.kind == SymbolKind::TypeAlias));
        assert!(symbols.iter().any(|s| s.name == "VoidCallback" && s.kind == SymbolKind::TypeAlias));
        assert!(symbols.iter().any(|s| s.name == "ResponseParser" && s.kind == SymbolKind::TypeAlias));
    }

    #[test]
    fn test_parse_property() {
        let content = r#"
final String appName = 'MyApp';
const int maxRetries = 3;
late final Logger logger;
"#;
        let symbols = parse_dart_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "appName" && s.kind == SymbolKind::Property));
        assert!(symbols.iter().any(|s| s.name == "maxRetries" && s.kind == SymbolKind::Property));
        assert!(symbols.iter().any(|s| s.name == "logger" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_import() {
        let content = r#"
import 'package:flutter/material.dart';
import 'dart:async';
export 'src/my_widget.dart';
"#;
        let symbols = parse_dart_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "material" && s.kind == SymbolKind::Import));
        assert!(symbols.iter().any(|s| s.name == "dart:async" && s.kind == SymbolKind::Import));
        assert!(symbols.iter().any(|s| s.name == "my_widget" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn test_skip_comments() {
        let content = r#"
// class FakeClass {
/* class AnotherFake { */
class RealClass {
}
"#;
        let symbols = parse_dart_symbols(content).unwrap();
        assert!(!symbols.iter().any(|s| s.name == "FakeClass"));
        assert!(!symbols.iter().any(|s| s.name == "AnotherFake"));
        assert!(symbols.iter().any(|s| s.name == "RealClass" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_mixin_with_implements() {
        let content = "mixin _PublicAppScopeImpl on _AppScopeDeps implements AppScope {\n}\n";
        let symbols = parse_dart_symbols(content).unwrap();
        let m = symbols.iter().find(|s| s.name == "_PublicAppScopeImpl").unwrap();
        assert_eq!(m.kind, SymbolKind::Interface);
        assert!(m.parents.iter().any(|(p, k)| p == "_AppScopeDeps" && k == "extends"),
            "should have _AppScopeDeps as extends parent, got: {:?}", m.parents);
        assert!(m.parents.iter().any(|(p, k)| p == "AppScope" && k == "implements"),
            "should have AppScope as implements parent, got: {:?}", m.parents);
    }

    #[test]
    fn test_parse_abstract_interface_class() {
        let content = "abstract interface class AppScope {\n}\n";
        let symbols = parse_dart_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "AppScope").unwrap();
        assert_eq!(cls.kind, SymbolKind::Interface,
            "abstract interface class should be Interface, got: {:?}", cls.kind);
    }

    #[test]
    fn test_parse_multiline_class() {
        let content = r#"class _AppScopeContainer extends AppScopeContainer
    with _AppScopeDeps, _AppScopeInitializeQueue, _PublicAppScopeImpl {
  _AppScopeContainer();
}
"#;
        let symbols = parse_dart_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "_AppScopeContainer" && s.kind == SymbolKind::Class).unwrap();
        assert!(cls.parents.iter().any(|(p, k)| p == "AppScopeContainer" && k == "extends"),
            "should have AppScopeContainer as extends, got: {:?}", cls.parents);
        assert!(cls.parents.iter().any(|(p, k)| p == "_AppScopeDeps" && k == "with"),
            "should have _AppScopeDeps as with, got: {:?}", cls.parents);
        assert!(cls.parents.iter().any(|(p, k)| p == "_AppScopeInitializeQueue" && k == "with"),
            "should have _AppScopeInitializeQueue as with, got: {:?}", cls.parents);
        assert!(cls.parents.iter().any(|(p, k)| p == "_PublicAppScopeImpl" && k == "with"),
            "should have _PublicAppScopeImpl as with, got: {:?}", cls.parents);
    }

    #[test]
    fn test_parse_class_parents() {
        let parents = parse_dart_class_parents("extends Base with Mixin1, Mixin2 implements IFoo, IBar");
        assert_eq!(parents.len(), 5);
        assert_eq!(parents[0], ("Base".to_string(), "extends".to_string()));
        assert_eq!(parents[1], ("Mixin1".to_string(), "with".to_string()));
        assert_eq!(parents[2], ("Mixin2".to_string(), "with".to_string()));
        assert_eq!(parents[3], ("IFoo".to_string(), "implements".to_string()));
        assert_eq!(parents[4], ("IBar".to_string(), "implements".to_string()));
    }

    #[test]
    fn test_full_dart_file() {
        let content = r#"
import 'package:flutter/material.dart';
import 'dart:async';

typedef JsonMap = Map<String, dynamic>;

const String appVersion = '1.0.0';

mixin LoggerMixin on Object {
  void log(String msg) {}
}

abstract class BaseService {
  Future<void> init();
}

class ApiService extends BaseService with LoggerMixin implements Disposable {
  final String baseUrl;

  ApiService(this.baseUrl);

  ApiService.withDefault() : baseUrl = 'https://api.example.com';

  factory ApiService.create() => ApiService.withDefault();

  @override
  Future<void> init() async {}

  String get endpoint => '$baseUrl/v1';

  set timeout(int value) {}
}

extension ApiServiceX on ApiService {
  void ping() {}
}

enum Status {
  loading,
  success,
  error,
}
"#;
        let symbols = parse_dart_symbols(content).unwrap();

        // Imports
        assert!(symbols.iter().any(|s| s.name == "material" && s.kind == SymbolKind::Import));

        // Typedef
        assert!(symbols.iter().any(|s| s.name == "JsonMap" && s.kind == SymbolKind::TypeAlias));

        // Property
        assert!(symbols.iter().any(|s| s.name == "appVersion" && s.kind == SymbolKind::Property));

        // Mixin
        let mixin = symbols.iter().find(|s| s.name == "LoggerMixin").unwrap();
        assert_eq!(mixin.kind, SymbolKind::Interface);

        // Abstract class
        let base = symbols.iter().find(|s| s.name == "BaseService").unwrap();
        assert_eq!(base.kind, SymbolKind::Class);

        // Class with full inheritance
        let api = symbols.iter().find(|s| s.name == "ApiService" && s.kind == SymbolKind::Class).unwrap();
        assert!(api.parents.iter().any(|(p, k)| p == "BaseService" && k == "extends"));
        assert!(api.parents.iter().any(|(p, k)| p == "LoggerMixin" && k == "with"));
        assert!(api.parents.iter().any(|(p, k)| p == "Disposable" && k == "implements"));

        // Constructors
        assert!(symbols.iter().any(|s| s.name == "ApiService.withDefault" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "ApiService.create" && s.kind == SymbolKind::Function));

        // Getter/Setter
        assert!(symbols.iter().any(|s| s.name == "endpoint" && s.kind == SymbolKind::Property));
        assert!(symbols.iter().any(|s| s.name == "timeout" && s.kind == SymbolKind::Property));

        // Extension
        let ext = symbols.iter().find(|s| s.name == "ApiServiceX").unwrap();
        assert_eq!(ext.kind, SymbolKind::Object);
        assert!(ext.parents.iter().any(|(p, k)| p == "ApiService" && k == "extends"));

        // Enum
        assert!(symbols.iter().any(|s| s.name == "Status" && s.kind == SymbolKind::Enum));

        // Function inside class
        assert!(symbols.iter().any(|s| s.name == "init" && s.kind == SymbolKind::Function));
    }
}
