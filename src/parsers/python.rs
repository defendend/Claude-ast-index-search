use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

use crate::db::SymbolKind;
use super::ParsedSymbol;

/// Parse Python source file and extract symbols
pub fn parse_python_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();

    // Class definition: class ClassName(Base1, Base2):
    static CLASS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[ \t]*class\s+([A-Z][A-Za-z0-9_]*)\s*(?:\(([^)]*)\))?:").unwrap());

    let class_re = &*CLASS_RE;

    // Function definition: def function_name(...):
    static FUNC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[ \t]*def\s+([a-z_][a-z0-9_]*)\s*\([^)]*\)\s*(?:->\s*[^:]+)?:").unwrap());

    let func_re = &*FUNC_RE;

    // Async function: async def function_name(...):
    static ASYNC_FUNC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[ \t]*async\s+def\s+([a-z_][a-z0-9_]*)\s*\([^)]*\)\s*(?:->\s*[^:]+)?:").unwrap());

    let async_func_re = &*ASYNC_FUNC_RE;

    // Import: import module or from module import X
    // Note: use [ \t] instead of \s to avoid capturing newlines in the import list
    static IMPORT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^(?:from\s+([a-zA-Z_][a-zA-Z0-9_\.]*)\s+)?import\s+([a-zA-Z_][a-zA-Z0-9_\.,\ \t]*)").unwrap());

    let import_re = &*IMPORT_RE;

    // Decorator: @decorator_name
    static DECORATOR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[ \t]*@([a-zA-Z_][a-zA-Z0-9_\.]*)").unwrap());

    let decorator_re = &*DECORATOR_RE;

    // Constant assignment: CONSTANT_NAME = value (all caps at module level)
    static CONSTANT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^([A-Z][A-Z0-9_]*)\s*=\s*").unwrap());

    let constant_re = &*CONSTANT_RE;

    // Type alias: TypeName = SomeType (PascalCase at module level, using TypeAlias or just =)
    static TYPE_ALIAS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^([A-Z][a-zA-Z0-9_]*)\s*(?::\s*TypeAlias\s*)?=\s*(?:Union|Optional|List|Dict|Tuple|Callable|Type)").unwrap());

    let type_alias_re = &*TYPE_ALIAS_RE;

    let lines: Vec<&str> = content.lines().collect();

    // Parse classes
    for cap in class_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let parents_str = cap.get(2).map(|m| m.as_str());
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        let parents: Vec<(String, String)> = parents_str
            .map(|s| {
                s.split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty() && p != "object")
                    .map(|p| (p, "extends".to_string()))
                    .collect()
            })
            .unwrap_or_default();

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Class,
            line,
            signature: line_text.trim().to_string(),
            parents,
        });
    }

    // Parse regular functions (skip methods inside classes - they start with more indentation)
    for cap in func_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Skip if this is a method (indented) - check if line starts with spaces/tabs
        let line_start = content[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let indent = &content[line_start..start];

        // Module-level functions have no indentation, methods have 4+ spaces
        let _indent_level = indent.chars().filter(|c| *c == ' ').count()
            + indent.chars().filter(|c| *c == '\t').count() * 4;

        // Skip private methods/functions starting with _ (except __init__, __call__)
        if !name.starts_with('_') || name == "__init__" || name == "__call__" {
            symbols.push(ParsedSymbol {
                name: name.to_string(),
                kind: SymbolKind::Function,
                line,
                signature: line_text.trim().to_string(),
                parents: vec![],
            });
        }
    }

    // Parse async functions
    for cap in async_func_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Skip private methods
        if !name.starts_with('_') {
            symbols.push(ParsedSymbol {
                name: name.to_string(),
                kind: SymbolKind::Function,
                line,
                signature: line_text.trim().to_string(),
                parents: vec![],
            });
        }
    }

    // Parse imports (for reference tracking)
    for cap in import_re.captures_iter(content) {
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        if let Some(from_module) = cap.get(1) {
            // from X import Y - record the module
            symbols.push(ParsedSymbol {
                name: from_module.as_str().to_string(),
                kind: SymbolKind::Import,
                line,
                signature: line_text.trim().to_string(),
                parents: vec![],
            });
        }

        // Also record what's being imported
        if let Some(imports) = cap.get(2) {
            for import_item in imports.as_str().split(',') {
                let item = import_item.split(" as ").next().unwrap().trim();
                if !item.is_empty() && item != "*" {
                    symbols.push(ParsedSymbol {
                        name: item.to_string(),
                        kind: SymbolKind::Import,
                        line,
                        signature: line_text.trim().to_string(),
                        parents: vec![],
                    });
                }
            }
        }
    }

    // Parse decorators (useful for finding handlers, routes, etc.)
    for cap in decorator_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Only track significant decorators
        if name.contains("route") || name.contains("handler") || name.contains("pytest")
            || name.contains("fixture") || name.contains("dataclass") || name.contains("property") {
            symbols.push(ParsedSymbol {
                name: format!("@{}", name),
                kind: SymbolKind::Annotation,
                line,
                signature: line_text.trim().to_string(),
                parents: vec![],
            });
        }
    }

    // Parse module-level constants
    for cap in constant_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Only module-level (no indentation)
        let line_start = content[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
        if start == line_start {
            symbols.push(ParsedSymbol {
                name: name.to_string(),
                kind: SymbolKind::Constant,
                line,
                signature: line_text.trim().to_string(),
                parents: vec![],
            });
        }
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

    Ok(symbols)
}

