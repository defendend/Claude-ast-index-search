//! Kotlin/Java symbol parser
//!
//! Parses Kotlin and Java source files (.kt, .java) to extract:
//! - Classes, Objects
//! - Interfaces
//! - Enums
//! - Functions
//! - Properties (val/var)
//! - Type aliases

use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

use crate::db::SymbolKind;
use super::ParsedSymbol;

/// Parse Kotlin/Java source code and extract symbols
pub fn parse_kotlin_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();

    // Simple regex for detecting class/interface start
    static CLASS_START_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*((?:public|private|protected|internal|abstract|open|final|sealed|data|value|inline|annotation|inner|enum)[\s]+)*(?:class|object)\s+(\w+)"

    ).unwrap());

    let class_start_re = &*CLASS_START_RE;

    static INTERFACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*((?:public|private|protected|internal|sealed|fun)[\s]+)*interface\s+(\w+)(?:\s*<[^>]*>)?(?:\s*:\s*([^{]+))?"


    ).unwrap());


    let interface_re = &*INTERFACE_RE;

    static FUN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*((?:public|private|protected|internal|override|suspend|inline|operator|infix|tailrec|external|actual|expect)[\s]+)*fun\s+(?:<[^>]*>\s*)?(?:(\w+)\.)?(\w+)\s*\(([^)]*)\)(?:\s*:\s*(\S+))?"


    ).unwrap());


    let fun_re = &*FUN_RE;

    static PROPERTY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*((?:public|private|protected|internal|override|const|lateinit|lazy)[\s]+)*(?:val|var)\s+(\w+)(?:\s*:\s*(\S+))?"


    ).unwrap());


    let property_re = &*PROPERTY_RE;

    static TYPEALIAS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[\s]*typealias\s+(\w+)(?:\s*<[^>]*>)?\s*=\s*(.+)").unwrap());


    let typealias_re = &*TYPEALIAS_RE;
    static ENUM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[\s]*((?:public|private|protected|internal)[\s]+)*enum\s+class\s+(\w+)").unwrap());

    let enum_re = &*ENUM_RE;

    // Java static fields: public static final Type NAME = value;
    static JAVA_FIELD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*((?:public|private|protected)[\s]+)?(?:static[\s]+)?(?:final[\s]+)?(\w+(?:<[^>]+>)?)\s+([A-Z][A-Z0-9_]*)\s*="

    ).unwrap());

    let java_field_re = &*JAVA_FIELD_RE;

    let lines: Vec<&str> = content.lines().collect();

    for (line_num, line) in lines.iter().enumerate() {
        let line_num = line_num + 1;

        // Classes and objects - handle multiline declarations
        if let Some(caps) = class_start_re.captures(line) {
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
            let is_object = line.contains("object ");
            let kind = if is_object { SymbolKind::Object } else { SymbolKind::Class };

            // Collect full declaration (may span multiple lines)
            let full_decl = collect_class_declaration(&lines, line_num - 1);
            let parents = extract_parents_from_declaration(&full_decl);

            symbols.push(ParsedSymbol {
                name,
                kind,
                line: line_num,
                signature: line.trim().to_string(),
                parents,
            });
        }

        // Interfaces
        if let Some(caps) = interface_re.captures(line) {
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
            let parents_str = caps.get(3).map(|m| m.as_str().trim());

            let mut parents = Vec::new();
            if let Some(ps) = parents_str {
                for parent in parse_parents(ps) {
                    let parent_name = parent.trim().split('<').next().unwrap_or("").trim();
                    if !parent_name.is_empty() {
                        parents.push((parent_name.to_string(), "extends".to_string()));
                    }
                }
            }

            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Interface,
                line: line_num,
                signature: line.trim().to_string(),
                parents,
            });
        }

        // Enums
        if let Some(caps) = enum_re.captures(line) {
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Enum,
                line: line_num,
                signature: line.trim().to_string(),
                parents: vec![],
            });
        }

        // Functions
        if let Some(caps) = fun_re.captures(line) {
            let name = caps.get(3).map(|m| m.as_str()).unwrap_or("").to_string();
            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Function,
                line: line_num,
                signature: line.trim().to_string(),
                parents: vec![],
            });
        }

        // Properties
        if let Some(caps) = property_re.captures(line) {
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() && name != "val" && name != "var" {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Property,
                    line: line_num,
                    signature: line.trim().to_string(),
                    parents: vec![],
                });
            }
        }

        // Type aliases
        if let Some(caps) = typealias_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::TypeAlias,
                line: line_num,
                signature: line.trim().to_string(),
                parents: vec![],
            });
        }

        // Java static fields (e.g., public static final String FOO = "bar";)
        if let Some(caps) = java_field_re.captures(line) {
            let name = caps.get(3).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Property,
                    line: line_num,
                    signature: line.trim().to_string(),
                    parents: vec![],
                });
            }
        }
    }

    Ok(symbols)
}

