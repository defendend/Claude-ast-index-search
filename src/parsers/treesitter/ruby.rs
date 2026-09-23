//! Tree-sitter based Ruby parser

use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{line_text, node_end_line, node_line, node_text, parse_tree, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static RUBY_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_ruby::LANGUAGE.into());

static RUBY_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&RUBY_LANGUAGE, include_str!("queries/ruby.scm"))
        .expect("Failed to compile Ruby tree-sitter query")
});

pub static RUBY_PARSER: RubyParser = RubyParser;

pub struct RubyParser;

impl RubyParser {
    fn ruby_extract_refs(
        &self,
        content: &str,
        defined: &[ParsedSymbol],
        file_type: Option<super::super::FileType>,
    ) -> Result<Vec<super::super::ParsedRef>> {
        // Start with the language-aware generic references
        let mut refs = super::super::extract_references_for_lang(content, defined, file_type)?;

        // Add Ruby-specific: bang methods (method!) and question methods (method?)
        // The generic extractor misses these because ! and ? are outside \w
        static RUBY_BANG_RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"\b([a-z_][a-z0-9_]*[!?])").unwrap());

        let defined_names: std::collections::HashSet<&str> =
            defined.iter().map(|s| s.name.as_str()).collect();

        for (line_num, line) in content.lines().enumerate() {
            let line_num = line_num + 1;
            let trimmed = line.trim();

            // Skip comments and definitions
            if trimmed.starts_with('#') {
                continue;
            }
            if trimmed.starts_with("def ") || trimmed.starts_with("def self.") {
                continue;
            }

            for caps in RUBY_BANG_RE.captures_iter(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                if !name.is_empty() && !defined_names.contains(name) && name.len() > 2 {
                    refs.push(super::super::ParsedRef {
                        name: name.to_string(),
                        line: line_num,
                        context: super::super::truncate_context(trimmed),
                    });
                }
            }
        }

        Ok(refs)
    }
}

impl LanguageParser for RubyParser {
    fn extract_refs(
        &self,
        content: &str,
        defined: &[ParsedSymbol],
    ) -> Result<Vec<super::super::ParsedRef>> {
        self.ruby_extract_refs(content, defined, None)
    }

