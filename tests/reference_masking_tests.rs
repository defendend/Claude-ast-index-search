//! Reference extraction skips comments and string literals: a name in prose
//! is no use of the symbol. Code nested in a string (interpolation) and the
//! strings that name code stay.

use ast_index::parsers::{parse_file_symbols, FileType};

struct Case {
    file_type: FileType,
    source: &'static str,
    present: &'static [&'static str],
    absent: &'static [&'static str],
}

fn check(case: &Case) {
    let (_, refs) = parse_file_symbols(case.source, case.file_type).unwrap();
    let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
    for name in case.present {
        assert!(
            names.contains(name),
            "{:?}: expected a reference to {name}; got {names:?}",
            case.file_type
        );
    }
    for name in case.absent {
        assert!(
            !names.contains(name),
            "{:?}: {name} sits in a comment or a string; got {names:?}",
            case.file_type
        );
    }
}

#[test]
fn ruby_comments_strings_and_heredocs_are_not_references() {
    check(&Case {
        file_type: FileType::Ruby,
        source: r#"# CommentName is mentioned here
class Widget
  TYPES = %w[Event::WordName User-Agent].freeze

  def run
    CodeName.new("StringName #{InterpName} sure?")
    belongs_to :owner, class_name: 'KeptName'
    sql = <<~SQL
      SELECT * FROM t WHERE type = 'QuotedName' AND HeredocName
    SQL
  end
end
=begin
BlockName
=end
"#,
        present: &[
            "CodeName",
            "InterpName",
            "KeptName",
            "QuotedName",
            "WordName",
        ],
        absent: &[
            "CommentName",
            "StringName",
            "HeredocName",
            "SQL",
            "BlockName",
            "Agent",
            "sure?",
        ],
    });
}

#[test]
fn python_comments_docstrings_and_strings_are_not_references() {
    check(&Case {
        file_type: FileType::Python,
        source: r#"# CommentName
def run(item: "ForwardName") -> None:
    """DocName in a docstring.

    More ProseName here.
    """
    CodeName(f"StringName {InterpName}")
    method = "GET"
    patch("pkg.module.PatchedName")
"#,
        present: &["CodeName", "InterpName", "ForwardName", "PatchedName"],
        absent: &["CommentName", "DocName", "ProseName", "StringName", "GET"],
    });
}

#[test]
fn typescript_comments_strings_and_jsx_text_are_not_references() {
    check(&Case {
        file_type: FileType::TypeScript,
        source: r#"// CommentName
const Page = lazy(() => import('./pages/LazyName'));
const title = `StringName ${InterpName}`;
const el = <CodeName title="AttrName">TextName</CodeName>;
/* BlockName */
"#,
        present: &["LazyName", "InterpName", "CodeName"],
        absent: &[
            "CommentName",
            "StringName",
            "AttrName",
            "TextName",
            "BlockName",
        ],
    });
}

#[test]
fn c_family_comments_and_literals_are_not_references() {
    check(&Case {
        file_type: FileType::Cpp,
        source: "/* CommentName\n   ContinuedName */\n#define LN_widget \"LongName of it\"\n#define CALL(x) MacroName(x, 'Q') /* MacroComment */\nint run(void) { return CodeName(\"StringName\", 'C'); } // TrailName\n",
        present: &["CodeName", "MacroName"],
        absent: &[
            "CommentName",
            "ContinuedName",
            "StringName",
            "TrailName",
            "C",
            "LongName",
            "MacroComment",
            "Q",
        ],
    });
    check(&Case {
        file_type: FileType::ObjC,
        source: "// CommentName\n@implementation Widget\n- (void)run { [CodeName call:@\"StringName\"]; }\n@end\n",
        present: &["CodeName"],
        absent: &["CommentName", "StringName"],
    });
}

