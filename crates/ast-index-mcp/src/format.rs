//! Compact-text formatter for ast-index JSON responses.
//!
//! Reasoning: agent context is the scarcest resource. ast-index's
//! `--format json` produces pretty-printed JSON (whitespace + quoting) that
//! eats 2-3× the tokens of a plain-text summary carrying the same
//! information. For MCP use we default to a TOON-inspired compact format
//! and keep JSON as an opt-in via the `format: "json"` tool parameter.
//!
//! Size comparison on a typical `search` response (3 content matches):
//!   pretty JSON:   ~280 bytes, ~90 tokens
//!   compact JSON:  ~180 bytes, ~60 tokens
//!   this format:   ~120 bytes, ~35 tokens
//!
//! Shape detection is best-effort: anything we don't recognise falls
//! through to compact JSON (`serde_json::to_string`) which still beats
//! pretty JSON by ~40%.

use serde_json::Value;
use std::fmt::Write;

/// Format an ast-index JSON response as compact text.
///
/// `tool` is the MCP tool name (`search`, `usages`, etc.) and drives
/// shape-aware formatting. If the response shape doesn't match what we
/// expect for that tool, we fall back to compact JSON so no information
/// is lost.
pub fn to_compact(tool: &str, raw_json: &str) -> String {
    let value: Value = match serde_json::from_str(raw_json) {
        Ok(v) => v,
        // Not JSON (e.g. `outline` prints plain text) — pass through.
        Err(_) => return raw_json.trim_end().to_string(),
    };

    let mut out = String::with_capacity(raw_json.len() / 2);
    let rendered = match tool {
        "search" => render_search(&value, &mut out),
        "refs" => render_refs(&value, &mut out),
        "usages" | "callers" => render_ref_list(&value, &mut out),
        "symbol" | "class" | "implementations" => render_symbol_list(&value, &mut out),
        "file" | "find_file" => render_file_list(&value, &mut out),
        "stats" => render_stats(&value, &mut out),
        "changed" => render_changed(&value, &mut out),
        _ => false,
    };

    if !rendered {
        return serde_json::to_string(&value).unwrap_or_else(|_| raw_json.to_string());
    }

    let trimmed = out.trim_end().to_string();
    if trimmed.is_empty() {
        "(no results)".to_string()
    } else {
        trimmed
    }
}

fn render_search(v: &Value, out: &mut String) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };

    let mut any_section = false;

    if let Some(files) = obj.get("files").and_then(Value::as_array) {
        if !files.is_empty() {
            any_section = true;
            writeln!(out, "Files:").ok();
            for f in files {
                if let Some(s) = f.as_str() {
                    writeln!(out, "  {s}").ok();
                }
            }
            write_named_pagination_notice(obj, "files", out);
        }
    }

    if let Some(symbols) = obj.get("symbols").and_then(Value::as_array) {
        if !symbols.is_empty() {
            any_section = true;
            writeln!(out, "\nSymbols:").ok();
            for s in symbols {
                write_symbol_line(s, "  ", out);
            }
            write_named_pagination_notice(obj, "symbols", out);
        }
    }

    if let Some(refs) = obj.get("references").and_then(Value::as_array) {
        if !refs.is_empty() {
            any_section = true;
            writeln!(out, "\nReferences (usage counts):").ok();
            for r in refs {
                if let (Some(name), Some(count)) = (
                    r.get("name").and_then(Value::as_str),
                    r.get("usage_count").and_then(Value::as_i64),
                ) {
                    writeln!(out, "  {name} ×{count}").ok();
                }
            }
            write_named_pagination_notice(obj, "references", out);
        }
    }

    if let Some(content) = obj.get("content_matches").and_then(Value::as_array) {
        if !content.is_empty() {
            any_section = true;
            writeln!(out, "\nContent:").ok();
            for m in content {
                if let (Some(path), Some(line), Some(snippet)) = (
                    m.get("path").and_then(Value::as_str),
                    m.get("line").and_then(Value::as_i64),
                    m.get("content").and_then(Value::as_str),
                ) {
                    writeln!(out, "  {path}:{line}  {}", truncate(snippet, 100)).ok();
                }
            }
            write_named_pagination_notice(obj, "content_matches", out);
        }
    }

    any_section || obj.contains_key("files")
}

