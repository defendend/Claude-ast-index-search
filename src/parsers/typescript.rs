//! TypeScript/JavaScript parser for symbol extraction
//!
//! Supports:
//! - TypeScript (.ts, .tsx, .mts)
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
use std::sync::LazyLock;

use super::ParsedSymbol;
use crate::db::SymbolKind;

/// Parse TypeScript/JavaScript source file and extract symbols
pub fn parse_typescript_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();
    let lines: Vec<&str> = content.lines().collect();

    // Class definition: class ClassName extends/implements ...
    static CLASS_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?(?:declare\s+)?(?:abstract\s+)?class\s+([A-Z][A-Za-z0-9_]*)\s*(?:<[^>]*>)?\s*(?:extends\s+([A-Z][A-Za-z0-9_.<>,\s]*))?(?:\s+implements\s+([A-Z][A-Za-z0-9_.<>,\s]*))?"
    ).unwrap()
    });
    let class_re = &*CLASS_RE;

    // Interface definition: interface InterfaceName extends ... or declare interface
    static INTERFACE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?(?:declare\s+)?interface\s+([A-Z][A-Za-z0-9_]*)\s*(?:<[^>]*>)?\s*(?:extends\s+([A-Z][A-Za-z0-9_.<>,\s]*))?"
    ).unwrap()
    });
    let interface_re = &*INTERFACE_RE;

    // Type alias: type TypeName = ... or declare type TypeName = ...
    static TYPE_ALIAS_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?(?:declare\s+)?type\s+([A-Z][A-Za-z0-9_]*)\s*(?:<[^>]*>)?\s*="
    ).unwrap()
    });
    let type_alias_re = &*TYPE_ALIAS_RE;

    // Enum: enum EnumName { ... }
    static ENUM_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?m)^[ \t]*(?:export\s+)?(?:declare\s+)?(?:const\s+)?enum\s+([A-Z][A-Za-z0-9_]*)",
        )
        .unwrap()
    });
    let enum_re = &*ENUM_RE;

    // Regular function: function functionName(...) or export function or export declare function
    static FUNC_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?(?:declare\s+)?(?:async\s+)?function\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*(?:<[^>]*>)?\s*\("
    ).unwrap()
    });
    let func_re = &*FUNC_RE;

    // Arrow function as const: const functionName = (...) => or const functionName = async (...) =>
    static ARROW_FUNC_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?(?:const|let)\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*(?::\s*[^=]+)?\s*=\s*(?:async\s+)?\([^)]*\)\s*(?::\s*[^=]+)?\s*=>"
    ).unwrap()
    });
    let arrow_func_re = &*ARROW_FUNC_RE;

    // Arrow function without parens: const fn = x =>
    static ARROW_FUNC_SIMPLE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?(?:const|let)\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*=\s*(?:async\s+)?[a-zA-Z_][a-zA-Z0-9_]*\s*=>"
    ).unwrap()
    });
    let arrow_func_simple_re = &*ARROW_FUNC_SIMPLE_RE;

    // React functional component as arrow function: const ComponentName = (props) => {
    static REACT_ARROW_COMPONENT_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?const\s+([A-Z][A-Za-z0-9_]*)\s*(?::\s*(?:React\.)?FC[^=]*)?\s*=\s*(?:\([^)]*\)|[a-zA-Z_][a-zA-Z0-9_]*)\s*(?::\s*[^=]+)?\s*=>"
    ).unwrap()
    });
    let react_arrow_component_re = &*REACT_ARROW_COMPONENT_RE;

    // React functional component as function: function ComponentName(props) {
    static REACT_FUNC_COMPONENT_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^[ \t]*(?:export\s+)?function\s+([A-Z][A-Za-z0-9_]*)\s*\(").unwrap()
    });
    let react_func_component_re = &*REACT_FUNC_COMPONENT_RE;

    // React hooks: const [state, setState] = useState(...) or custom hooks: function useXxx()
    // Also matches: export declare function useXxx()
    static HOOK_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?m)^[ \t]*(?:export\s+)?(?:declare\s+)?(?:const|function)\s+(use[A-Z][a-zA-Z0-9_]*)",
        )
        .unwrap()
    });
    let hook_re = &*HOOK_RE;

    // Decorator: @DecoratorName or @DecoratorName(...)
    static DECORATOR_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^[ \t]*@([A-Z][a-zA-Z0-9_]*)\s*(?:\([^)]*\))?").unwrap());
    let decorator_re = &*DECORATOR_RE;

    // Import: import { X } from 'module' or import X from 'module'
    static IMPORT_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r#"(?m)^[ \t]*import\s+(?:\{[^}]*\}|\*\s+as\s+[a-zA-Z_][a-zA-Z0-9_]*|[a-zA-Z_][a-zA-Z0-9_]*)\s+from\s+['"]([^'"]+)['"]"#
    ).unwrap()
    });
    let import_re = &*IMPORT_RE;

    // Module-level const (UPPER_CASE): const API_URL = ... or declare const
    static CONST_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?m)^(?:export\s+)?(?:declare\s+)?const\s+([A-Z][A-Z0-9_]+)\s*(?::\s*[^=]+)?\s*=",
        )
        .unwrap()
    });
    let const_re = &*CONST_RE;

    // Namespace: namespace NamespaceName { ... }
    static NAMESPACE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^[ \t]*(?:export\s+)?(?:declare\s+)?namespace\s+([A-Z][A-Za-z0-9_]*)")
            .unwrap()
    });
    let namespace_re = &*NAMESPACE_RE;

    // Vue Composition API reactive variables: const x = ref(), computed(), reactive(), etc.
    static REACTIVE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?m)^[ \t]*(?:export\s+)?(?:const|let)\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*=\s*(?:ref|reactive|computed|readonly|shallowRef|shallowReactive|toRef|toRefs)\s*(?:<[^>]*>)?\s*\("
    ).unwrap()
    });
    let reactive_re = &*REACTIVE_RE;

    // Vue lifecycle hooks and watchers: watch(), watchEffect(), onMounted(), etc.
    static VUE_LIFECYCLE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?m)^[ \t]*(watch|watchEffect|onMounted|onUnmounted|onBeforeMount|onBeforeUnmount|onUpdated|onBeforeUpdate|onActivated|onDeactivated|onErrorCaptured)\s*\("
    ).unwrap()
    });
    let vue_lifecycle_re = &*VUE_LIFECYCLE_RE;

    // export default { ... } (object literal)
    static EXPORT_DEFAULT_OBJ_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^export\s+default\s+\{").unwrap());
    let export_default_obj_re = &*EXPORT_DEFAULT_OBJ_RE;

    // export default someFunction(...) or export default defineComponent(...)
    static EXPORT_DEFAULT_CALL_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^export\s+default\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*\(").unwrap()
    });
    let export_default_call_re = &*EXPORT_DEFAULT_CALL_RE;

    // export default identifier; (re-export of a variable/const)
    static EXPORT_DEFAULT_IDENT_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^export\s+default\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*;?\s*$").unwrap()
    });
    let export_default_ident_re = &*EXPORT_DEFAULT_IDENT_RE;

    // Vue macros: defineProps, defineEmits, defineModel, defineStore, defineExpose, withDefaults
    static DEFINE_MACRO_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?m)^[ \t]*(?:(?:export\s+)?(?:const|let)\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*=\s*)?(defineProps|defineEmits|defineModel|defineStore|defineExpose|withDefaults)\s*(?:<[^>]*>)?\s*\("
    ).unwrap()
    });
    let define_macro_re = &*DEFINE_MACRO_RE;

    // Vue defineComponent: export default defineComponent({ name: 'ComponentName' })
    static VUE_COMPONENT_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?m)defineComponent\s*\(\s*\{[^}]*name\s*:\s*['"]([A-Z][A-Za-z0-9_]*)['"]"#)
            .unwrap()
    });
    let vue_component_re = &*VUE_COMPONENT_RE;

    // Svelte: export let propName (props)
    static SVELTE_PROP_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^[ \t]*export\s+let\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap()
    });
    let svelte_prop_re = &*SVELTE_PROP_RE;

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
            end_line: None,
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
            end_line: None,
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
            end_line: None,
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
            end_line: None,
        });
    }

    // Parse regular functions
    for cap in func_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Skip if already captured as hook
        if name.starts_with("use") && name.len() > 3 && name.chars().nth(3).unwrap().is_uppercase()
        {
            continue;
        }

        // Skip PascalCase functions - they are React components (handled separately)
        if name
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
        {
            continue;
        }

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Function,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
            end_line: None,
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
        if name.starts_with("use")
            && name.len() > 3
            && name
                .chars()
                .nth(3)
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
        {
            continue;
        }

        // Skip React components (PascalCase) - handled separately
        if name
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
        {
            continue;
        }

        if arrow_func_names.insert(name.to_string()) {
            symbols.push(ParsedSymbol {
                name: name.to_string(),
                kind: SymbolKind::Function,
                line,
                signature: line_text.trim().to_string(),
                parents: vec![],
                end_line: None,
            });
        }
    }

    // Parse simple arrow functions
    for cap in arrow_func_simple_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        if name
            .chars()
            .next()
            .map(|c| c.is_lowercase())
            .unwrap_or(false)
        {
            if arrow_func_names.insert(name.to_string()) {
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    line,
                    signature: line_text.trim().to_string(),
                    parents: vec![],
                    end_line: None,
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
            end_line: None,
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
            end_line: None,
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
            end_line: None,
        });
    }

    // Parse decorators
    for cap in decorator_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Only track significant decorators
        let significant = [
            "Controller",
            "Get",
            "Post",
            "Put",
            "Delete",
            "Patch",
            "Injectable",
            "Module",
            "Component",
            "Service",
            "Pipe",
            "Guard",
            "Interceptor",
            "Middleware",
            "Entity",
            "Column",
            "PrimaryColumn",
            "PrimaryGeneratedColumn",
            "ManyToOne",
            "OneToMany",
            "ManyToMany",
            "OneToOne",
            "JoinColumn",
            "ViewChild",
            "ViewChildren",
            "Input",
            "Output",
            "Inject",
        ];

        if significant.iter().any(|s| name.contains(s)) {
            symbols.push(ParsedSymbol {
                name: format!("@{}", name),
                kind: SymbolKind::Annotation,
                line,
                signature: line_text.trim().to_string(),
                parents: vec![],
                end_line: None,
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
            end_line: None,
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
                end_line: None,
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
            end_line: None,
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
            end_line: None,
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
            end_line: None,
        });
    }

    // Parse export default { ... }
    for cap in export_default_obj_re.captures_iter(content) {
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: "default".to_string(),
            kind: SymbolKind::Object,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
            end_line: None,
        });
    }

    // Parse export default someFunction(...)
    for cap in export_default_call_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Skip if already caught as export default object
        if name == "default" {
            continue;
        }

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Function,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
            end_line: None,
        });
    }

    // Parse export default identifier;
    for cap in export_default_ident_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: format!("default({})", name),
            kind: SymbolKind::Object,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
            end_line: None,
        });
    }

    // Parse Vue macros (defineProps, defineEmits, defineStore, etc.)
    for cap in define_macro_re.captures_iter(content) {
        let variable = cap.get(1).map(|m| m.as_str());
        let macro_name = cap.get(2).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        let name = variable.unwrap_or(macro_name);

        // Skip if already caught by HOOK_RE (e.g. useAuthStore = defineStore)
        if name.starts_with("use")
            && name.len() > 3
            && name
                .chars()
                .nth(3)
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
        {
            continue;
        }

        let kind = match macro_name {
            "defineStore" => SymbolKind::Function,
            _ => SymbolKind::Property, // defineProps, defineEmits, defineModel, defineExpose, withDefaults
        };

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
            end_line: None,
        });
    }

    // Parse Vue Composition API reactive variables
    for cap in reactive_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Skip if already caught by ARROW_FUNC_RE or HOOK_RE
        if arrow_func_names.contains(name) {
            continue;
        }
        if name.starts_with("use")
            && name.len() > 3
            && name
                .chars()
                .nth(3)
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
        {
            continue;
        }

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Property,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
            end_line: None,
        });
    }

    // Parse Vue lifecycle hooks and watchers
    for cap in vue_lifecycle_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Annotation,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
            end_line: None,
        });
    }

    // Deduplicate symbols by name+line (some patterns may overlap)
    let mut seen: std::collections::HashSet<(String, usize)> = std::collections::HashSet::new();
    symbols.retain(|s| seen.insert((s.name.clone(), s.line)));

    Ok(symbols)
}