#[test]
fn other_languages_skip_comments_and_strings() {
    let cases = [
        Case {
            file_type: FileType::Go,
            source: "package main\n\n// CommentName\nfunc run() { CodeName(\"StringName\", `RawName`) }\n",
            present: &["CodeName"],
            absent: &["CommentName", "StringName", "RawName"],
        },
        Case {
            file_type: FileType::Rust,
            source: "/// DocName\nfn run() { CodeName::new(\"StringName\"); } // TrailName\n",
            present: &["CodeName"],
            absent: &["DocName", "StringName", "TrailName"],
        },
        Case {
            file_type: FileType::Java,
            source: "class Widget {\n  void run() { CodeName.call(\"StringName\"); } // TrailName\n}\n",
            present: &["CodeName"],
            absent: &["StringName", "TrailName"],
        },
        Case {
            file_type: FileType::CSharp,
            source: "class Widget {\n  void Run() { CodeName.Call($\"StringName {InterpName}\"); } // TrailName\n}\n",
            present: &["CodeName", "InterpName"],
            absent: &["StringName", "TrailName"],
        },
        Case {
            file_type: FileType::Php,
            source: "<?php\n// CommentName\nCodeName::call(\"StringName {$obj->interpName()}\", 'SingleName');\n?>\n<p>TextName</p>\n",
            present: &["CodeName", "interpName"],
            absent: &["CommentName", "StringName", "SingleName", "TextName"],
        },
        Case {
            file_type: FileType::Swift,
            source: "// CommentName\nfunc run() { CodeName.call(\"StringName \\(InterpName)\") }\n",
            present: &["CodeName", "InterpName"],
            absent: &["CommentName", "StringName"],
        },
        Case {
            file_type: FileType::Scala,
            source: "// CommentName\nobject Widget { def run() = CodeName.call(s\"StringName ${InterpName}\") }\n",
            present: &["CodeName", "InterpName"],
            absent: &["CommentName", "StringName"],
        },
        Case {
            file_type: FileType::Dart,
            source: "// CommentName\nvoid run() { CodeName.call('StringName ${InterpName}'); }\n",
            present: &["CodeName", "InterpName"],
            absent: &["CommentName", "StringName"],
        },
        Case {
            file_type: FileType::Lua,
            source: "-- CommentName\nlocal x = CodeName(\"StringName\")\n",
            present: &["CodeName"],
            absent: &["CommentName", "StringName"],
        },
        Case {
            file_type: FileType::Groovy,
            source: "// CommentName\ndef run() { CodeName.call(\"Hi ${InterpName}\", 'StringName') }\n",
            present: &["CodeName", "InterpName"],
            absent: &["CommentName", "StringName"],
        },
        Case {
            file_type: FileType::Elixir,
            source: "# CommentName\ndefmodule Widget do\n  def run, do: CodeName.call(\"StringName #{InterpName}\")\nend\n",
            present: &["CodeName", "InterpName"],
            absent: &["CommentName", "StringName"],
        },
        Case {
            file_type: FileType::Bash,
            source: "# CommentName\necho \"StringName $(CodeName)\"\n",
            present: &["CodeName"],
            absent: &["CommentName", "StringName"],
        },
        Case {
            file_type: FileType::R,
            source: "# CommentName\nx <- CodeName(\"StringName\")\n",
            present: &["CodeName"],
            absent: &["CommentName", "StringName"],
        },
        Case {
            file_type: FileType::Zig,
            source: "// CommentName\nfn run() void {\n    CodeName(\"StringName\");\n}\n",
            present: &["CodeName"],
            absent: &["CommentName", "StringName"],
        },
        Case {
            file_type: FileType::Proto,
            source: "syntax = \"proto3\";\n// CommentName\nmessage Widget { CodeName field = 1 [json_name = \"StringName\"]; }\n",
            present: &["CodeName"],
            absent: &["CommentName", "StringName"],
        },
    ];
    for case in &cases {
        check(case);
    }
}

#[test]
fn rust_format_strings_keep_their_captured_identifiers() {
    check(&Case {
        file_type: FileType::Rust,
        source: "fn report(total: usize) -> String {\n    let plain = \"{PlainName}\";\n    format!(\"{MAX_ROWS} of {total:>4} ProseName {}\", plain)\n}\n",
        present: &["MAX_ROWS"],
        absent: &["PlainName", "ProseName"],
    });
}

#[test]
fn masked_references_keep_the_original_line_as_context() {
    let source = "x = CodeName.call(\"StringName\") # trailing CommentName\n";
    let (_, refs) = parse_file_symbols(source, FileType::Ruby).unwrap();
    let code = refs.iter().find(|r| r.name == "CodeName").unwrap();
    assert_eq!(
        code.context,
        "x = CodeName.call(\"StringName\") # trailing CommentName"
    );
}

#[test]
fn masking_keeps_multibyte_text_and_line_numbers() {
    let source =
        "# Комментарий — CommentName\n# ещё\nЗначение = CodeName.call(\"строка StringName\")\n";
    let (_, refs) = parse_file_symbols(source, FileType::Ruby).unwrap();
    let code = refs.iter().find(|r| r.name == "CodeName").unwrap();
    assert_eq!(code.line, 3);
    assert!(!refs
        .iter()
        .any(|r| r.name == "CommentName" || r.name == "StringName"));
}
