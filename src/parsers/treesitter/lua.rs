//! Tree-sitter based Lua parser

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{node_line, node_text, parse_tree, signature_line, text_end_line, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static LUA_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_lua::LANGUAGE.into());

static LUA_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&LUA_LANGUAGE, include_str!("queries/lua.scm"))
        .expect("Failed to compile Lua tree-sitter query")
});

pub static LUA_PARSER: LuaParser = LuaParser;

pub struct LuaParser;

/// Comments and string literals.
static NON_CODE: super::NonCode = super::NonCode {
    language: &LUA_LANGUAGE,
    prose: &["comment"],
    strings: &["string"],
    code: &[],
    keep: super::keep_no_string,
};

impl LanguageParser for LuaParser {
    fn non_code(&self) -> Option<&'static super::NonCode> {
        Some(&NON_CODE)
    }

    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &LUA_LANGUAGE)?;
        let mut symbols = Vec::new();
        let mut cursor = QueryCursor::new();
        let query = &*LUA_QUERY;

        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_func_name = idx("func_name");
        let idx_local_func_name = idx("local_func_name");
        let idx_method_class = idx("method_class");
        let idx_method_name = idx("method_name");
        let idx_dot_method_class = idx("dot_method_class");
        let idx_dot_method_name = idx("dot_method_name");
        let idx_local_var_name = idx("local_var_name");
        let idx_local_var_value = idx("local_var_value");
        let idx_require_alias = idx("require_alias");
        let idx_require_path = idx("require_path");
        let idx_module_return = idx("module_return");
        let idx_definition = idx("definition");

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            let end_line = find_capture(m, idx_definition).map(|c| text_end_line(content, &c.node));

            // Require import (must check before local_var since it also matches variable_declaration)
            if let Some(alias_cap) = find_capture(m, idx_require_alias) {
                if let Some(path_cap) = find_capture(m, idx_require_path) {
                    let alias = node_text(content, &alias_cap.node);
                    let path = node_text(content, &path_cap.node);
                    let line = node_line(&alias_cap.node);
                    symbols.push(ParsedSymbol {
                        name: alias.to_string(),
                        kind: SymbolKind::Import,
                        line,
                        signature: signature_line(content, line),
                        parents: vec![(path.to_string(), "from".to_string())],
                        end_line,
                    });
                }
                continue;
            }

            // Method with colon: function Class:method()
            if let Some(class_cap) = find_capture(m, idx_method_class) {
                if let Some(name_cap) = find_capture(m, idx_method_name) {
                    let class = node_text(content, &class_cap.node);
                    let name = node_text(content, &name_cap.node);
                    let line = node_line(&name_cap.node);
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Function,
                        line,
                        signature: signature_line(content, line),
                        parents: vec![(class.to_string(), "receiver".to_string())],
                        end_line,
                    });
                }
                continue;
            }

            // Method with dot: function Class.method()
            if let Some(class_cap) = find_capture(m, idx_dot_method_class) {
                if let Some(name_cap) = find_capture(m, idx_dot_method_name) {
                    let class = node_text(content, &class_cap.node);
                    let name = node_text(content, &name_cap.node);
                    let line = node_line(&name_cap.node);
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Function,
                        line,
                        signature: signature_line(content, line),
                        parents: vec![(class.to_string(), "receiver".to_string())],
                        end_line,
                    });
                }
                continue;
            }

            // Function (global)
            if let Some(cap) = find_capture(m, idx_func_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Local function
            if let Some(cap) = find_capture(m, idx_local_func_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Local variable with table constructor → Class
            if let Some(name_cap) = find_capture(m, idx_local_var_name) {
                if let Some(val_cap) = find_capture(m, idx_local_var_value) {
                    let name = node_text(content, &name_cap.node);
                    let line = node_line(&name_cap.node);
                    let val_kind = val_cap.node.kind();
                    let kind = if val_kind == "table_constructor" {
                        SymbolKind::Class
                    } else {
                        SymbolKind::Property
                    };
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind,
                        line,
                        signature: signature_line(content, line),
                        parents: vec![],
                        end_line,
                    });
                }
                continue;
            }

            // Module return
            if let Some(cap) = find_capture(m, idx_module_return) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Package,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }
        }

        Ok(symbols)
    }
}

/// Find a capture by index in a match
fn find_capture<'a>(
    m: &'a tree_sitter::QueryMatch<'a, 'a>,
    idx: Option<u32>,
) -> Option<&'a tree_sitter::QueryCapture<'a>> {
    let idx = idx?;
    m.captures.iter().find(|c| c.index == idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_function() {
        let content = "function greet(name)\n    print(name)\nend\n";
        let symbols = LUA_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "greet" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_local_function() {
        let content = "local function helper(x)\n    return x + 1\nend\n";
        let symbols = LUA_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "helper" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_method() {
        let content = "function MyClass:init(name)\n    self.name = name\nend\n";
        let symbols = LUA_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "init"
            && s.kind == SymbolKind::Function
            && s.parents
                .iter()
                .any(|(p, k)| p == "MyClass" && k == "receiver")));
    }

    #[test]
    fn test_parse_variable() {
        let content = "local max_retries = 5\n";
        let symbols = LUA_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "max_retries" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_comments_ignored() {
        let content = "-- function fake()\nfunction real()\nend\n--[[ function also_fake() ]]\n";
        let symbols = LUA_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "real"));
        assert!(!symbols.iter().any(|s| s.name == "fake"));
        assert!(!symbols.iter().any(|s| s.name == "also_fake"));
    }

    #[test]
    fn test_full_module() {
        let content = r#"local M = {}

local json = require("cjson")

local max_size = 1024

function M:new(opts)
    local instance = setmetatable({}, { __index = self })
    instance.opts = opts
    return instance
end

function M.create(name)
    return M:new({ name = name })
end

local function validate(input)
    return input ~= nil
end

return M
"#;
        let symbols = LUA_PARSER.parse_symbols(content).unwrap();

        // Table constructor as class
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "M" && s.kind == SymbolKind::Class),
            "Expected M as Class, got: {:?}",
            symbols
        );

        // Require import
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "json" && s.kind == SymbolKind::Import),
            "Expected json import, got: {:?}",
            symbols
        );

        // Local variable
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "max_size" && s.kind == SymbolKind::Property),
            "Expected max_size as Property, got: {:?}",
            symbols
        );

        // Colon method
        assert!(
            symbols.iter().any(|s| s.name == "new"
                && s.kind == SymbolKind::Function
                && s.parents.iter().any(|(p, _)| p == "M")),
            "Expected M:new method, got: {:?}",
            symbols
        );

        // Dot method
        assert!(
            symbols.iter().any(|s| s.name == "create"
                && s.kind == SymbolKind::Function
                && s.parents.iter().any(|(p, _)| p == "M")),
            "Expected M.create method, got: {:?}",
            symbols
        );

        // Local function
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "validate" && s.kind == SymbolKind::Function),
            "Expected validate function, got: {:?}",
            symbols
        );

        // Module return
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "M" && s.kind == SymbolKind::Package),
            "Expected M as module return, got: {:?}",
            symbols
        );
    }
}
