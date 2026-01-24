//! TypeScript/JavaScript parser for symbol extraction
//!
//! Supports:
//! - TypeScript (.ts, .tsx)
//! - JavaScript (.js, .jsx, .mjs, .cjs)
//! - Vue SFC (.vue)
//! - Svelte (.svelte)
//!
//! Indexed constructs:
//! - Classes, interfaces, type aliases, enums
//! - Functions (regular, arrow, async)
//! - React components and hooks
//! - Vue/Svelte components
//! - Decorators (@Controller, @Injectable, etc.)
//! - Module-level constants and exports

use anyhow::Result;
use regex::Regex;

use crate::db::SymbolKind;
use super::ParsedSymbol;

/// Parse TypeScript/JavaScript source file and extract symbols
pub fn parse_typescript_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();
    let lines: Vec<&str> = content.lines().collect();

    // Class definition: class ClassName extends/implements ...
    let class_re = Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?(?:abstract\s+)?class\s+([A-Z][A-Za-z0-9_]*)\s*(?:<[^>]*>)?\s*(?:extends\s+([A-Z][A-Za-z0-9_.<>,\s]*))?(?:\s+implements\s+([A-Z][A-Za-z0-9_.<>,\s]*))?"
    )?;

    // Interface definition: interface InterfaceName extends ...
    let interface_re = Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?interface\s+([A-Z][A-Za-z0-9_]*)\s*(?:<[^>]*>)?\s*(?:extends\s+([A-Z][A-Za-z0-9_.<>,\s]*))?"
    )?;

    // Type alias: type TypeName = ...
    let type_alias_re = Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?type\s+([A-Z][A-Za-z0-9_]*)\s*(?:<[^>]*>)?\s*="
    )?;

    // Enum: enum EnumName { ... }
    let enum_re = Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?(?:const\s+)?enum\s+([A-Z][A-Za-z0-9_]*)"
    )?;

    // Regular function: function functionName(...) or export function
    let func_re = Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?(?:async\s+)?function\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*(?:<[^>]*>)?\s*\("
    )?;

    // Arrow function as const: const functionName = (...) => or const functionName = async (...) =>
    let arrow_func_re = Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?(?:const|let)\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*(?::\s*[^=]+)?\s*=\s*(?:async\s+)?\([^)]*\)\s*(?::\s*[^=]+)?\s*=>"
    )?;

    // Arrow function without parens: const fn = x =>
    let arrow_func_simple_re = Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?(?:const|let)\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*=\s*(?:async\s+)?[a-zA-Z_][a-zA-Z0-9_]*\s*=>"
    )?;

    // React functional component as arrow function: const ComponentName = (props) => {
    let react_arrow_component_re = Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?const\s+([A-Z][A-Za-z0-9_]*)\s*(?::\s*(?:React\.)?FC[^=]*)?\s*=\s*(?:\([^)]*\)|[a-zA-Z_][a-zA-Z0-9_]*)\s*(?::\s*[^=]+)?\s*=>"
    )?;

    // React functional component as function: function ComponentName(props) {
    let react_func_component_re = Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?function\s+([A-Z][A-Za-z0-9_]*)\s*\("
    )?;

    // React hooks: const [state, setState] = useState(...) or custom hooks: function useXxx()
    let hook_re = Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?(?:const|function)\s+(use[A-Z][a-zA-Z0-9_]*)"
    )?;

    // Decorator: @DecoratorName or @DecoratorName(...)
    let decorator_re = Regex::new(
        r"(?m)^[ \t]*@([A-Z][a-zA-Z0-9_]*)\s*(?:\([^)]*\))?"
    )?;

    // Import: import { X } from 'module' or import X from 'module'
    let import_re = Regex::new(
        r#"(?m)^[ \t]*import\s+(?:\{[^}]*\}|\*\s+as\s+[a-zA-Z_][a-zA-Z0-9_]*|[a-zA-Z_][a-zA-Z0-9_]*)\s+from\s+['"]([^'"]+)['"]"#
    )?;

    // Module-level const (UPPER_CASE): const API_URL = ...
    let const_re = Regex::new(
        r"(?m)^(?:export\s+)?const\s+([A-Z][A-Z0-9_]+)\s*(?::\s*[^=]+)?\s*="
    )?;

    // Namespace: namespace NamespaceName { ... }
    let namespace_re = Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?(?:declare\s+)?namespace\s+([A-Z][A-Za-z0-9_]*)"
    )?;

    // Vue defineComponent: export default defineComponent({ name: 'ComponentName' })
    // Simplified: look for defineComponent
    let vue_component_re = Regex::new(
        r#"(?m)defineComponent\s*\(\s*\{[^}]*name\s*:\s*['"]([A-Z][A-Za-z0-9_]*)['"]"#
    )?;

    // Vue setup script: <script setup> ... </script>
    // We'll extract the file name as component name if it's a .vue file

    // Svelte: export let propName (props)
    let svelte_prop_re = Regex::new(
        r"(?m)^[ \t]*export\s+let\s+([a-zA-Z_][a-zA-Z0-9_]*)"
    )?;

    // Parse classes
    for cap in class_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let extends = cap.get(2).map(|m| m.as_str().trim().to_string());
        let implements = cap.get(3).map(|m| m.as_str().trim().to_string());
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        let mut parents: Vec<(String, String)> = Vec::new();
        if let Some(ext) = extends {
            // Handle multiple extends separated by comma (rare in TS but possible with mixins)
            for parent in ext.split(',') {
                let parent = parent.trim().split('<').next().unwrap_or("").trim();
                if !parent.is_empty() {
                    parents.push((parent.to_string(), "extends".to_string()));
                }
            }
        }
        if let Some(impl_list) = implements {
            for iface in impl_list.split(',') {
                let iface = iface.trim().split('<').next().unwrap_or("").trim();
                if !iface.is_empty() {
                    parents.push((iface.to_string(), "implements".to_string()));
                }
            }
        }

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Class,
            line,
            signature: line_text.trim().to_string(),
            parents,
        });
    }

    // Parse interfaces
    for cap in interface_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let extends = cap.get(2).map(|m| m.as_str().trim().to_string());
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        let mut parents: Vec<(String, String)> = Vec::new();
        if let Some(ext) = extends {
            for parent in ext.split(',') {
                let parent = parent.trim().split('<').next().unwrap_or("").trim();
                if !parent.is_empty() {
                    parents.push((parent.to_string(), "extends".to_string()));
                }
            }
        }

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Interface,
            line,
            signature: line_text.trim().to_string(),
            parents,
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

    // Parse regular functions
    for cap in func_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Skip if already captured as hook
        if name.starts_with("use") && name.len() > 3 && name.chars().nth(3).unwrap().is_uppercase() {
            continue;
        }

        // Skip PascalCase functions - they are React components (handled separately)
        if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
            continue;
        }

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Function,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse arrow functions
    let mut arrow_func_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    for cap in arrow_func_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Skip hooks (handled separately)
        if name.starts_with("use") && name.len() > 3 && name.chars().nth(3).map(|c| c.is_uppercase()).unwrap_or(false) {
            continue;
        }

        // Skip React components (PascalCase) - handled separately
        if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
            continue;
        }

        if arrow_func_names.insert(name.to_string()) {
            symbols.push(ParsedSymbol {
                name: name.to_string(),
                kind: SymbolKind::Function,
                line,
                signature: line_text.trim().to_string(),
                parents: vec![],
            });
        }
    }

    // Parse simple arrow functions
    for cap in arrow_func_simple_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        if name.chars().next().map(|c| c.is_lowercase()).unwrap_or(false) {
            if arrow_func_names.insert(name.to_string()) {
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    line,
                    signature: line_text.trim().to_string(),
                    parents: vec![],
                });
            }
        }
    }

    // Parse React arrow components (const ComponentName = () => {})
    for cap in react_arrow_component_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Skip if it's a class (already handled)
        if line_text.contains("class ") {
            continue;
        }

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Class, // React components as Class for consistency
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse React function components (function ComponentName() {})
    for cap in react_func_component_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Skip if it's a class (already handled)
        if line_text.contains("class ") {
            continue;
        }

        // Skip if it's a type alias or interface
        if line_text.contains("type ") || line_text.contains("interface ") {
            continue;
        }

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Class, // React components as Class for consistency
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse React hooks (useXxx pattern)
    for cap in hook_re.captures_iter(content) {
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

    // Parse decorators
    for cap in decorator_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Only track significant decorators
        let significant = ["Controller", "Get", "Post", "Put", "Delete", "Patch",
                          "Injectable", "Module", "Component", "Service", "Pipe",
                          "Guard", "Interceptor", "Middleware", "Entity", "Column",
                          "PrimaryColumn", "PrimaryGeneratedColumn", "ManyToOne",
                          "OneToMany", "ManyToMany", "OneToOne", "JoinColumn",
                          "ViewChild", "ViewChildren", "Input", "Output", "Inject"];

        if significant.iter().any(|s| name.contains(s)) {
            symbols.push(ParsedSymbol {
                name: format!("@{}", name),
                kind: SymbolKind::Annotation,
                line,
                signature: line_text.trim().to_string(),
                parents: vec![],
            });
        }
    }

    // Parse imports
    for cap in import_re.captures_iter(content) {
        let module = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Skip node_modules imports for cleaner index
        if !module.starts_with('.') && !module.starts_with("@/") && !module.starts_with('~') {
            continue;
        }

        symbols.push(ParsedSymbol {
            name: module.to_string(),
            kind: SymbolKind::Import,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse module-level constants (UPPER_CASE)
    for cap in const_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Only module-level (minimal indentation)
        let line_start = content[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let indent = &content[line_start..start];
        let indent_level = indent.chars().filter(|c| *c == ' ').count()
            + indent.chars().filter(|c| *c == '\t').count() * 4;

        if indent_level <= 2 {
            symbols.push(ParsedSymbol {
                name: name.to_string(),
                kind: SymbolKind::Constant,
                line,
                signature: line_text.trim().to_string(),
                parents: vec![],
            });
        }
    }

    // Parse namespaces
    for cap in namespace_re.captures_iter(content) {
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

    // Parse Vue components
    for cap in vue_component_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Class,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse Svelte props (export let)
    for cap in svelte_prop_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Property,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Deduplicate symbols by name+line (some patterns may overlap)
    let mut seen: std::collections::HashSet<(String, usize)> = std::collections::HashSet::new();
    symbols.retain(|s| seen.insert((s.name.clone(), s.line)));

    Ok(symbols)
}

/// Extract script content from Vue SFC
pub fn extract_vue_script(content: &str) -> String {
    let script_re = Regex::new(r"(?s)<script[^>]*>(.*?)</script>").unwrap();

    script_re.captures_iter(content)
        .filter_map(|cap| cap.get(1))
        .map(|m| m.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract script content from Svelte component
pub fn extract_svelte_script(content: &str) -> String {
    let script_re = Regex::new(r"(?s)<script[^>]*>(.*?)</script>").unwrap();

    script_re.captures_iter(content)
        .filter_map(|cap| cap.get(1))
        .map(|m| m.as_str())
        .collect::<Vec<_>>()
        .join("\n")
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
export class UserService extends BaseService implements IUserService {
    constructor() {}
}

class ChildClass extends ParentClass {
}
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "UserService" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "ChildClass" && s.parents.iter().any(|(p, _)| p == "ParentClass")));
    }

    #[test]
    fn test_parse_interface() {
        let content = r#"
interface User {
    id: string;
    name: string;
}

export interface IUserService extends IService {
    getUser(id: string): User;
}
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "User" && s.kind == SymbolKind::Interface));
        assert!(symbols.iter().any(|s| s.name == "IUserService" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_parse_type_alias() {
        let content = r#"
type UserId = string;
export type UserMap = Map<string, User>;
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "UserId" && s.kind == SymbolKind::TypeAlias));
        assert!(symbols.iter().any(|s| s.name == "UserMap" && s.kind == SymbolKind::TypeAlias));
    }

    #[test]
    fn test_parse_enum() {
        let content = r#"
enum Status {
    Active,
    Inactive,
}

export const enum Direction {
    Up,
    Down,
}
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Status" && s.kind == SymbolKind::Enum));
        assert!(symbols.iter().any(|s| s.name == "Direction" && s.kind == SymbolKind::Enum));
    }

    #[test]
    fn test_parse_functions() {
        let content = r#"
function handleRequest(req: Request): Response {
    return new Response();
}

export async function fetchUser(id: string): Promise<User> {
    return fetch(`/users/${id}`);
}

const processData = (data: Data) => {
    return data;
};

const asyncHandler = async (event) => {
    return event;
};
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "handleRequest" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "fetchUser" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "processData" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "asyncHandler" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_react_hooks() {
        let content = r#"
function useAuth() {
    const [user, setUser] = useState(null);
    return { user };
}

export const useCounter = () => {
    const [count, setCount] = useState(0);
    return { count, setCount };
};
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "useAuth" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "useCounter" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_react_component() {
        let content = r#"
const Button: React.FC<ButtonProps> = ({ children, onClick }) => {
    return <button onClick={onClick}>{children}</button>;
};

export function UserCard({ user }: UserCardProps) {
    return <div>{user.name}</div>;
}
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Button" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "UserCard" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_decorators() {
        let content = r#"
@Controller('users')
export class UserController {
    @Get(':id')
    getUser(@Param('id') id: string) {}

    @Post()
    createUser(@Body() dto: CreateUserDto) {}
}
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "@Controller" && s.kind == SymbolKind::Annotation));
        assert!(symbols.iter().any(|s| s.name == "@Get" && s.kind == SymbolKind::Annotation));
        assert!(symbols.iter().any(|s| s.name == "@Post" && s.kind == SymbolKind::Annotation));
    }

    #[test]
    fn test_parse_namespace() {
        let content = r#"
namespace Utils {
    export function helper() {}
}

export namespace Types {
    export interface User {}
}
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Utils" && s.kind == SymbolKind::Package));
        assert!(symbols.iter().any(|s| s.name == "Types" && s.kind == SymbolKind::Package));
    }

    #[test]
    fn test_parse_constants() {
        let content = r#"
const API_URL = 'https://api.example.com';
export const MAX_RETRIES = 3;
const INTERNAL_TIMEOUT = 5000;
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "API_URL" && s.kind == SymbolKind::Constant));
        assert!(symbols.iter().any(|s| s.name == "MAX_RETRIES" && s.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_extract_vue_script() {
        let content = r#"
<template>
  <div>{{ message }}</div>
</template>

<script setup lang="ts">
import { ref } from 'vue';
const message = ref('Hello');
</script>

<style>
div { color: red; }
</style>
"#;
        let script = extract_vue_script(content);
        assert!(script.contains("import { ref } from 'vue'"));
        assert!(script.contains("const message = ref"));
    }
}