/// Collect a full class declaration that may span multiple lines
fn collect_class_declaration(lines: &[&str], start_idx: usize) -> String {
    let mut result = String::new();
    let mut paren_depth = 0;
    let mut found_opening_brace = false;

    for i in start_idx..lines.len().min(start_idx + 20) { // Max 20 lines
        let line = lines[i];
        result.push_str(line);
        result.push(' ');

        for c in line.chars() {
            match c {
                '(' => paren_depth += 1,
                ')' => paren_depth -= 1,
                '{' => {
                    found_opening_brace = true;
                    break;
                }
                _ => {}
            }
        }

        // Stop when we found the opening brace (end of declaration)
        if found_opening_brace {
            break;
        }

        // Also stop if parentheses are balanced and we see ':'
        if paren_depth == 0 && line.contains(':') && i > start_idx {
            // Check if next line starts the body
            if i + 1 < lines.len() && lines[i + 1].trim().starts_with('{') {
                break;
            }
        }
    }

    result
}

/// Extract parent classes/interfaces from a full class declaration
fn extract_parents_from_declaration(decl: &str) -> Vec<(String, String)> {
    let mut parents = Vec::new();

    // Find the inheritance clause after ')' followed by ':'
    // Pattern: ClassName(...) : Parent1, Parent2 {
    static INHERITANCE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\)\s*:\s*([^{]+)").unwrap());

    let inheritance_re = Some(&*INHERITANCE_RE);

    if let Some(re) = inheritance_re {
        if let Some(caps) = re.captures(decl) {
            if let Some(parents_str) = caps.get(1) {
                for parent in parse_parents(parents_str.as_str()) {
                    let inherit_kind = if parent.contains("()") {
                        "extends"
                    } else {
                        "implements"
                    };
                    let parent_name = parent
                        .trim()
                        .trim_end_matches("()")
                        .split('<')
                        .next()
                        .unwrap_or("")
                        .trim();
                    if !parent_name.is_empty() {
                        parents.push((parent_name.to_string(), inherit_kind.to_string()));
                    }
                }
            }
        }
    }

    // Also check for simple inheritance (class Name : Parent)
    if parents.is_empty() {
        static SIMPLE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:class|object)\s+\w+(?:\s*<[^>]*>)?\s*:\s*([^{(]+)").unwrap());

        let simple_re = Some(&*SIMPLE_RE);
        if let Some(re) = simple_re {
            if let Some(caps) = re.captures(decl) {
                if let Some(parents_str) = caps.get(1) {
                    for parent in parse_parents(parents_str.as_str()) {
                        let parent_name = parent
                            .trim()
                            .trim_end_matches("()")
                            .split('<')
                            .next()
                            .unwrap_or("")
                            .trim();
                        if !parent_name.is_empty() {
                            parents.push((parent_name.to_string(), "implements".to_string()));
                        }
                    }
                }
            }
        }
    }

    parents
}

/// Parse parent classes/interfaces from inheritance clause
pub fn parse_parents(parents_str: &str) -> Vec<&str> {
    // Split by comma, handling generics
    let mut result = Vec::new();
    let mut depth = 0;
    let mut start = 0;

    for (i, c) in parents_str.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                let parent = parents_str[start..i].trim();
                if !parent.is_empty() {
                    result.push(parent);
                }
                start = i + 1;
            }
            _ => {}
        }
    }

    // Add last parent
    let last = parents_str[start..].trim();
    if !last.is_empty() {
        result.push(last);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_class() {
        let content = "class MyService {\n}\n";
        let symbols = parse_kotlin_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "MyService").unwrap();
        assert_eq!(cls.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_data_class() {
        let content = "data class User(val name: String, val age: Int)\n";
        let symbols = parse_kotlin_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "User").unwrap();
        assert_eq!(cls.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_object() {
        let content = "object Singleton {\n}\n";
        let symbols = parse_kotlin_symbols(content).unwrap();
        let obj = symbols.iter().find(|s| s.name == "Singleton").unwrap();
        assert_eq!(obj.kind, SymbolKind::Object);
    }

    #[test]
    fn test_parse_class_with_inheritance() {
        let content = "class MyFragment(arg: String) : Fragment(), Serializable {\n}\n";
        let symbols = parse_kotlin_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "MyFragment").unwrap();
        assert_eq!(cls.kind, SymbolKind::Class);
        assert!(cls.parents.iter().any(|(p, k)| p == "Fragment" && k == "extends"));
        assert!(cls.parents.iter().any(|(p, k)| p == "Serializable" && k == "implements"));
    }

    #[test]
    fn test_parse_class_simple_inheritance() {
        let content = "class Child : Parent {\n}\n";
        let symbols = parse_kotlin_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "Child").unwrap();
        assert!(!cls.parents.is_empty(), "should have parents: {:?}", cls.parents);
    }

    #[test]
    fn test_parse_interface() {
        let content = "interface Repository {\n    fun getAll(): List<Item>\n}\n";
        let symbols = parse_kotlin_symbols(content).unwrap();
        let iface = symbols.iter().find(|s| s.name == "Repository").unwrap();
        assert_eq!(iface.kind, SymbolKind::Interface);
    }

    #[test]
    fn test_parse_interface_with_parent() {
        let content = "interface UserRepository : BaseRepository<User> {\n}\n";
        let symbols = parse_kotlin_symbols(content).unwrap();
        let iface = symbols.iter().find(|s| s.name == "UserRepository").unwrap();
        assert_eq!(iface.kind, SymbolKind::Interface);
        assert!(iface.parents.iter().any(|(p, _)| p == "BaseRepository"));
    }

    #[test]
    fn test_parse_sealed_interface() {
        let content = "sealed interface Result {\n}\n";
        let symbols = parse_kotlin_symbols(content).unwrap();
        let iface = symbols.iter().find(|s| s.name == "Result").unwrap();
        assert_eq!(iface.kind, SymbolKind::Interface);
    }

    #[test]
    fn test_parse_enum_class() {
        let content = "enum class Direction {\n    NORTH, SOUTH, EAST, WEST\n}\n";
        let symbols = parse_kotlin_symbols(content).unwrap();
        let e = symbols.iter().find(|s| s.name == "Direction").unwrap();
        assert_eq!(e.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_function() {
        let content = "fun processPayment(amount: Double): Boolean {\n}\n";
        let symbols = parse_kotlin_symbols(content).unwrap();
        let f = symbols.iter().find(|s| s.name == "processPayment").unwrap();
        assert_eq!(f.kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_suspend_function() {
        let content = "    suspend fun fetchData(): Result<Data> {\n    }\n";
        let symbols = parse_kotlin_symbols(content).unwrap();
        let f = symbols.iter().find(|s| s.name == "fetchData").unwrap();
        assert_eq!(f.kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_extension_function() {
        let content = "fun String.toSlug(): String = this.lowercase().replace(\" \", \"-\")\n";
        let symbols = parse_kotlin_symbols(content).unwrap();
        let f = symbols.iter().find(|s| s.name == "toSlug").unwrap();
        assert_eq!(f.kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_property() {
        let content = "    val name: String = \"test\"\n    var count: Int = 0\n";
        let symbols = parse_kotlin_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "name" && s.kind == SymbolKind::Property));
        assert!(symbols.iter().any(|s| s.name == "count" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_typealias() {
        let content = "typealias StringMap = Map<String, String>\n";
        let symbols = parse_kotlin_symbols(content).unwrap();
        let ta = symbols.iter().find(|s| s.name == "StringMap").unwrap();
        assert_eq!(ta.kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn test_parse_java_static_field() {
        let content = "    public static final String TAG = \"MyClass\";\n";
        let symbols = parse_kotlin_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "TAG" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_multiline_class() {
        let content = r#"class AppModule(
    private val context: Context
) : Module(),
    Serializable {
}
"#;
        let symbols = parse_kotlin_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "AppModule" && s.kind == SymbolKind::Class).unwrap();
        assert!(cls.parents.iter().any(|(p, k)| p == "Module" && k == "extends"),
            "should have Module as extends, got: {:?}", cls.parents);
    }

    #[test]
    fn test_parse_parents_with_generics() {
        let parents = parse_parents("BaseAdapter<Item>, Serializable, Comparable<Item>");
        assert_eq!(parents.len(), 3);
        assert_eq!(parents[0], "BaseAdapter<Item>");
        assert_eq!(parents[1], "Serializable");
        assert_eq!(parents[2], "Comparable<Item>");
    }

    #[test]
    fn test_parse_abstract_class() {
        let content = "abstract class BaseViewModel : ViewModel() {\n}\n";
        let symbols = parse_kotlin_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "BaseViewModel").unwrap();
        assert_eq!(cls.kind, SymbolKind::Class);
    }
}