fn render_refs(v: &Value, out: &mut String) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };

    let mut any = false;

    if let Some(defs) = obj.get("definitions").and_then(Value::as_array) {
        if !defs.is_empty() {
            any = true;
            writeln!(out, "Definitions:").ok();
            for d in defs {
                write_symbol_line(d, "  ", out);
            }
            write_named_pagination_notice(obj, "definitions", out);
        }
    }

    if let Some(imports) = obj.get("imports").and_then(Value::as_array) {
        if !imports.is_empty() {
            any = true;
            writeln!(out, "\nImports:").ok();
            for i in imports {
                if let (Some(path), Some(line)) = (
                    i.get("path").and_then(Value::as_str),
                    i.get("line").and_then(Value::as_i64),
                ) {
                    let sig = i.get("signature").and_then(Value::as_str).unwrap_or("");
                    if sig.is_empty() {
                        writeln!(out, "  {path}:{line}").ok();
                    } else {
                        writeln!(out, "  {path}:{line}  {}", truncate(sig, 80)).ok();
                    }
                }
            }
            write_named_pagination_notice(obj, "imports", out);
        }
    }

    if let Some(usages) = obj.get("usages").and_then(Value::as_array) {
        if !usages.is_empty() {
            any = true;
            writeln!(out, "\nUsages:").ok();
            for u in usages {
                write_ref_line(u, "  ", out);
            }
            write_named_pagination_notice(obj, "usages", out);
        }
    }

    any || obj.contains_key("definitions")
}

fn render_ref_list(v: &Value, out: &mut String) -> bool {
    // Legacy responses are arrays; schema v2 wraps them in {items,pagination}.
    let Some(arr) = page_items(v) else {
        return false;
    };
    for r in arr {
        write_ref_line(r, "", out);
    }
    write_page_pagination_notice(v, out);
    true
}

fn render_symbol_list(v: &Value, out: &mut String) -> bool {
    let Some(arr) = page_items(v) else {
        return false;
    };
    for s in arr {
        write_symbol_line(s, "", out);
    }
    write_page_pagination_notice(v, out);
    true
}

fn page_items(value: &Value) -> Option<&Vec<Value>> {
    value
        .as_array()
        .or_else(|| value.as_object()?.get("items").and_then(Value::as_array))
}

fn write_page_pagination_notice(value: &Value, out: &mut String) {
    if let Some(pagination) = value.get("pagination") {
        write_pagination_notice(pagination, out);
    }
}

fn write_named_pagination_notice(
    object: &serde_json::Map<String, Value>,
    name: &str,
    out: &mut String,
) {
    if let Some(pagination) = object
        .get("pagination")
        .and_then(Value::as_object)
        .and_then(|pages| pages.get(name))
    {
        write_pagination_notice(pagination, out);
    }
}

fn write_pagination_notice(pagination: &Value, out: &mut String) {
    if pagination.get("truncated").and_then(Value::as_bool) != Some(true) {
        return;
    }
    let returned = pagination
        .get("returned")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = pagination
        .get("total")
        .and_then(Value::as_u64)
        .unwrap_or(returned);
    writeln!(
        out,
        "  … truncated: showing {returned} of {total}; raise limit to {total}"
    )
    .ok();
}

fn render_file_list(v: &Value, out: &mut String) -> bool {
    let Some(arr) = v.as_array() else {
        return false;
    };
    for f in arr {
        if let Some(s) = f.as_str() {
            writeln!(out, "{s}").ok();
        }
    }
    true
}

fn render_stats(v: &Value, out: &mut String) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };

    let project = obj.get("project").and_then(Value::as_str).unwrap_or("?");
    let db_size = obj
        .get("db_size_bytes")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let db_path = obj.get("db_path").and_then(Value::as_str).unwrap_or("");

    writeln!(out, "project: {project}").ok();
    if let Some(stats) = obj.get("stats").and_then(Value::as_object) {
        // Known counters first (stable order, skip zeros to save tokens).
        let keys = [
            "file_count",
            "symbol_count",
            "refs_count",
            "module_count",
            "xml_usages_count",
            "resources_count",
            "storyboard_usages_count",
            "ios_assets_count",
        ];
        for k in keys {
            if let Some(n) = stats.get(k).and_then(Value::as_i64) {
                if n > 0 {
                    writeln!(out, "{k}: {n}").ok();
                }
            }
        }
    }
    if db_size > 0 {
        writeln!(out, "db_size_mb: {:.2}", db_size as f64 / 1024.0 / 1024.0).ok();
    }
    if !db_path.is_empty() {
        writeln!(out, "db_path: {db_path}").ok();
    }
    true
}

