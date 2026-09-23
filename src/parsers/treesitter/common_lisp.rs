//! Tree-sitter based Common Lisp parser

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{line_text, node_line, node_text, parse_tree, text_end_line, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static CL_LANGUAGE: LazyLock<Language> =
    LazyLock::new(|| tree_sitter_commonlisp::LANGUAGE_COMMONLISP.into());

static CL_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&CL_LANGUAGE, include_str!("queries/commonlisp.scm"))
        .expect("Failed to compile Common Lisp tree-sitter query")
});

pub static COMMON_LISP_PARSER: CommonLispParser = CommonLispParser;

pub struct CommonLispParser;

impl LanguageParser for CommonLispParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &CL_LANGUAGE)?;
        let mut symbols = Vec::new();
        let mut cursor = QueryCursor::new();
        let query = &*CL_QUERY;

        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_kw = idx("kw");
        let idx_func_name = idx("func_name");
        let idx_kw_pkg = idx("kw_pkg");
        let idx_func_name_pkg = idx("func_name_pkg");
        let idx_class_name = idx("class_name");
        let idx_struct_name = idx("struct_name");
        let idx_var_name = idx("var_name");
        let idx_const_name = idx("const_name");
        let idx_pkg_name_kwd = idx("pkg_name_kwd");
        let idx_pkg_name_sym = idx("pkg_name_sym");
        let idx_definition = idx("definition");

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            // A `defun_header` holds only the name and lambda list; the body
            // forms are its siblings in the enclosing list.
            let end_line = find_capture(m, idx_definition).map(|c| {
                let definition = match c.node.kind() {
                    "defun_header" => c.node.parent().unwrap_or(c.node),
                    _ => c.node,
                };
                text_end_line(content, &definition)
            });

            // defun/defmacro/defgeneric/defmethod with simple name
            if let Some(name_cap) = find_capture(m, idx_func_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                let kind = kw_to_kind(find_capture(m, idx_kw).map(|c| node_text(content, &c.node)));
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // defun/defmacro/defgeneric/defmethod with package-qualified name
            if let Some(name_cap) = find_capture(m, idx_func_name_pkg) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                let kind =
                    kw_to_kind(find_capture(m, idx_kw_pkg).map(|c| node_text(content, &c.node)));
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // defclass
            if let Some(cap) = find_capture(m, idx_class_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Class,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // defstruct
            if let Some(cap) = find_capture(m, idx_struct_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Class,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // defvar / defparameter
            if let Some(cap) = find_capture(m, idx_var_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Property,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // defconstant
            if let Some(cap) = find_capture(m, idx_const_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Constant,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // defpackage with keyword name (:my-package)
            if let Some(cap) = find_capture(m, idx_pkg_name_kwd) {
                let raw = node_text(content, &cap.node);
                // Strip leading colon from keyword symbol
                let name = raw.trim_start_matches(':');
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Package,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // defpackage with symbol name
            if let Some(cap) = find_capture(m, idx_pkg_name_sym) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Package,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }
        }

        Ok(symbols)
    }
}

/// Map defun_keyword text to SymbolKind
fn kw_to_kind(kw: Option<&str>) -> SymbolKind {
    match kw {
        Some(k) if k.eq_ignore_ascii_case("defmacro") => SymbolKind::Annotation,
        _ => SymbolKind::Function,
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
    fn test_parse_defun() {
        let content = "(defun my-function (x y)\n  (+ x y))\n";
        let symbols = COMMON_LISP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "my-function" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_defmacro() {
        let content = "(defmacro when-bound (var &body body)\n  `(when (boundp ',var) ,@body))\n";
        let symbols = COMMON_LISP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "when-bound" && s.kind == SymbolKind::Annotation));
    }

    #[test]
    fn test_parse_defgeneric() {
        let content =
            "(defgeneric speak (animal)\n  (:documentation \"Make the animal speak.\"))\n";
        let symbols = COMMON_LISP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "speak" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_defmethod() {
        let content = "(defmethod speak ((animal dog))\n  \"Woof!\")\n";
        let symbols = COMMON_LISP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "speak" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_defclass() {
        let content = r#"(defclass animal ()
  ((name :accessor animal-name :initarg :name)
   (sound :accessor animal-sound :initarg :sound)))
"#;
        let symbols = COMMON_LISP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "animal" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_defclass_with_superclass() {
        let content = "(defclass dog (animal)\n  ((breed :accessor dog-breed :initarg :breed)))\n";
        let symbols = COMMON_LISP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "dog" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_defstruct() {
        let content = "(defstruct point\n  x\n  y)\n";
        let symbols = COMMON_LISP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "point" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_defvar() {
        let content = "(defvar *max-retries* 3)\n";
        let symbols = COMMON_LISP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "*max-retries*" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_defparameter() {
        let content = "(defparameter *debug-mode* nil)\n";
        let symbols = COMMON_LISP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "*debug-mode*" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_defconstant() {
        let content = "(defconstant +pi+ 3.14159)\n";
        let symbols = COMMON_LISP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "+pi+" && s.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_parse_defpackage_keyword() {
        let content = "(defpackage :my-app\n  (:use :cl)\n  (:export #:main))\n";
        let symbols = COMMON_LISP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "my-app" && s.kind == SymbolKind::Package));
    }

    #[test]
    fn test_parse_defpackage_symbol() {
        let content = "(defpackage my-utils\n  (:use :cl))\n";
        let symbols = COMMON_LISP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "my-utils" && s.kind == SymbolKind::Package));
    }

    #[test]
    fn test_comments_ignored() {
        let content = "; (defun fake-func (x) x)\n(defun real-func (x) x)\n";
        let symbols = COMMON_LISP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "real-func"));
        assert!(!symbols.iter().any(|s| s.name == "fake-func"));
    }

    #[test]
    fn test_block_comment_ignored() {
        let content = "#|\n(defun fake-func (x) x)\n|#\n(defun real-func (x) x)\n";
        let symbols = COMMON_LISP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "real-func"));
        assert!(!symbols.iter().any(|s| s.name == "fake-func"));
    }

    #[test]
    fn test_full_module() {
        let content = r#"(defpackage :my-app
  (:use :cl)
  (:export #:make-point #:point-x #:point-y))

(in-package :my-app)

(defconstant +origin+ '(0 . 0))

(defparameter *default-color* :black)

(defstruct point
  x
  y)

(defclass shape ()
  ((color :accessor shape-color :initarg :color :initform *default-color*)))

(defgeneric area (shape)
  (:documentation "Calculate the area of a shape."))

(defmethod area ((s shape))
  0)

(defun make-colored-point (x y &optional (color :red))
  (make-point :x x :y y))

(defmacro with-shape ((var shape) &body body)
  `(let ((,var ,shape))
     ,@body))
"#;
        let symbols = COMMON_LISP_PARSER.parse_symbols(content).unwrap();

        assert!(
            symbols
                .iter()
                .any(|s| s.name == "my-app" && s.kind == SymbolKind::Package),
            "Expected my-app package, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "+origin+" && s.kind == SymbolKind::Constant),
            "Expected +origin+ constant, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "*default-color*" && s.kind == SymbolKind::Property),
            "Expected *default-color* parameter, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "point" && s.kind == SymbolKind::Class),
            "Expected point struct, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "shape" && s.kind == SymbolKind::Class),
            "Expected shape class, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "area" && s.kind == SymbolKind::Function),
            "Expected area generic, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "make-colored-point" && s.kind == SymbolKind::Function),
            "Expected make-colored-point function, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "with-shape" && s.kind == SymbolKind::Annotation),
            "Expected with-shape macro, got: {:?}",
            symbols
        );
    }
}
