use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

use crate::db::SymbolKind;
use super::ParsedSymbol;

/// Parse Rust source file and extract symbols
pub fn parse_rust_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();

    // Struct definition: struct Name { ... } or struct Name(...)
    static STRUCT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Z][A-Za-z0-9_]*)\s*(?:<[^>]*>)?"

    ).unwrap());

    let struct_re = &*STRUCT_RE;

    // Enum definition: enum Name { ... }
    static ENUM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?enum\s+([A-Z][A-Za-z0-9_]*)\s*(?:<[^>]*>)?"

    ).unwrap());

    let enum_re = &*ENUM_RE;

    // Trait definition: trait Name { ... }
    static TRAIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?(?:unsafe\s+)?trait\s+([A-Z][A-Za-z0-9_]*)\s*(?:<[^>]*>)?(?:\s*:\s*[A-Za-z0-9_+\s<>,]+)?"

    ).unwrap());

    let trait_re = &*TRAIT_RE;

    // Impl block: impl Trait for Type or impl Type
    static IMPL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:unsafe\s+)?impl\s*(?:<[^>]*>\s*)?([A-Z][A-Za-z0-9_<>,\s]*?)\s+for\s+([A-Z][A-Za-z0-9_<>,\s]*)"

    ).unwrap());

    let impl_re = &*IMPL_RE;

    // Self impl block: impl Type { ... }
    static IMPL_SELF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*impl\s*(?:<[^>]*>\s*)?([A-Z][A-Za-z0-9_<>,\s]*)\s*\{"

    ).unwrap());

    let impl_self_re = &*IMPL_SELF_RE;

    // Function: fn name(...) or pub fn name(...)
    static FUNC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+([a-z_][a-z0-9_]*)\s*(?:<[^>]*>)?\s*\("

    ).unwrap());

    let func_re = &*FUNC_RE;

    // Macro definition: macro_rules! name { ... }
    static MACRO_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:#\[macro_export\]\s*)?macro_rules!\s+([a-z_][a-z0-9_]*)"

    ).unwrap());

    let macro_re = &*MACRO_RE;

    // Type alias: type Name = ...
    static TYPE_ALIAS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?type\s+([A-Z][A-Za-z0-9_]*)\s*(?:<[^>]*>)?\s*="

    ).unwrap());

    let type_alias_re = &*TYPE_ALIAS_RE;

    // Constant: const NAME: Type = ...
    static CONST_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?const\s+([A-Z][A-Z0-9_]+)\s*:"

    ).unwrap());

    let const_re = &*CONST_RE;

    // Static: static NAME: Type = ...
    static STATIC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?static\s+(?:mut\s+)?([A-Z][A-Z0-9_]+)\s*:"

    ).unwrap());

    let static_re = &*STATIC_RE;

    // Module: mod name;
    static MOD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?mod\s+([a-z_][a-z0-9_]*)"

    ).unwrap());

    let mod_re = &*MOD_RE;

    // Use statement: use path::to::item;
    static USE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?use\s+([a-zA-Z_][a-zA-Z0-9_:]*)"

    ).unwrap());

    let use_re = &*USE_RE;

    // Derive attribute: #[derive(Trait1, Trait2)]
    static DERIVE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*#\[derive\(([^)]+)\)\]"

    ).unwrap());

    let derive_re = &*DERIVE_RE;

    // Other attributes: #[attribute]
    static ATTR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*#\[([a-z_][a-z0-9_]*)"

    ).unwrap());

    let attr_re = &*ATTR_RE;

    let lines: Vec<&str> = content.lines().collect();

    // Parse structs
    for cap in struct_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Class, // Struct -> Class for compatibility
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse enums
    for cap in enum_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Enum,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse traits
    for cap in trait_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Interface, // Trait -> Interface
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse impl blocks for traits
    for cap in impl_re.captures_iter(content) {
        let trait_name = cap.get(1).unwrap().as_str().trim();
        let type_name = cap.get(2).unwrap().as_str().trim();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Record as implementation relationship
        symbols.push(ParsedSymbol {
            name: format!("impl {} for {}", trait_name, type_name),
            kind: SymbolKind::Class,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![(trait_name.to_string(), "implements".to_string())],
        });
    }

    // Parse impl self blocks
    for cap in impl_self_re.captures_iter(content) {
        let full_match = cap.get(0).unwrap().as_str();
        // Skip if this is "impl Trait for Type" (already handled)
        if full_match.contains(" for ") {
            continue;
        }

        let type_name = cap.get(1).unwrap().as_str().trim();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: format!("impl {}", type_name),
            kind: SymbolKind::Class,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse functions
    for cap in func_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Function,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse macros
    for cap in macro_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: format!("{}!", name),
            kind: SymbolKind::Function, // Macro -> Function
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse type aliases
    for cap in type_alias_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::TypeAlias,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse constants
    for cap in const_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Constant,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse statics
    for cap in static_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Constant,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse modules
    for cap in mod_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Package,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse use statements
    for cap in use_re.captures_iter(content) {
        let path = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: path.to_string(),
            kind: SymbolKind::Import,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse derive attributes
    for cap in derive_re.captures_iter(content) {
        let derives = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        for derive in derives.split(',') {
            let derive_name = derive.trim();
            if !derive_name.is_empty() {
                symbols.push(ParsedSymbol {
                    name: format!("#[derive({})]", derive_name),
                    kind: SymbolKind::Annotation,
                    line,
                    signature: line_text.trim().to_string(),
                    parents: vec![],
                });
            }
        }
    }

    // Parse significant attributes
    for cap in attr_re.captures_iter(content) {
        let attr_name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Only track significant attributes
        if matches!(attr_name,
            "test" | "bench" | "cfg" | "allow" | "warn" | "deny" |
            "macro_export" | "inline" | "cold" | "must_use" |
            "tokio" | "async_trait" | "proc_macro" | "proc_macro_derive" |
            "serde" | "rocket" | "actix" | "axum"
        ) {
            symbols.push(ParsedSymbol {
                name: format!("#[{}]", attr_name),
                kind: SymbolKind::Annotation,
                line,
                signature: line_text.trim().to_string(),
                parents: vec![],
            });
        }
    }

    Ok(symbols)
}

fn find_line_number(content: &str, byte_offset: usize) -> usize {
    content[..byte_offset].matches('\n').count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_struct() {
        let content = r#"
pub struct User {
    pub id: u64,
    pub name: String,
}

struct PrivateData(i32);
"#;
        let symbols = parse_rust_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "User" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "PrivateData" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_enum() {
        let content = r#"
pub enum Status {
    Active,
    Inactive,
    Pending,
}

enum Color {
    Red,
    Green,
    Blue,
}
"#;
        let symbols = parse_rust_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Status" && s.kind == SymbolKind::Enum));
        assert!(symbols.iter().any(|s| s.name == "Color" && s.kind == SymbolKind::Enum));
    }

    #[test]
    fn test_parse_trait() {
        let content = r#"
pub trait Repository {
    fn find(&self, id: u64) -> Option<User>;
    fn save(&mut self, user: User);
}

trait Display: Debug {
    fn display(&self);
}
"#;
        let symbols = parse_rust_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Repository" && s.kind == SymbolKind::Interface));
        assert!(symbols.iter().any(|s| s.name == "Display" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_parse_impl() {
        let content = r#"
impl Repository for SqlUserRepository {
    fn find(&self, id: u64) -> Option<User> {
        None
    }
}

impl User {
    pub fn new(name: String) -> Self {
        Self { id: 0, name }
    }
}
"#;
        let symbols = parse_rust_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "impl Repository for SqlUserRepository"));
        assert!(symbols.iter().any(|s| s.name == "impl User"));
        assert!(symbols.iter().any(|s| s.name == "find" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "new" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_functions() {
        let content = r#"
pub fn process_data(data: &[u8]) -> Result<(), Error> {
    Ok(())
}

fn private_helper() {
    println!("helper");
}

pub async fn fetch_user(id: u64) -> User {
    todo!()
}

const fn compute() -> i32 {
    42
}
"#;
        let symbols = parse_rust_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "process_data"));
        assert!(symbols.iter().any(|s| s.name == "private_helper"));
        assert!(symbols.iter().any(|s| s.name == "fetch_user"));
        assert!(symbols.iter().any(|s| s.name == "compute"));
    }

    #[test]
    fn test_parse_macro() {
        let content = r#"
macro_rules! vec_of_strings {
    ($($x:expr),*) => (vec![$($x.to_string()),*]);
}

#[macro_export]
macro_rules! my_macro {
    () => {};
}
"#;
        let symbols = parse_rust_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "vec_of_strings!"));
        assert!(symbols.iter().any(|s| s.name == "my_macro!"));
    }

    #[test]
    fn test_parse_type_alias() {
        let content = r#"
pub type UserId = u64;
type Result<T> = std::result::Result<T, Error>;
"#;
        let symbols = parse_rust_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "UserId" && s.kind == SymbolKind::TypeAlias));
        assert!(symbols.iter().any(|s| s.name == "Result" && s.kind == SymbolKind::TypeAlias));
    }

    #[test]
    fn test_parse_const_static() {
        let content = r#"
pub const MAX_SIZE: usize = 1024;
const DEFAULT_VALUE: i32 = 0;
static GLOBAL_COUNTER: AtomicUsize = AtomicUsize::new(0);
static mut MUTABLE_GLOBAL: i32 = 0;
"#;
        let symbols = parse_rust_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "MAX_SIZE" && s.kind == SymbolKind::Constant));
        assert!(symbols.iter().any(|s| s.name == "DEFAULT_VALUE" && s.kind == SymbolKind::Constant));
        assert!(symbols.iter().any(|s| s.name == "GLOBAL_COUNTER" && s.kind == SymbolKind::Constant));
        assert!(symbols.iter().any(|s| s.name == "MUTABLE_GLOBAL" && s.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_parse_modules() {
        let content = r#"
mod tests;
pub mod utils;
mod inner {
    fn helper() {}
}
"#;
        let symbols = parse_rust_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "tests" && s.kind == SymbolKind::Package));
        assert!(symbols.iter().any(|s| s.name == "utils" && s.kind == SymbolKind::Package));
        assert!(symbols.iter().any(|s| s.name == "inner" && s.kind == SymbolKind::Package));
    }

    #[test]
    fn test_parse_derive() {
        let content = r#"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub name: String,
}
"#;
        let symbols = parse_rust_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "#[derive(Debug)]"));
        assert!(symbols.iter().any(|s| s.name == "#[derive(Clone)]"));
        assert!(symbols.iter().any(|s| s.name == "#[derive(Serialize)]"));
    }
}