/// Extract script content from Vue SFC, preserving line numbers
pub fn extract_vue_script(content: &str) -> String {
    static SCRIPT_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<script[^>]*>(.*?)</script>").unwrap());
    let script_re = &*SCRIPT_RE;

    let mut result = String::new();
    let mut last_end = 0;

    for cap in script_re.captures_iter(content) {
        let inner = cap.get(1).unwrap();
        // Add empty lines for all content before this script block
        let prefix = &content[last_end..inner.start()];
        for ch in prefix.chars() {
            if ch == '\n' {
                result.push('\n');
            }
        }
        result.push_str(inner.as_str());
        last_end = inner.end();
    }
    result
}

/// Extract script content from Svelte component, preserving line numbers
pub fn extract_svelte_script(content: &str) -> String {
    static SCRIPT_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<script[^>]*>(.*?)</script>").unwrap());
    let script_re = &*SCRIPT_RE;

    let mut result = String::new();
    let mut last_end = 0;

    for cap in script_re.captures_iter(content) {
        let inner = cap.get(1).unwrap();
        // Add empty lines for all content before this script block
        let prefix = &content[last_end..inner.start()];
        for ch in prefix.chars() {
            if ch == '\n' {
                result.push('\n');
            }
        }
        result.push_str(inner.as_str());
        last_end = inner.end();
    }
    result
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
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserService" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "ChildClass" && s.parents.iter().any(|(p, _)| p == "ParentClass")));
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
        assert!(symbols
            .iter()
            .any(|s| s.name == "User" && s.kind == SymbolKind::Interface));
        assert!(symbols
            .iter()
            .any(|s| s.name == "IUserService" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_parse_type_alias() {
        let content = r#"
type UserId = string;
export type UserMap = Map<string, User>;
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserId" && s.kind == SymbolKind::TypeAlias));
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserMap" && s.kind == SymbolKind::TypeAlias));
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
        assert!(symbols
            .iter()
            .any(|s| s.name == "Status" && s.kind == SymbolKind::Enum));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Direction" && s.kind == SymbolKind::Enum));
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
        assert!(symbols
            .iter()
            .any(|s| s.name == "handleRequest" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "fetchUser" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "processData" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "asyncHandler" && s.kind == SymbolKind::Function));
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
        assert!(symbols
            .iter()
            .any(|s| s.name == "useAuth" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "useCounter" && s.kind == SymbolKind::Function));
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
        assert!(symbols
            .iter()
            .any(|s| s.name == "Button" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserCard" && s.kind == SymbolKind::Class));
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
        assert!(symbols
            .iter()
            .any(|s| s.name == "@Controller" && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "@Get" && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "@Post" && s.kind == SymbolKind::Annotation));
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
        assert!(symbols
            .iter()
            .any(|s| s.name == "Utils" && s.kind == SymbolKind::Package));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Types" && s.kind == SymbolKind::Package));
    }

    #[test]
    fn test_parse_constants() {
        let content = r#"
const API_URL = 'https://api.example.com';
export const MAX_RETRIES = 3;
const INTERNAL_TIMEOUT = 5000;
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "API_URL" && s.kind == SymbolKind::Constant));
        assert!(symbols
            .iter()
            .any(|s| s.name == "MAX_RETRIES" && s.kind == SymbolKind::Constant));
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

    #[test]
    fn test_extract_vue_script_preserves_line_numbers() {
        let content = "<template>\n  <div>hello</div>\n</template>\n\n<script setup lang=\"ts\">\nimport { ref } from 'vue'\nconst count = ref(0)\nfunction increment() { count.value++ }\n</script>\n";
        let script = extract_vue_script(content);
        // <template> block is 3 lines + 1 empty + <script> tag = 5 lines before content
        // So script content should start at line 6
        let lines: Vec<&str> = script.lines().collect();
        // First 5 lines should be empty (preserving offset)
        assert_eq!(lines[0], "", "line 1 should be empty (template)");
        assert_eq!(lines[1], "", "line 2 should be empty (template)");
        assert_eq!(lines[2], "", "line 3 should be empty (template)");
        assert_eq!(lines[3], "", "line 4 should be empty (blank)");
        assert_eq!(lines[4], "", "line 5 should be empty (script tag)");
        assert!(lines[5].contains("import"), "line 6 should have import");
        assert!(lines[6].contains("const count"), "line 7 should have const");
        assert!(
            lines[7].contains("function increment"),
            "line 8 should have function"
        );
    }

    #[test]
    fn test_vue_script_setup_symbols() {
        let content = "<template>\n  <div>{{ count }}</div>\n</template>\n\n<script setup lang=\"ts\">\nimport { ref, computed } from 'vue'\nconst count = ref(0)\nconst API_URL = 'https://example.com'\nfunction increment() { count.value++ }\nconst doubled = computed(() => count.value * 2)\n</script>\n";

        let script = extract_vue_script(content);
        let symbols = parse_typescript_symbols(&script).unwrap();

        // Should find the function
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "increment" && s.kind == SymbolKind::Function),
            "should find 'increment' function"
        );
        // Should find UPPER_CASE constant
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "API_URL" && s.kind == SymbolKind::Constant),
            "should find 'API_URL' constant"
        );
        // Line numbers should reflect Vue file position (not start from 1)
        let increment = symbols.iter().find(|s| s.name == "increment").unwrap();
        assert_eq!(
            increment.line, 9,
            "increment should be on line 9 of Vue file"
        );
    }

    #[test]
    fn test_extract_svelte_script_preserves_line_numbers() {
        let content = "<script lang=\"ts\">\nimport { onMount } from 'svelte'\nlet count = 0\nfunction increment() { count++ }\n</script>\n\n<main>\n  <p>{count}</p>\n</main>\n";
        let script = extract_svelte_script(content);
        let lines: Vec<&str> = script.lines().collect();
        // <script> tag is on line 1, content starts on line 2
        assert_eq!(lines[0], "", "line 1 should be empty (script tag)");
        assert!(lines[1].contains("import"), "line 2 should have import");
        assert!(
            lines[2].contains("let count"),
            "line 3 should have let count"
        );
        assert!(
            lines[3].contains("function increment"),
            "line 4 should have function"
        );
    }

    #[test]
    fn test_parse_vue_reactive_variables() {
        let content = r#"
const count = ref(0)
const items = reactive<Item[]>([])
const doubled = computed(() => count.value * 2)
const name = shallowRef('hello')
const data = readonly(state)
const x = toRef(props, 'x')
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "count" && s.kind == SymbolKind::Property),
            "should find 'count' as reactive property"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "items" && s.kind == SymbolKind::Property),
            "should find 'items' as reactive property"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "doubled" && s.kind == SymbolKind::Property),
            "should find 'doubled' as reactive property"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "name" && s.kind == SymbolKind::Property),
            "should find 'name' as reactive property"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "data" && s.kind == SymbolKind::Property),
            "should find 'data' as reactive property"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "x" && s.kind == SymbolKind::Property),
            "should find 'x' as reactive property"
        );
    }

    #[test]
    fn test_parse_vue_lifecycle_hooks() {
        let content = r#"
onMounted(() => {
  console.log('mounted')
})
onUnmounted(() => cleanup())
watch(count, (val) => console.log(val))
watchEffect(() => console.log(count.value))
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "onMounted" && s.kind == SymbolKind::Annotation),
            "should find 'onMounted' as lifecycle hook"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "onUnmounted" && s.kind == SymbolKind::Annotation),
            "should find 'onUnmounted' as lifecycle hook"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "watch" && s.kind == SymbolKind::Annotation),
            "should find 'watch' as lifecycle hook"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "watchEffect" && s.kind == SymbolKind::Annotation),
            "should find 'watchEffect' as lifecycle hook"
        );
    }

    #[test]
    fn test_parse_export_default_object() {
        let content = r#"
export default {
  install(app) {
    app.component('MyComponent', MyComponent)
  }
}
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "default" && s.kind == SymbolKind::Object),
            "should find 'default' as object export"
        );
    }

    #[test]
    fn test_parse_export_default_call() {
        let content = r#"
export default createRouter({
  history: createWebHistory(),
  routes,
})
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "createRouter" && s.kind == SymbolKind::Function),
            "should find 'createRouter' as default export call"
        );
    }

    #[test]
    fn test_parse_export_default_identifier() {
        let content = r#"
const router = createRouter({ routes })

export default router;
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "default(router)" && s.kind == SymbolKind::Object),
            "should find 'default(router)' as re-exported identifier"
        );
    }

    #[test]
    fn test_parse_vue_macros() {
        let content = r#"
const props = defineProps<{ msg: string }>()
const emit = defineEmits<{ click: [] }>()
defineExpose({ count })
const model = defineModel<string>()
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "props" && s.kind == SymbolKind::Property),
            "should find 'props' from defineProps"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "emit" && s.kind == SymbolKind::Property),
            "should find 'emit' from defineEmits"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "defineExpose" && s.kind == SymbolKind::Property),
            "should find 'defineExpose' without variable"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "model" && s.kind == SymbolKind::Property),
            "should find 'model' from defineModel"
        );
    }

    #[test]
    fn test_parse_define_store() {
        let content = r#"
export const useAuthStore = defineStore('auth', () => {
  const user = ref(null)
  return { user }
})
"#;
        let symbols = parse_typescript_symbols(content).unwrap();
        // useAuthStore should be caught by HOOK_RE (useXxx pattern), not as defineStore
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "useAuthStore" && s.kind == SymbolKind::Function),
            "should find 'useAuthStore' as hook/function"
        );
        // Should also find reactive variable inside
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "user" && s.kind == SymbolKind::Property),
            "should find 'user' as reactive property"
        );
    }

    #[test]
    fn test_vue_composition_api_full() {
        let content = "<template>\n  <div>{{ count }}</div>\n</template>\n\n<script setup lang=\"ts\">\nimport { ref, computed, onMounted } from 'vue'\n\nconst props = defineProps<{ initial: number }>()\nconst emit = defineEmits<{ update: [value: number] }>()\n\nconst count = ref(props.initial)\nconst doubled = computed(() => count.value * 2)\n\nfunction increment() {\n  count.value++\n  emit('update', count.value)\n}\n\nonMounted(() => {\n  console.log('ready')\n})\n</script>\n";

        let script = extract_vue_script(content);
        let symbols = parse_typescript_symbols(&script).unwrap();

        // Function
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "increment" && s.kind == SymbolKind::Function),
            "should find 'increment' function"
        );
        // Reactive variables
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "count" && s.kind == SymbolKind::Property),
            "should find 'count' as reactive ref"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "doubled" && s.kind == SymbolKind::Property),
            "should find 'doubled' as computed"
        );
        // Vue macros
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "props" && s.kind == SymbolKind::Property),
            "should find 'props' from defineProps"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "emit" && s.kind == SymbolKind::Property),
            "should find 'emit' from defineEmits"
        );
        // Lifecycle
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "onMounted" && s.kind == SymbolKind::Annotation),
            "should find 'onMounted' lifecycle hook"
        );
    }
}