    fn extract_refs_for_lang(
        &self,
        content: &str,
        defined: &[ParsedSymbol],
        file_type: super::super::FileType,
    ) -> Result<Vec<super::super::ParsedRef>> {
        self.ruby_extract_refs(content, defined, Some(file_type))
    }

    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &RUBY_LANGUAGE)?;
        let mut symbols = Vec::new();
        let query = &*RUBY_QUERY;
        let mut cursor = QueryCursor::new();

        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_class_name = idx("class_name");
        let idx_class_parent = idx("class_parent");
        let idx_class_node = idx("class_node");
        let idx_module_name = idx("module_name");
        let idx_module_node = idx("module_node");
        let idx_method_name = idx("method_name");
        let idx_method_node = idx("method_node");
        let idx_singleton_object = idx("singleton_object");
        let idx_singleton_method_name = idx("singleton_method_name");
        let idx_singleton_method_node = idx("singleton_method_node");
        let idx_assign_const_name = idx("assign_const_name");
        let idx_assign_const_node = idx("assign_const_node");
        let idx_call_method = idx("call_method");
        let idx_call_first_arg = idx("call_first_arg");

        let end_line_of = |m: &tree_sitter::QueryMatch, capture: Option<u32>| {
            find_capture(m, capture).map(|c| node_end_line(&c.node))
        };

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            // Class definition
            if let Some(name_cap) = find_capture(m, idx_class_name) {
                let raw_name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                let name = build_qualified_name(content, &name_cap.node, raw_name);
                let parents = find_capture(m, idx_class_parent)
                    .map(|p| {
                        vec![(
                            node_text(content, &p.node).to_string(),
                            "extends".to_string(),
                        )]
                    })
                    .unwrap_or_default();
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Class,
                    line,
                    end_line: end_line_of(m, idx_class_node),
                    signature: line_text(content, line).trim().to_string(),
                    parents,
                });
                continue;
            }

            // Module definition
            if let Some(cap) = find_capture(m, idx_module_name) {
                let raw_name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                let name = build_qualified_name(content, &cap.node, raw_name);
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Package,
                    line,
                    end_line: end_line_of(m, idx_module_node),
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                });
                continue;
            }

            // Singleton method: def self.method_name
            if let Some(obj_cap) = find_capture(m, idx_singleton_object) {
                if let Some(name_cap) = find_capture(m, idx_singleton_method_name) {
                    let obj = node_text(content, &obj_cap.node);
                    let method_name = node_text(content, &name_cap.node);
                    let line = node_line(&name_cap.node);
                    symbols.push(ParsedSymbol {
                        name: format!("{}.{}", obj, method_name),
                        kind: SymbolKind::Function,
                        line,
                        end_line: end_line_of(m, idx_singleton_method_node),
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
                continue;
            }

            // Instance method: def method_name
            if let Some(cap) = find_capture(m, idx_method_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    line,
                    end_line: end_line_of(m, idx_method_node),
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                });
                continue;
            }

            // Constant assignment: `LIMIT = 10`, `Types = Dry.Types()`,
            // `Billing::Import = Container.injector`
            if let Some(cap) = find_capture(m, idx_assign_const_name) {
                let text = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                if is_constant_path(text) {
                    let name = if cap.node.kind() == "scope_resolution" {
                        qualify_scoped_constant(content, &cap.node, text)
                    } else {
                        text.to_string()
                    };
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Constant,
                        line,
                        end_line: end_line_of(m, idx_assign_const_node),
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
                continue;
            }

            // Call expressions (DSL patterns)
            if let Some(method_cap) = find_capture(m, idx_call_method) {
                let method = node_text(content, &method_cap.node);
                let line = node_line(&method_cap.node);
                let first_arg =
                    find_capture(m, idx_call_first_arg).map(|c| node_text(content, &c.node));

                // Skip calls with a receiver (e.g., Foo.bar, obj.method)
                // We only want bare calls like `require 'json'`, `include Mod`, etc.
                let call_node = method_cap.node.parent();
                let call_end_line = call_node.as_ref().map(node_end_line);
                let has_receiver = call_node
                    .map(|n| n.child_by_field_name("receiver").is_some())
                    .unwrap_or(false);

                match method {
                    // require / require_relative
                    "require" | "require_relative" if !has_receiver => {
                        if let Some(arg) = first_arg {
                            let path = arg.trim_matches(|c| c == '\'' || c == '"');
                            symbols.push(ParsedSymbol {
                                name: path.to_string(),
                                kind: SymbolKind::Import,
                                line,
                                signature: line_text(content, line).trim().to_string(),
                                parents: vec![],
                                end_line: call_end_line,
                            });
                        }
                    }

                    // include / extend / prepend — Annotation (not Import) so outline shows them
                    "include" | "extend" | "prepend" if !has_receiver => {
                        if let Some(arg) = first_arg {
                            symbols.push(ParsedSymbol {
                                name: format!("{} {}", method, arg),
                                kind: SymbolKind::Annotation,
                                line,
                                signature: line_text(content, line).trim().to_string(),
                                parents: vec![],
                                end_line: call_end_line,
                            });
                        }
                    }

                    // attr_reader / attr_writer / attr_accessor — all arguments
                    "attr_reader" | "attr_writer" | "attr_accessor" if !has_receiver => {
                        let sig = line_text(content, line).trim().to_string();
                        if let Some(call) = call_node {
                            if let Some(args_node) = call.child_by_field_name("arguments") {
                                for i in 0..args_node.named_child_count() {
                                    if let Some(arg_node) = args_node.named_child(i as u32) {
                                        let arg_text = node_text(content, &arg_node);
                                        let sym_name = normalize_symbol(arg_text);
                                        symbols.push(ParsedSymbol {
                                            name: format!(":{}", sym_name),
                                            kind: SymbolKind::Property,
                                            line,
                                            signature: sig.clone(),
                                            parents: vec![],
                                            end_line: call_end_line,
                                        });
                                    }
                                }
                            }
                        }
                    }

                    // Rails associations: has_many, has_one, belongs_to, has_and_belongs_to_many
                    "has_many" | "has_one" | "belongs_to" | "has_and_belongs_to_many"
                        if !has_receiver =>
                    {
                        if let Some(arg) = first_arg {
                            let sym_name = normalize_symbol(arg);
                            symbols.push(ParsedSymbol {
                                name: format!("{} :{}", method, sym_name),
                                kind: SymbolKind::Property,
                                line,
                                signature: line_text(content, line).trim().to_string(),
                                parents: vec![],
                                end_line: call_end_line,
                            });
                        }
                    }

                    // Alba serializer: attribute (singular with block), one, many
                    // Dry::Initializer: option (keyword arg), param (positional arg)
                    "attribute" | "one" | "many" | "option" | "param" if !has_receiver => {
                        if let Some(arg) = first_arg {
                            let sym_name = normalize_symbol(arg);
                            symbols.push(ParsedSymbol {
                                name: format!("{} :{}", method, sym_name),
                                kind: SymbolKind::Property,
                                line,
                                signature: line_text(content, line).trim().to_string(),
                                parents: vec![],
                                end_line: call_end_line,
                            });
                        }
                    }

                    // Alba serializer: attributes (plural, multiple args)
                    "attributes" if !has_receiver => {
                        let sig = line_text(content, line).trim().to_string();
                        if let Some(call) = call_node {
                            if let Some(args_node) = call.child_by_field_name("arguments") {
                                for i in 0..args_node.named_child_count() {
                                    if let Some(arg_node) = args_node.named_child(i as u32) {
                                        let arg_text = node_text(content, &arg_node);
                                        let sym_name = normalize_symbol(arg_text);
                                        symbols.push(ParsedSymbol {
                                            name: format!("attributes :{}", sym_name),
                                            kind: SymbolKind::Property,
                                            line,
                                            signature: sig.clone(),
                                            parents: vec![],
                                            end_line: call_end_line,
                                        });
                                    }
                                }
                            }
                        }
                    }

                    // Rails ActiveStorage / enum / delegate / encrypts / store_accessor
                    "has_one_attached" | "has_many_attached" | "enum" | "delegate" | "encrypts"
                    | "store_accessor"
                        if !has_receiver =>
                    {
                        if let Some(arg) = first_arg {
                            let sym_name = normalize_symbol(arg);
                            symbols.push(ParsedSymbol {
                                name: format!("{} :{}", method, sym_name),
                                kind: SymbolKind::Property,
                                line,
                                signature: line_text(content, line).trim().to_string(),
                                parents: vec![],
                                end_line: call_end_line,
                            });
                        }
                    }

                    // Rails validates / validate
                    "validates" | "validate" if !has_receiver => {
                        if let Some(arg) = first_arg {
                            let sym_name = normalize_symbol(arg);
                            symbols.push(ParsedSymbol {
                                name: format!("{} :{}", method, sym_name),
                                kind: SymbolKind::Annotation,
                                line,
                                signature: line_text(content, line).trim().to_string(),
                                parents: vec![],
                                end_line: call_end_line,
                            });
                        }
                    }

                    // Rails callbacks
                    "before_action"
                    | "after_action"
                    | "around_action"
                    | "before_create"
                    | "after_create"
                    | "before_update"
                    | "after_update"
                    | "before_save"
                    | "after_save"
                    | "before_destroy"
                    | "after_destroy"
                    | "before_validation"
                    | "after_validation"
                    | "after_commit"
                    | "after_create_commit"
                    | "after_update_commit"
                    | "after_destroy_commit"
                    | "after_save_commit"
                    | "after_rollback"
                    | "around_create"
                    | "around_update"
                    | "around_save"
                    | "around_destroy"
                        if !has_receiver =>
                    {
                        if let Some(arg) = first_arg {
                            let sym_name = normalize_symbol(arg);
                            symbols.push(ParsedSymbol {
                                name: format!("{} :{}", method, sym_name),
                                kind: SymbolKind::Annotation,
                                line,
                                signature: line_text(content, line).trim().to_string(),
                                parents: vec![],
                                end_line: call_end_line,
                            });
                        }
                    }

                    // Rails scope
                    "scope" if !has_receiver => {
                        if let Some(arg) = first_arg {
                            let sym_name = normalize_symbol(arg);
                            symbols.push(ParsedSymbol {
                                name: format!("scope :{}", sym_name),
                                kind: SymbolKind::Function,
                                line,
                                signature: line_text(content, line).trim().to_string(),
                                parents: vec![],
                                end_line: call_end_line,
                            });
                        }
                    }

                    // RSpec describe / context (allow RSpec.describe with receiver)
                    "describe"
                    | "context"
                    | "shared_examples"
                    | "shared_context"
                    | "shared_examples_for" => {
                        // Allow RSpec.describe (has receiver), skip other receivers
                        if has_receiver {
                            let receiver_text = call_node
                                .and_then(|n| n.child_by_field_name("receiver"))
                                .map(|r| node_text(content, &r));
                            if receiver_text != Some("RSpec") {
                                continue;
                            }
                        }
                        if let Some(arg) = first_arg {
                            let desc = arg.trim_matches(|c| c == '\'' || c == '"');
                            symbols.push(ParsedSymbol {
                                name: format!("{} \"{}\"", method, desc),
                                kind: SymbolKind::Class,
                                line,
                                signature: line_text(content, line).trim().to_string(),
                                parents: vec![],
                                end_line: call_end_line,
                            });
                        }
                    }

                    // RSpec it / specify
                    "it" | "specify" if !has_receiver => {
                        if let Some(arg) = first_arg {
                            let desc = arg.trim_matches(|c| c == '\'' || c == '"');
                            symbols.push(ParsedSymbol {
                                name: format!("{} \"{}\"", method, desc),
                                kind: SymbolKind::Function,
                                line,
                                signature: line_text(content, line).trim().to_string(),
                                parents: vec![],
                                end_line: call_end_line,
                            });
                        }
                    }

                    // RSpec let / let! / subject
                    "let" | "let!" | "subject" if !has_receiver => {
                        if let Some(arg) = first_arg {
                            let sym_name = normalize_symbol(arg);
                            symbols.push(ParsedSymbol {
                                name: format!("{}(:{})", method, sym_name),
                                kind: SymbolKind::Property,
                                line,
                                signature: line_text(content, line).trim().to_string(),
                                parents: vec![],
                                end_line: call_end_line,
                            });
                        }
                    }

                    _ => {}
                }
                continue;
            }
        }

        Ok(symbols)
    }
}