fn render_changed(v: &Value, out: &mut String) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };
    if obj.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return false;
    }

    let (Some(vcs), Some(base), Some(head), Some(scope_value), Some(changes)) = (
        obj.get("vcs").and_then(Value::as_str),
        obj.get("base").and_then(Value::as_str),
        obj.get("head").and_then(Value::as_str),
        obj.get("scope"),
        obj.get("changes").and_then(Value::as_array),
    ) else {
        return false;
    };
    let scope = match scope_value {
        Value::Null => ".",
        Value::String(scope) => scope,
        _ => return false,
    };

    let mut lines = Vec::with_capacity(changes.len());
    for change in changes {
        let (Some(status), Some(path)) = (
            change.get("status").and_then(Value::as_str),
            change.get("path").and_then(Value::as_str),
        ) else {
            return false;
        };
        let line = match status {
            "A" | "M" | "D" => format!("{status} {}", escape_path(path)),
            "R" => {
                let Some(old_path) = change.get("old_path").and_then(Value::as_str) else {
                    return false;
                };
                format!("R {} -> {}", escape_path(old_path), escape_path(path))
            }
            _ => return false,
        };
        lines.push(line);
    }

    let vcs = escape_path(vcs);
    let base = escape_path(base);
    let head = escape_path(head);
    let scope = escape_path(scope);
    writeln!(
        out,
        "Changed files from merge-base({base}, {head}) to {head} ({vcs}, scope={scope})"
    )
    .ok();
    if lines.is_empty() {
        writeln!(out, "(no changes)").ok();
    } else {
        for line in lines {
            writeln!(out, "{line}").ok();
        }
    }
    true
}

fn escape_path(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for character in path.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\0' => escaped.push_str("\\0"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{1b}' => escaped.push_str("\\x1b"),
            value if value.is_control() => {
                write!(escaped, "\\u{{{:x}}}", value as u32).ok();
            }
            value => escaped.push(value),
        }
    }
    escaped
}

fn write_symbol_line(s: &Value, indent: &str, out: &mut String) {
    let name = s
        .get("qualified_name")
        .and_then(Value::as_str)
        .or_else(|| s.get("name").and_then(Value::as_str))
        .unwrap_or("?");
    let kind = s.get("kind").and_then(Value::as_str).unwrap_or("?");
    let path = s.get("path").and_then(Value::as_str).unwrap_or("?");
    let line = s.get("line").and_then(Value::as_i64).unwrap_or(0);
    writeln!(out, "{indent}{name} [{kind}] {path}:{line}").ok();

    if let Some(sig) = s.get("signature").and_then(Value::as_str) {
        if !sig.is_empty() {
            writeln!(out, "{indent}  {}", truncate(sig, 80)).ok();
        }
    }
}

