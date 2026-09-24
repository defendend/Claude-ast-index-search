//! Tree-sitter based Elixir parser

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{node_line, node_text, parse_tree, signature_line, text_end_line, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static ELIXIR_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_elixir::LANGUAGE.into());

static ELIXIR_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&ELIXIR_LANGUAGE, include_str!("queries/elixir.scm"))
        .expect("Failed to compile Elixir tree-sitter query")
});

pub static ELIXIR_PARSER: ElixirParser = ElixirParser;

pub struct ElixirParser;

impl LanguageParser for ElixirParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &ELIXIR_LANGUAGE)?;
        let mut symbols = Vec::new();
        let mut cursor = QueryCursor::new();
        let query = &*ELIXIR_QUERY;

        // Build capture name → index map
        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_call_type = idx("call_type");
        let idx_module_name = idx("module_name");
        let idx_def_type = idx("def_type");
        let idx_func_name = idx("func_name");
        let idx_def_type_noargs = idx("def_type_noargs");
        let idx_func_name_noargs = idx("func_name_noargs");
        let idx_def_type_guard = idx("def_type_guard");
        let idx_func_name_guard = idx("func_name_guard");
        let idx_struct_call = idx("struct_call");
        let idx_attr_name = idx("attr_name");
        let idx_attr_name_simple = idx("attr_name_simple");
        let idx_impl_call = idx("impl_call");
        let idx_impl_protocol = idx("impl_protocol");
        let idx_definition = idx("definition");

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            let end_line = find_capture(m, idx_definition).map(|c| text_end_line(content, &c.node));

            // Module / Protocol definition: defmodule MyModule / defprotocol MyProtocol
            if let Some(type_cap) = find_capture(m, idx_call_type) {
                let call_type = node_text(content, &type_cap.node);
                if let Some(name_cap) = find_capture(m, idx_module_name) {
                    let name = node_text(content, &name_cap.node);
                    let line = node_line(&name_cap.node);
                    let kind = match call_type {
                        "defprotocol" => SymbolKind::Interface,
                        "defmodule" => SymbolKind::Class,
                        _ => continue,
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

            // defimpl: defimpl Protocol, for: Module
            if let Some(type_cap) = find_capture(m, idx_impl_call) {
                let call_type = node_text(content, &type_cap.node);
                if call_type == "defimpl" {
                    if let Some(proto_cap) = find_capture(m, idx_impl_protocol) {
                        let name = node_text(content, &proto_cap.node);
                        let line = node_line(&proto_cap.node);
                        symbols.push(ParsedSymbol {
                            name: format!("{}(impl)", name),
                            kind: SymbolKind::Class,
                            line,
                            signature: signature_line(content, line),
                            parents: vec![(name.to_string(), "implements".to_string())],
                            end_line,
                        });
                    }
                }
                continue;
            }

            // Function/macro definitions with arguments: def foo(args)
            if let Some(type_cap) = find_capture(m, idx_def_type) {
                let def_type = node_text(content, &type_cap.node);
                if is_def_keyword(def_type) {
                    if let Some(name_cap) = find_capture(m, idx_func_name) {
                        let name = node_text(content, &name_cap.node);
                        let line = node_line(&name_cap.node);
                        symbols.push(ParsedSymbol {
                            name: name.to_string(),
                            kind: SymbolKind::Function,
                            line,
                            signature: signature_line(content, line),
                            parents: vec![],
                            end_line,
                        });
                    }
                }
                continue;
            }

            // Zero-arity function/macro definitions: def foo do ... end
            if let Some(type_cap) = find_capture(m, idx_def_type_noargs) {
                let def_type = node_text(content, &type_cap.node);
                if is_def_keyword(def_type) {
                    if let Some(name_cap) = find_capture(m, idx_func_name_noargs) {
                        let name = node_text(content, &name_cap.node);
                        let line = node_line(&name_cap.node);
                        symbols.push(ParsedSymbol {
                            name: name.to_string(),
                            kind: SymbolKind::Function,
                            line,
                            signature: signature_line(content, line),
                            parents: vec![],
                            end_line,
                        });
                    }
                }
                continue;
            }

            // Function/macro with guard: def foo(args) when guard
            if let Some(type_cap) = find_capture(m, idx_def_type_guard) {
                let def_type = node_text(content, &type_cap.node);
                if is_def_keyword(def_type) {
                    if let Some(name_cap) = find_capture(m, idx_func_name_guard) {
                        let name = node_text(content, &name_cap.node);
                        let line = node_line(&name_cap.node);
                        symbols.push(ParsedSymbol {
                            name: name.to_string(),
                            kind: SymbolKind::Function,
                            line,
                            signature: signature_line(content, line),
                            parents: vec![],
                            end_line,
                        });
                    }
                }
                continue;
            }

            // defstruct
            if let Some(cap) = find_capture(m, idx_struct_call) {
                let call_type = node_text(content, &cap.node);
                if call_type == "defstruct" {
                    let line = node_line(&cap.node);
                    symbols.push(ParsedSymbol {
                        name: "defstruct".to_string(),
                        kind: SymbolKind::Class,
                        line,
                        signature: signature_line(content, line),
                        parents: vec![],
                        end_line,
                    });
                }
                continue;
            }

            // Module attributes: @type, @spec, @callback, @moduledoc, @doc
            if let Some(cap) = find_capture(m, idx_attr_name) {
                let attr = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                let kind = match attr {
                    "type" | "typep" | "opaque" => SymbolKind::TypeAlias,
                    "spec" => SymbolKind::Annotation,
                    "callback" => SymbolKind::Annotation,
                    "moduledoc" | "doc" => SymbolKind::Annotation,
                    _ => continue,
                };
                symbols.push(ParsedSymbol {
                    name: format!("@{}", attr),
                    kind,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Simple module attributes
            if let Some(cap) = find_capture(m, idx_attr_name_simple) {
                let attr = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                let kind = match attr {
                    "type" | "typep" | "opaque" => SymbolKind::TypeAlias,
                    "spec" => SymbolKind::Annotation,
                    "callback" => SymbolKind::Annotation,
                    "moduledoc" | "doc" => SymbolKind::Annotation,
                    _ => continue,
                };
                symbols.push(ParsedSymbol {
                    name: format!("@{}", attr),
                    kind,
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

/// Check if the identifier is a def/defmacro keyword
fn is_def_keyword(s: &str) -> bool {
    matches!(
        s,
        "def" | "defp" | "defmacro" | "defmacrop" | "defguard" | "defguardp" | "defdelegate"
    )
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
    fn test_parse_module() {
        let content = r#"defmodule MyApp.Users do
  def hello, do: :world
end
"#;
        let symbols = ELIXIR_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyApp.Users" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_function() {
        let content = r#"defmodule MyApp do
  def greet(name) do
    "Hello, #{name}"
  end
end
"#;
        let symbols = ELIXIR_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "greet" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_private_function() {
        let content = r#"defmodule MyApp do
  defp internal_helper(x) do
    x * 2
  end
end
"#;
        let symbols = ELIXIR_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "internal_helper" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_macro() {
        let content = r#"defmodule MyMacros do
  defmacro my_if(condition, do: do_clause) do
    quote do
      case unquote(condition) do
        x when x in [false, nil] -> nil
        _ -> unquote(do_clause)
      end
    end
  end
end
"#;
        let symbols = ELIXIR_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "my_if" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_struct() {
        let content = r#"defmodule User do
  defstruct [:name, :age, :email]
end
"#;
        let symbols = ELIXIR_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "defstruct" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_protocol() {
        let content = r#"defprotocol Printable do
  @doc "Converts data to a printable string"
  def to_string(data)
end
"#;
        let symbols = ELIXIR_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Printable" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_parse_type_attribute() {
        let content = r#"defmodule MyApp do
  @type user :: %{name: String.t(), age: integer()}
end
"#;
        let symbols = ELIXIR_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "@type" && s.kind == SymbolKind::TypeAlias));
    }

    #[test]
    fn test_comments_ignored() {
        let content = r#"defmodule MyApp do
  # def fake_function(x) do
  #   x
  # end
  def real_function(x) do
    x
  end
end
"#;
        let symbols = ELIXIR_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "real_function"));
        assert!(!symbols.iter().any(|s| s.name == "fake_function"));
    }

    #[test]
    fn test_parse_impl() {
        let content = r#"defimpl Printable, for: User do
  def to_string(user) do
    user.name
  end
end
"#;
        let symbols = ELIXIR_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Printable(impl)" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_zero_arity_function() {
        let content = r#"defmodule MyApp do
  def hello do
    :world
  end
end
"#;
        let symbols = ELIXIR_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "hello" && s.kind == SymbolKind::Function));
    }
}