/// Build a qualified name by walking up the AST to find enclosing class/module scopes.
///
/// For nested definitions like:
///   class Event
///     class CreateService
///   end
/// end
///
/// When processing `CreateService`, walks up the tree to find `Event` and returns `Event::CreateService`.
/// Already-qualified names (e.g., `Admin::Dashboard` from `class Admin::Dashboard`) are preserved as-is
/// and get parent scopes prepended if nested further.
fn build_qualified_name(content: &str, name_node: &tree_sitter::Node, base_name: &str) -> String {
    // The name_node is the captured name (constant or scope_resolution).
    // Its parent should be the class/module AST node.
    let container = match name_node.parent() {
        Some(n) if n.kind() == "class" || n.kind() == "module" => n,
        _ => return base_name.to_string(),
    };

    prefix_enclosing_scopes(content, container, base_name)
}

/// Prepend the names of every class/module enclosing `node` (outermost first).
fn prefix_enclosing_scopes(content: &str, node: tree_sitter::Node, base_name: &str) -> String {
    let mut scope_parts: Vec<String> = Vec::new();
    let mut current = node.parent();
    while let Some(node) = current {
        if node.kind() == "class" || node.kind() == "module" {
            if let Some(name_child) = node.child_by_field_name("name") {
                scope_parts.push(node_text(content, &name_child).to_string());
            }
        }
        current = node.parent();
    }

    if scope_parts.is_empty() {
        base_name.to_string()
    } else {
        scope_parts.reverse();
        scope_parts.push(base_name.to_string());
        scope_parts.join("::")
    }
}