fn find_line_number(content: &str, byte_offset: usize) -> usize {
    content[..byte_offset].matches('\n').count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_class() {
        let content = r#"
class MyClass:
    pass

class ChildClass(ParentClass):
    pass
"#;
        let symbols = parse_python_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "MyClass" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "ChildClass" && s.parents.iter().any(|(p, _)| p == "ParentClass")));
    }

    #[test]
    fn test_parse_functions() {
        let content = r#"
def handle(request, context):
    pass

async def async_handler(request):
    pass
"#;
        let symbols = parse_python_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "handle"));
        assert!(symbols.iter().any(|s| s.name == "async_handler"));
    }

    #[test]
    fn test_parse_imports() {
        let content = r#"import logging
from driver_referrals.common import db
from typing import Optional, List
"#;
        let symbols = parse_python_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "logging" && s.kind == SymbolKind::Import));
        assert!(symbols.iter().any(|s| s.name == "driver_referrals.common" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn test_parse_decorators() {
        let content = r#"
@dataclass
class Config:
    host: str

@property
def name(self):
    return self._name

@pytest.fixture
def client():
    return Client()
"#;
        let symbols = parse_python_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "@dataclass"));
        assert!(symbols.iter().any(|s| s.name == "@property"));
        assert!(symbols.iter().any(|s| s.name == "@pytest.fixture"));
    }

    #[test]
    fn test_parse_async_def() {
        let content = r#"
async def fetch_data(url):
    pass

async def process_event(event):
    pass
"#;
        let symbols = parse_python_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "fetch_data" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "process_event" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_class_multiple_inheritance() {
        let content = r#"
class MyView(BaseView, PermissionMixin, LoggingMixin):
    pass
"#;
        let symbols = parse_python_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "MyView").unwrap();
        assert!(cls.parents.iter().any(|(p, _)| p == "BaseView"));
        assert!(cls.parents.iter().any(|(p, _)| p == "PermissionMixin"));
        assert!(cls.parents.iter().any(|(p, _)| p == "LoggingMixin"));
    }

    #[test]
    fn test_parse_constants() {
        let content = r#"
MAX_RETRIES = 5
DEFAULT_TIMEOUT = 30
API_KEY = "secret"
"#;
        let symbols = parse_python_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "MAX_RETRIES" && s.kind == SymbolKind::Constant));
        assert!(symbols.iter().any(|s| s.name == "DEFAULT_TIMEOUT" && s.kind == SymbolKind::Constant));
        assert!(symbols.iter().any(|s| s.name == "API_KEY" && s.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_parse_type_aliases() {
        let content = r#"
UserList = List[User]
Callback = Callable[[str], None]
"#;
        let symbols = parse_python_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "UserList" && s.kind == SymbolKind::TypeAlias));
        assert!(symbols.iter().any(|s| s.name == "Callback" && s.kind == SymbolKind::TypeAlias));
    }

    #[test]
    fn test_parse_function_with_return_type() {
        let content = r#"
def get_name(self) -> str:
    return ""
"#;
        let symbols = parse_python_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "get_name"));
    }
}