fn write_ref_line(r: &Value, indent: &str, out: &mut String) {
    let path = r.get("path").and_then(Value::as_str).unwrap_or("?");
    let line = r.get("line").and_then(Value::as_i64).unwrap_or(0);
    writeln!(out, "{indent}{path}:{line}").ok();

    if let Some(ctx) = r
        .get("context")
        .and_then(Value::as_str)
        .or_else(|| r.get("content").and_then(Value::as_str))
    {
        if !ctx.is_empty() {
            writeln!(out, "{indent}  {}", truncate(ctx, 80)).ok();
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- search shaper ---

    #[test]
    fn search_renders_all_four_sections_compactly() {
        let json = r#"{
            "files": ["src/a.rs", "src/b.rs"],
            "symbols": [
                {"name":"Foo","kind":"class","path":"src/a.rs","line":10}
            ],
            "references": [
                {"name":"Foo","usage_count":3}
            ],
            "content_matches": [
                {"path":"src/c.rs","line":42,"content":"let x = Foo::new();"}
            ]
        }"#;
        let out = to_compact("search", json);
        assert!(out.contains("Files:"));
        assert!(out.contains("src/a.rs"));
        assert!(out.contains("Symbols:"));
        assert!(out.contains("Foo [class] src/a.rs:10"));
        assert!(out.contains("References (usage counts):"));
        assert!(out.contains("Foo ×3"));
        assert!(out.contains("Content:"));
        assert!(out.contains("src/c.rs:42"));
    }

    #[test]
    fn search_with_empty_arrays_returns_no_results() {
        let json = r#"{
            "files":[],"symbols":[],"references":[],"content_matches":[]
        }"#;
        let out = to_compact("search", json);
        assert_eq!(out, "(no results)");
    }

    #[test]
    fn search_skips_missing_sections() {
        let json = r#"{"files": ["only/a.rs"]}"#;
        let out = to_compact("search", json);
        assert!(out.contains("Files:"));
        assert!(out.contains("only/a.rs"));
        assert!(!out.contains("Symbols:"));
        assert!(!out.contains("Content:"));
    }

    // --- refs shaper ---

    #[test]
    fn refs_renders_definitions_imports_usages() {
        let json = r#"{
            "definitions": [
                {"name":"Foo","kind":"class","path":"a.rs","line":5}
            ],
            "imports": [
                {"path":"b.rs","line":1,"signature":"use crate::Foo;"}
            ],
            "usages": [
                {"path":"c.rs","line":42,"context":"Foo::bar()"}
            ]
        }"#;
        let out = to_compact("refs", json);
        assert!(out.contains("Definitions:"));
        assert!(out.contains("Foo [class] a.rs:5"));
        assert!(out.contains("Imports:"));
        assert!(out.contains("b.rs:1"));
        assert!(out.contains("use crate::Foo;"));
        assert!(out.contains("Usages:"));
        assert!(out.contains("c.rs:42"));
        assert!(out.contains("Foo::bar()"));
    }

    // --- usages / callers (ref list) ---

    #[test]
    fn usages_renders_array_of_refs() {
        let json = r#"[
            {"path":"src/a.rs","line":10,"context":"foo();"},
            {"path":"src/b.rs","line":20,"context":"foo()"}
        ]"#;
        let out = to_compact("usages", json);
        assert!(out.contains("src/a.rs:10"));
        assert!(out.contains("src/b.rs:20"));
        assert!(out.contains("foo();"));
    }

    #[test]
    fn usages_renders_page_v2_and_truncation_notice() {
        let json = r#"{
            "schema_version":2,
            "items":[{"path":"src/a.rs","line":10,"context":"foo();"}],
            "pagination":{"total":4,"returned":1,"truncated":true,"limit":1}
        }"#;
        let out = to_compact("usages", json);
        assert!(out.contains("src/a.rs:10"));
        assert!(out.contains("showing 1 of 4"));
        assert!(out.contains("raise limit to 4"));
    }

    #[test]
    fn callers_uses_same_shaper_as_usages() {
        let json = r#"[{"path":"a.rs","line":1,"context":"foo()"}]"#;
        assert_eq!(to_compact("usages", json), to_compact("callers", json));
    }

    #[test]
    fn callers_page_v2_renders_content_field() {
        let json = r#"{
            "schema_version":2,
            "items":[{"path":"a.rs","line":1,"content":"foo()"}],
            "pagination":{"total":2,"returned":1,"truncated":true,"limit":1}
        }"#;
        let out = to_compact("callers", json);
        assert!(out.contains("a.rs:1"));
        assert!(out.contains("foo()"));
        assert!(out.contains("showing 1 of 2"));
    }

    // --- symbol / class / implementations (symbol list) ---

    #[test]
    fn symbol_renders_symbol_lines_with_signatures() {
        let json = r#"[
            {"name":"Foo","kind":"class","path":"a.rs","line":5,
             "signature":"struct Foo<T>"},
            {"name":"bar","kind":"function","path":"b.rs","line":10}
        ]"#;
        let out = to_compact("symbol", json);
        assert!(out.contains("Foo [class] a.rs:5"));
        assert!(out.contains("struct Foo<T>"));
        assert!(out.contains("bar [function] b.rs:10"));
    }

    #[test]
    fn symbol_renders_page_v2_without_losing_pagination() {
        let json = r#"{
            "schema_version":2,
            "items":[{"name":"Foo","kind":"class","path":"a.rs","line":5}],
            "pagination":{"total":3,"returned":1,"truncated":true,"limit":1}
        }"#;
        let out = to_compact("symbol", json);
        assert!(out.contains("Foo [class] a.rs:5"));
        assert!(out.contains("showing 1 of 3"));
    }

    #[test]
    fn implementations_uses_same_shape_as_symbol() {
        let json = r#"[{"name":"X","kind":"class","path":"a.rs","line":1}]"#;
        let a = to_compact("symbol", json);
        let b = to_compact("implementations", json);
        let c = to_compact("class", json);
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    // --- find_file ---

    #[test]
    fn find_file_renders_path_list() {
        let json = r#"["src/a.rs","src/b.rs","tests/c.rs"]"#;
        let out = to_compact("find_file", json);
        assert!(out.contains("src/a.rs"));
        assert!(out.contains("src/b.rs"));
        assert!(out.contains("tests/c.rs"));
    }

    // --- stats ---

    #[test]
    fn stats_renders_project_counts_and_db_size() {
        let json = r#"{
            "project": "Rust",
            "stats": {
                "file_count": 100, "symbol_count": 5000,
                "refs_count": 30000, "module_count": 0,
                "xml_usages_count": 0, "resources_count": 0,
                "storyboard_usages_count": 0, "ios_assets_count": 0
            },
            "db_size_bytes": 1048576,
            "db_path": "/tmp/index.db"
        }"#;
        let out = to_compact("stats", json);
        assert!(out.contains("project: Rust"));
        assert!(out.contains("file_count: 100"));
        assert!(out.contains("symbol_count: 5000"));
        assert!(out.contains("refs_count: 30000"));
        // Zero-counts should be skipped to save tokens
        assert!(!out.contains("module_count: 0"));
        assert!(!out.contains("xml_usages_count"));
        assert!(out.contains("db_size_mb: 1.00"));
        assert!(out.contains("db_path: /tmp/index.db"));
    }

    // --- changed ---

    #[test]
    fn changed_renders_schema_v1_with_all_statuses_and_rename_paths() {
        let json = r#"{
            "schema_version": 1,
            "vcs": "git",
            "base": "origin/main",
            "head": "HEAD",
            "scope": "crates/ast-index-mcp",
            "changes": [
                {"status": "A", "path": "src/new.rs"},
                {"status": "M", "path": "src/lib.rs"},
                {"status": "D", "path": "src/old.rs"},
                {"status": "R", "path": "src/current.rs", "old_path": "src/former.rs"}
            ]
        }"#;
        assert_eq!(
            to_compact("changed", json),
            concat!(
                "Changed files from merge-base(origin/main, HEAD) to HEAD ",
                "(git, scope=crates/ast-index-mcp)\n",
                "A src/new.rs\n",
                "M src/lib.rs\n",
                "D src/old.rs\n",
                "R src/former.rs -> src/current.rs"
            )
        );
    }

    #[test]
    fn changed_empty_branch_is_explicit() {
        let json = r#"{
            "schema_version": 1,
            "vcs": "arc",
            "base": "trunk",
            "head": "HEAD",
            "scope": null,
            "changes": []
        }"#;
        assert_eq!(
            to_compact("changed", json),
            concat!(
                "Changed files from merge-base(trunk, HEAD) to HEAD (arc, scope=.)\n",
                "(no changes)"
            )
        );
    }

    #[test]
    fn changed_escapes_path_controls_but_preserves_unicode() {
        let json = r#"{
            "schema_version": 1,
            "vcs": "git\rcli",
            "base": "main\nbranch",
            "head": "HEAD\tref",
            "scope": "nested\t雪",
            "changes": [
                {"status": "M", "path": "src/tab\t雪.rs"},
                {
                    "status": "R",
                    "path": "src/new\u001b雪.rs",
                    "old_path": "src/old\nname.rs"
                }
            ]
        }"#;
        assert_eq!(
            to_compact("changed", json),
            concat!(
                "Changed files from merge-base(main\\nbranch, HEAD\\tref) to ",
                "HEAD\\tref (git\\rcli, scope=nested\\t雪)\n",
                "M src/tab\\t雪.rs\n",
                "R src/old\\nname.rs -> src/new\\x1b雪.rs"
            )
        );
    }

    #[test]
    fn changed_unknown_schema_falls_back_without_losing_json() {
        let json = r#"{"schema_version":2,"changes":[]}"#;
        let rendered: Value = serde_json::from_str(&to_compact("changed", json)).unwrap();
        let original: Value = serde_json::from_str(json).unwrap();
        assert_eq!(rendered, original);
    }

    // --- fall-through behaviour ---

    #[test]
    fn unknown_tool_falls_through_to_compact_json() {
        // Tools without a shaper fall back to serde_json::to_string (compact)
        let json = r#"{"foo": "bar"}"#;
        let out = to_compact("xyz_no_shaper", json);
        assert!(out.contains("foo"));
        assert!(out.contains("bar"));
        // Compact (not pretty-printed)
        assert!(!out.contains("\n  "));
    }

    #[test]
    fn non_json_input_passes_through_trim_end_only() {
        // outline returns plain text with possible indentation that
        // matters — leading whitespace is preserved, only trailing
        // newlines are stripped.
        let plain = "  Foo [class] file.rs:10\n\n";
        let out = to_compact("outline", plain);
        assert_eq!(out, "  Foo [class] file.rs:10");
    }

    // --- truncate helper ---

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string_appends_ellipsis() {
        assert_eq!(truncate("0123456789abcdef", 10), "0123456789…");
    }

    #[test]
    fn truncate_preserves_unicode_chars() {
        assert_eq!(truncate("πρακτικά", 5), "πρακτ…");
    }
}