/// Full name of a `Scope::Name = value` assignment, qualified like a class
/// written `class Scope::Name` at the same place; `::Name = value` is top-level.
fn qualify_scoped_constant(content: &str, name_node: &tree_sitter::Node, text: &str) -> String {
    if let Some(absolute) = text.strip_prefix("::") {
        return absolute.to_string();
    }
    match name_node.parent() {
        Some(assignment) => prefix_enclosing_scopes(content, assignment, text),
        None => text.to_string(),
    }
}

/// `Name`, `Scope::Name` or `::Name`: every segment a constant. Excludes
/// `Scope::method = value`, which is a setter call, not a constant.
fn is_constant_path(text: &str) -> bool {
    let text = text.strip_prefix("::").unwrap_or(text);
    !text.is_empty()
        && text.split("::").all(|segment| {
            segment.chars().next().is_some_and(char::is_uppercase)
                && segment.chars().all(|c| c.is_alphanumeric() || c == '_')
        })
}

/// Normalize a Ruby symbol argument: strip leading `:` from `:name`
fn normalize_symbol(s: &str) -> &str {
    s.strip_prefix(':').unwrap_or(s)
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
    fn test_parse_class() {
        let content = "class User < ApplicationRecord\n  def initialize\n  end\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "User" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "User"
            && s.parents
                .iter()
                .any(|(p, k)| p == "ApplicationRecord" && k == "extends")));
    }

    #[test]
    fn test_parse_class_no_parent() {
        let content = "class Service\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        let cls = symbols
            .iter()
            .find(|s| s.name == "Service" && s.kind == SymbolKind::Class);
        assert!(cls.is_some());
        assert!(cls.unwrap().parents.is_empty());
    }

    #[test]
    fn test_parse_namespaced_class() {
        let content = "class Admin::Dashboard\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Admin::Dashboard" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_module() {
        let content = "module Authenticatable\n  def authenticate\n    true\n  end\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Authenticatable" && s.kind == SymbolKind::Package));
    }

    #[test]
    fn test_parse_namespaced_module() {
        let content = "module Admin::Helpers\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Admin::Helpers" && s.kind == SymbolKind::Package));
    }

    #[test]
    fn test_parse_nested_module_class() {
        let content = "module Admin\n  class Dashboard\n  end\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Admin" && s.kind == SymbolKind::Package));
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "Admin::Dashboard" && s.kind == SymbolKind::Class),
            "nested class should have qualified name, got: {:?}",
            symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_parse_nested_class_class() {
        // Common Rails pattern: class Event; class CreateService
        let content = "class Event\n  class CreateService < Event::BaseService\n  end\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Event" && s.kind == SymbolKind::Class));
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "Event::CreateService" && s.kind == SymbolKind::Class),
            "nested class inside class should be qualified, got: {:?}",
            symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_parse_triple_nesting() {
        let content = "module Api\n  module V2\n    class UsersController < ApplicationController\n    end\n  end\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Api" && s.kind == SymbolKind::Package));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Api::V2" && s.kind == SymbolKind::Package));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Api::V2::UsersController" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_inline_namespace_unchanged() {
        // Already-qualified names should stay as-is
        let content = "class Stage::CountService < ApplicationService\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Stage::CountService" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_inline_namespace_inside_module() {
        // class Admin::Dashboard inside module V2 → V2::Admin::Dashboard
        let content = "module V2\n  class Admin::Dashboard\n  end\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "V2" && s.kind == SymbolKind::Package));
        assert!(symbols
            .iter()
            .any(|s| s.name == "V2::Admin::Dashboard" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_nested_module_inside_class() {
        let content = "class Event\n  module Types\n    class Stage\n    end\n  end\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Event" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Event::Types" && s.kind == SymbolKind::Package));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Event::Types::Stage" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_instance_method() {
        let content = "class Foo\n  def bar\n    42\n  end\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "bar" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_method_with_question_mark() {
        let content = "class Foo\n  def valid?\n    true\n  end\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "valid?" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_method_with_bang() {
        let content = "class Foo\n  def save!\n    persist\n  end\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "save!" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_class_method() {
        let content = "class Service\n  def self.call(params)\n    new(params).call\n  end\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "self.call" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_require() {
        let content = "require 'json'\nrequire 'net/http'\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "json" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "net/http" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn test_parse_require_relative() {
        let content = "require_relative './helpers'\nrequire_relative '../models/user'\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "./helpers" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "../models/user" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn test_parse_include_extend_prepend() {
        let content = "class User\n  include Authenticatable\n  extend ClassMethods\n  prepend Trackable\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "include Authenticatable" && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "extend ClassMethods" && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "prepend Trackable" && s.kind == SymbolKind::Annotation));
    }

    #[test]
    fn test_parse_attr_accessor() {
        let content = "class User\n  attr_reader :name, :email\n  attr_writer :password\n  attr_accessor :age\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == ":name" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == ":email" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == ":password" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == ":age" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_constants() {
        let content = "class Config\n  LIMIT = 100\n  DEFAULT_ROLE = \"user\"\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "LIMIT" && s.kind == SymbolKind::Constant));
        assert!(symbols
            .iter()
            .any(|s| s.name == "DEFAULT_ROLE" && s.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_parse_rails_associations() {
        let content = r#"class Post < ApplicationRecord
  belongs_to :author
  has_many :comments
  has_one :featured_image
  has_and_belongs_to_many :tags
end
"#;
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "belongs_to :author" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "has_many :comments" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "has_one :featured_image" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "has_and_belongs_to_many :tags" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_rails_validates() {
        let content =
            "class User < ApplicationRecord\n  validates :name\n  validates :email\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "validates :name" && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "validates :email" && s.kind == SymbolKind::Annotation));
    }

    #[test]
    fn test_parse_rails_validate_without_s() {
        let content = "class User < ApplicationRecord\n  validate :timezone_must_be_valid\n  validate :password_complexity\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "validate :timezone_must_be_valid"
                && s.kind == SymbolKind::Annotation));
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "validate :password_complexity"
                    && s.kind == SymbolKind::Annotation)
        );
    }

    #[test]
    fn test_parse_rails_dsl_methods() {
        let content = "class User < ApplicationRecord\n  enum :role, { admin: 0, user: 1 }\n  delegate :name, to: :profile\n  has_one_attached :avatar\n  has_many_attached :photos\n  encrypts :access_token\n  store_accessor :settings, :theme\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "enum :role" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "delegate :name" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "has_one_attached :avatar" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "has_many_attached :photos" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "encrypts :access_token" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "store_accessor :settings" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_rails_callbacks() {
        let content = "class Post < ApplicationRecord\n  before_save :normalize_title\n  after_create :notify_subscribers\n  after_commit :sync_to_calendar\n  after_update_commit :refresh_cache\n  around_save :wrap_in_transaction\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "before_save :normalize_title" && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "after_create :notify_subscribers"
                && s.kind == SymbolKind::Annotation));
        assert!(symbols.iter().any(
            |s| s.name == "after_commit :sync_to_calendar" && s.kind == SymbolKind::Annotation
        ));
        assert!(symbols
            .iter()
            .any(|s| s.name == "after_update_commit :refresh_cache"
                && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "around_save :wrap_in_transaction"
                && s.kind == SymbolKind::Annotation));
    }

    #[test]
    fn test_parse_rails_scope() {
        let content = "class Post < ApplicationRecord\n  scope :published, -> { where(published: true) }\n  scope :recent, -> { order(created_at: :desc) }\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "scope :published" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "scope :recent" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_rspec_describe_context() {
        let content = r#"RSpec.describe User, type: :model do
  describe "validations" do
    context "when valid" do
      it "returns true" do
      end
    end
  end
end
"#;
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols.iter().any(|s| s.name.contains("describe")
                && s.name.contains("User")
                && s.kind == SymbolKind::Class),
            "should find RSpec.describe with receiver"
        );
        assert!(symbols.iter().any(|s| s.name.contains("describe")
            && s.name.contains("validations")
            && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name.contains("context")
            && s.name.contains("when valid")
            && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_rspec_shared_examples() {
        let content = "RSpec.shared_examples \"authenticatable\" do\n  it \"authenticates\" do\n  end\nend\n\nshared_context \"with admin\" do\n  let(:admin) { create(:admin) }\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols.iter().any(|s| s.name.contains("shared_examples")
                && s.name.contains("authenticatable")
                && s.kind == SymbolKind::Class),
            "should find RSpec.shared_examples"
        );
        assert!(
            symbols.iter().any(|s| s.name.contains("shared_context")
                && s.name.contains("with admin")
                && s.kind == SymbolKind::Class),
            "should find shared_context"
        );
    }

    #[test]
    fn test_parse_rspec_it_specify() {
        let content = r#"describe "User" do
  it "returns true for valid user" do
    expect(true).to be_truthy
  end

  specify "returns false for invalid" do
    expect(false).to be_falsy
  end
end
"#;
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name.contains("it")
            && s.name.contains("returns true")
            && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name.contains("specify")
            && s.name.contains("returns false")
            && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_rspec_let() {
        let content = "describe User do\n  let(:user) { build(:user) }\n  let!(:admin) { create(:admin) }\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name.contains("let")
            && s.name.contains("user")
            && s.kind == SymbolKind::Property));
        assert!(symbols.iter().any(|s| s.name.contains("let!")
            && s.name.contains("admin")
            && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_full_rails_model() {
        let content = r#"require 'json'

class Post < ApplicationRecord
  include Publishable
  extend Searchable

  attr_accessor :draft_content

  CATEGORIES = %w[tech science art].freeze

  has_many :comments
  belongs_to :author

  validates :title
  validates :content

  scope :published, -> { where(published: true) }

  before_save :normalize_title
  after_create :notify_subscribers

  def initialize(attrs = {})
    super
  end

  def self.find_by_slug(slug)
    where(slug: slug).first
  end

  def publish!
    update(published: true)
  end

  private

  def normalize_title
    self.title = title.strip
  end
end
"#;
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();

        // Imports
        assert!(symbols
            .iter()
            .any(|s| s.name == "json" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "include Publishable" && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "extend Searchable" && s.kind == SymbolKind::Annotation));

        // Class
        assert!(symbols
            .iter()
            .any(|s| s.name == "Post" && s.kind == SymbolKind::Class));

        // Properties
        assert!(symbols
            .iter()
            .any(|s| s.name == ":draft_content" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "has_many :comments" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "belongs_to :author" && s.kind == SymbolKind::Property));

        // Constants
        assert!(symbols
            .iter()
            .any(|s| s.name == "CATEGORIES" && s.kind == SymbolKind::Constant));

        // Annotations
        assert!(symbols
            .iter()
            .any(|s| s.name == "validates :title" && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "before_save :normalize_title" && s.kind == SymbolKind::Annotation));

        // Functions
        assert!(symbols
            .iter()
            .any(|s| s.name == "scope :published" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "initialize" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "self.find_by_slug" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "publish!" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "normalize_title" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_comments_ignored() {
        let content = "# class FakeClass\n# def fake_method\nclass RealClass\n  def real_method\n  end\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealClass"));
        assert!(!symbols.iter().any(|s| s.name == "FakeClass"));
        assert!(symbols.iter().any(|s| s.name == "real_method"));
        assert!(!symbols.iter().any(|s| s.name == "fake_method"));
    }

    #[test]
    fn test_parse_method_with_params() {
        let content = "def process(input, output = nil)\n  input\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "process" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_scoped_constant_assignment_is_qualified() {
        let content = "Billing::Import = Billing::Container.injector\n\
                       module Api\n  V2::Client = Struct.new(:token) do\n    def ping; end\n  end\nend\n\
                       ::Root::Setting = 1\n\
                       Config::timeout = 5\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        let constant = |name: &str| {
            symbols
                .iter()
                .find(|s| s.name == name && s.kind == SymbolKind::Constant)
                .unwrap_or_else(|| panic!("no constant {name}: {symbols:?}"))
        };
        assert_eq!(constant("Billing::Import").line, 1);
        assert_eq!(constant("Billing::Import").end_line, Some(1));
        assert_eq!(constant("Api::V2::Client").line, 3);
        assert_eq!(constant("Api::V2::Client").end_line, Some(5));
        assert_eq!(constant("Root::Setting").line, 7);
        assert!(
            !symbols.iter().any(|s| s.name.contains("timeout")),
            "a setter call is not a constant: {symbols:?}"
        );
    }

    #[test]
    fn test_camel_case_constant_assignment() {
        let content = "module Types\n  Email = String.constrained(format: /@/)\nend\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Email" && s.kind == SymbolKind::Constant && s.line == 2));
    }

    #[test]
    fn test_constant_not_class() {
        // Constants should not be confused with class names
        let content = "VERSION = \"1.0\"\nMAX_RETRIES = 3\n";
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "VERSION" && s.kind == SymbolKind::Constant));
        assert!(symbols
            .iter()
            .any(|s| s.name == "MAX_RETRIES" && s.kind == SymbolKind::Constant));
        assert!(!symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Class && s.name == "VERSION"));
    }

    #[test]
    fn test_extract_refs_bang_methods() {
        let content = r#"class Controller
  def create
    authenticate_user!
    validate_contract!
    record.save!
  end
end
"#;
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        let refs = RUBY_PARSER.extract_refs(content, &symbols).unwrap();
        assert!(
            refs.iter().any(|r| r.name == "authenticate_user!"),
            "should find 'authenticate_user!' reference; got: {:?}",
            refs.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
        assert!(
            refs.iter().any(|r| r.name == "validate_contract!"),
            "should find 'validate_contract!' reference"
        );
        assert!(
            refs.iter().any(|r| r.name == "save!"),
            "should find 'save!' reference"
        );
    }

    #[test]
    fn test_extract_refs_question_methods() {
        let content = r#"class Service
  def process
    return unless valid?
    result.success?
  end
end
"#;
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        let refs = RUBY_PARSER.extract_refs(content, &symbols).unwrap();
        assert!(
            refs.iter().any(|r| r.name == "valid?"),
            "should find 'valid?' reference; got: {:?}",
            refs.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
        assert!(
            refs.iter().any(|r| r.name == "success?"),
            "should find 'success?' reference"
        );
    }

    #[test]
    fn test_extract_refs_skips_definitions() {
        // In a controller that CALLS authenticate_user! (defined elsewhere),
        // the method should appear as a reference
        let content = r#"class PostsController < BaseController
  before_action :authenticate_user!

  def create
    validate_contract!
  end
end
"#;
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        let refs = RUBY_PARSER.extract_refs(content, &symbols).unwrap();
        // authenticate_user! and validate_contract! should appear as refs
        // (they are NOT locally defined, only called)
        assert!(
            refs.iter().any(|r| r.name == "authenticate_user!"),
            "should find authenticate_user! as cross-file reference; got: {:?}",
            refs.iter()
                .filter(|r| r.name.contains('!'))
                .map(|r| &r.name)
                .collect::<Vec<_>>()
        );
        assert!(
            refs.iter().any(|r| r.name == "validate_contract!"),
            "should find validate_contract! as cross-file reference"
        );
    }

    #[test]
    fn test_parse_alba_attributes_plural() {
        // Alba: attributes :id, :name, :icon, :color
        let content = r#"class CategorySerializer
  include Alba::Resource
  attributes :id, :name, :icon, :color
end
"#;
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "attributes :id" && s.kind == SymbolKind::Property),
            "should find attributes :id; got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.kind))
                .collect::<Vec<_>>()
        );
        assert!(symbols
            .iter()
            .any(|s| s.name == "attributes :name" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "attributes :icon" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "attributes :color" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_alba_attribute_one_many() {
        // Alba: attribute (singular with block), one, many
        let content = r#"class EventRecordSerializer
  include Alba::Resource
  attributes :id, :name
  one :category, serializer: CategorySerializer
  many :records, serializer: RecordSerializer
  attribute :last_recorded_at do |record|
    record.last_recorded_at&.iso8601
  end
end
"#;
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "one :category" && s.kind == SymbolKind::Property),
            "should find one :category; got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.kind))
                .collect::<Vec<_>>()
        );
        assert!(symbols
            .iter()
            .any(|s| s.name == "many :records" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "attribute :last_recorded_at" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_dry_initializer_option_and_param() {
        // Dry::Initializer: option (keyword args), param (positional args)
        let content = r#"class CreateService < ApplicationService
  option :event_record, Types.Instance(EventRecord)
  option :email, Types::String
  option :category_ids, default: -> { nil }
  param :name, Types::String

  def process
    # ...
  end
end
"#;
        let symbols = RUBY_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "option :event_record" && s.kind == SymbolKind::Property),
            "should find option :event_record; got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.kind))
                .collect::<Vec<_>>()
        );
        assert!(symbols
            .iter()
            .any(|s| s.name == "option :email" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "option :category_ids" && s.kind == SymbolKind::Property));
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "param :name" && s.kind == SymbolKind::Property),
            "should find param :name; got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.kind))
                .collect::<Vec<_>>()
        );
    }
}
