//! File operation commands
//!
//! Commands for working with files:
//! - file: Find files by pattern
//! - outline: Show file symbols outline
//! - imports: Show file imports
//! - api: Show module public API
//! - changed: See [`super::changed`]

use std::path::{Path, PathBuf};

use anyhow::Result;
use colored::Colorize;
use regex::Regex;
use rusqlite::OptionalExtension;

use crate::db::SymbolKind;

use super::{relative_path, search_files};
use crate::db;

fn print_minified_notice() {
    println!(
        "  Skipped: minified file, not analysed (set {}=0 to include minified files).",
        crate::minified::SKIP_ENV
    );
}

/// Find files by pattern
pub fn cmd_file(root: &Path, pattern: &str, exact: bool, limit: usize, format: &str) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db_leased(root)?;

    let search_pattern = if exact {
        pattern.to_string()
    } else {
        pattern.to_string()
    };
    let files = db::find_files_with_roots(&conn, &search_pattern, limit)?;
    let resolver = super::PathResolver::try_from_conn(root, &conn)?;
    let files: Vec<String> = files
        .into_iter()
        .filter(|f| resolver.matches_filter(f.root_path.as_deref()))
        .map(|file| resolver.resolve_with_root(&file.path, file.root_path.as_deref()))
        .collect();

    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&files)?);
        return Ok(());
    }

    println!("{}", format!("Files matching '{}':", pattern).bold());

    for path in &files {
        println!("  {}", path);
    }

    if files.is_empty() {
        println!("  No files found.");
    }

    Ok(())
}

/// Version of the `outline --format json` document. Outline reads one file
/// and has no limit, so it has its own schema instead of the paginated one.
pub const OUTLINE_JSON_SCHEMA_VERSION: u8 = 1;

/// One outline row of `outline --format json`.
#[derive(Debug, serde::Serialize)]
struct OutlineRow<'a> {
    name: &'a str,
    kind: &'a str,
    line: usize,
    /// Last line of the definition; `null` where the parser reports none.
    end_line: Option<usize>,
    /// Columns folded into a schema table's row (absent with `--full`).
    #[serde(skip_serializing_if = "Option::is_none")]
    columns: Option<usize>,
}

/// Why an outline lists no symbols without having parsed the file.
#[derive(Clone, Copy)]
enum OutlineSkip {
    NotFound,
    Minified,
    Unsupported,
}

impl OutlineSkip {
    fn as_str(self) -> &'static str {
        match self {
            OutlineSkip::NotFound => "not_found",
            OutlineSkip::Minified => "minified",
            OutlineSkip::Unsupported => "unsupported",
        }
    }
}

/// Symbols of `content` as outline shows them, or `None` for an extension no
/// parser handles. Kinds the outline of that language leaves out are removed.
fn outline_symbols(
    content: &str,
    ext: &str,
    file: &str,
) -> Result<Option<Vec<crate::parsers::ParsedSymbol>>> {
    use crate::parsers::FileType;
    let (file_type, skip_kinds): (FileType, &[SymbolKind]) = match ext {
        "pm" | "pl" | "t" => (FileType::Perl, &[SymbolKind::Import]),
        "py" => (
            FileType::Python,
            &[SymbolKind::Import, SymbolKind::Property],
        ),
        // .h may be C++, C or ObjC — sniff content
        "h" if crate::parsers::detect_h_file_objc(content) => {
            (FileType::ObjC, &[SymbolKind::Import])
        }
        "kts" => (FileType::Kotlin, &[SymbolKind::Import]),
        "dart" => (FileType::Dart, &[SymbolKind::Import, SymbolKind::Property]),
        "java" => (
            FileType::Java,
            &[SymbolKind::Import, SymbolKind::Annotation],
        ),
        "proto" | "bsl" | "os" => (
            FileType::from_extension(ext).unwrap_or(FileType::Proto),
            &[],
        ),
        "m" => (FileType::detect_m_file_type(content), &[SymbolKind::Import]),
        "mm" => (FileType::ObjC, &[SymbolKind::Import]),
        _ => match FileType::from_extension(ext) {
            Some(file_type) => (file_type, &[SymbolKind::Import]),
            None => return Ok(None),
        },
    };
    let mut symbols = crate::parsers::parse_file_symbols_only(content, file_type)?;
    if file_type == FileType::TypeScript {
        // Name the anonymous default export after the file exactly as the
        // index does.
        crate::parsers::treesitter::typescript::name_default_export(&mut symbols, file);
    }
    symbols.retain(|sym| !skip_kinds.contains(&sym.kind));
    // Parsers emit definitions in query-match order, which can put a method
    // before the class around it; an outline reads top to bottom.
    symbols.sort_by_key(|sym| (sym.line, std::cmp::Reverse(sym.end_line)));
    Ok(Some(symbols))
}

/// Outline rows of `symbols`; unless `full`, a schema dump's columns are
/// left out and counted on their table's row.
fn outline_rows(symbols: &[crate::parsers::ParsedSymbol], full: bool) -> Vec<OutlineRow<'_>> {
    symbols
        .iter()
        .filter(|sym| full || sym.kind != SymbolKind::Column)
        .map(|sym| OutlineRow {
            name: &sym.name,
            kind: sym.kind.as_str(),
            line: sym.line,
            end_line: sym.end_line,
            columns: (!full && sym.kind == SymbolKind::Table).then(|| table_columns(sym, symbols)),
        })
        .collect()
}

fn print_outline_json(file: &str, rows: &[OutlineRow], skipped: Option<OutlineSkip>) -> Result<()> {
    let mut document = serde_json::json!({
        "schema_version": OUTLINE_JSON_SCHEMA_VERSION,
        "file": file,
        "symbols": rows,
    });
    if let Some(skipped) = skipped {
        document["skipped"] = serde_json::Value::from(skipped.as_str());
    }
    println!("{}", serde_json::to_string_pretty(&document)?);
    Ok(())
}

/// Show file symbols outline. The columns of a schema dump (Rails
/// `db/schema.rb`) are folded into a count on their table's row unless `full`
/// is set: thousands of column rows would bury the tables.
pub fn cmd_outline(root: &Path, file: &str, full: bool, format: &str) -> Result<()> {
    let json = format == "json";
    let file_path = if file.starts_with('/') {
        PathBuf::from(file)
    } else {
        root.join(file)
    };

    if !file_path.exists() {
        if json {
            return print_outline_json(file, &[], Some(OutlineSkip::NotFound));
        }
        println!("{}", format!("File not found: {}", file).red());
        return Ok(());
    }

    let header = format!("Outline of {}:", file);
    if crate::minified::skip(&file_path, None) {
        if json {
            return print_outline_json(file, &[], Some(OutlineSkip::Minified));
        }
        println!("{}", header.bold());
        print_minified_notice();
        return Ok(());
    }

    let content = std::fs::read_to_string(&file_path)?;
    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let symbols = outline_symbols(&content, ext, file)?;

    let rows = symbols
        .as_deref()
        .map(|symbols| outline_rows(symbols, full));

    if json {
        let skipped = rows.is_none().then_some(OutlineSkip::Unsupported);
        return print_outline_json(file, rows.as_deref().unwrap_or(&[]), skipped);
    }

    println!("{}", header.bold());
    let Some(rows) = rows else {
        println!("  Unsupported file type: .{}", ext);
        println!("  No symbols found.");
        return Ok(());
    };
    let mut folded = (0, 0);
    for row in &rows {
        let position = match row.end_line {
            Some(end) if end > row.line => format!(":{}-{}", row.line, end),
            _ => format!(":{}", row.line),
        };
        let mut text = format!("  {} {} [{}]", position.dimmed(), row.name.cyan(), row.kind);
        if let Some(columns) = row.columns.filter(|&columns| columns > 0) {
            let noun = if columns == 1 { "column" } else { "columns" };
            text.push_str(&format!(" {columns} {noun}"));
            folded = (folded.0 + columns, folded.1 + 1);
        }
        println!("{text}");
    }
    if rows.is_empty() {
        println!("  No symbols found.");
    }
    if folded.0 > 0 {
        println!(
            "  {}",
            format!(
                "{} columns folded into {} tables: --full lists them, or symbol --type column --pattern '<table>.*'.",
                folded.0, folded.1
            )
            .dimmed()
        );
    }
    Ok(())
}

/// Columns of a schema dump's `table` symbol: the `table.column` symbols
/// inside its `create_table` block.
fn table_columns(
    table: &crate::parsers::ParsedSymbol,
    symbols: &[crate::parsers::ParsedSymbol],
) -> usize {
    let prefix = format!("{}.", table.name);
    let end = table.end_line.unwrap_or(table.line);
    symbols
        .iter()
        .filter(|sym| {
            sym.kind == SymbolKind::Column
                && sym.name.starts_with(&prefix)
                && (table.line..=end).contains(&sym.line)
        })
        .count()
}

/// Show file imports
pub fn cmd_imports(root: &Path, file: &str) -> Result<()> {
    let file_path = if file.starts_with('/') {
        PathBuf::from(file)
    } else {
        root.join(file)
    };

    if !file_path.exists() {
        println!("{}", format!("File not found: {}", file).red());
        return Ok(());
    }

    let header = format!("Imports in {}:", file);
    if crate::minified::skip(&file_path, None) {
        println!("{}", header.bold());
        print_minified_notice();
        return Ok(());
    }

    let content = std::fs::read_to_string(&file_path)?;

    // Detect file type by extension
    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let is_perl = ext == "pm" || ext == "pl" || ext == "t";
    let is_python = ext == "py";
    let is_go = ext == "go";
    let is_cpp = ext == "cpp" || ext == "cc" || ext == "c" || ext == "hpp" || ext == "h";
    let is_typescript =
        crate::parsers::FileType::from_extension(ext) == Some(crate::parsers::FileType::TypeScript);

    println!("{}", header.bold());

    let mut imports: Vec<String> = vec![];

    if is_typescript {
        imports = crate::parsers::treesitter::typescript::import_declarations(&content)?;
    } else if is_perl {
        // Perl: use Module; or require Module;
        let use_re = Regex::new(r"^\s*(use|require)\s+([A-Za-z][A-Za-z0-9_:]*)")?;
        for line in content.lines() {
            if let Some(caps) = use_re.captures(line) {
                let keyword = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let module = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                // Skip pragmas
                if module != "strict"
                    && module != "warnings"
                    && module != "utf8"
                    && module != "constant"
                    && module != "base"
                    && module != "parent"
                    && !module.starts_with("v5")
                    && !module.starts_with("5.")
                {
                    imports.push(format!("{} {}", keyword, module));
                }
            }
        }
    } else if is_python {
        // Python: import module or from module import something
        let import_re = Regex::new(r"^import\s+([A-Za-z_][A-Za-z0-9_\.]*)")?;
        let from_re = Regex::new(r"^from\s+([A-Za-z_][A-Za-z0-9_\.]*)\s+import\s+(.+)")?;
        for line in content.lines() {
            if let Some(caps) = from_re.captures(line) {
                let module = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let what = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                imports.push(format!("from {} import {}", module, what));
            } else if let Some(caps) = import_re.captures(line) {
                let module = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                imports.push(format!("import {}", module));
            }
        }
    } else if is_go {
        // Go: import "module" or import ( "module1" "module2" )
        let single_import_re = Regex::new(r#"^import\s+"([^"]+)""#)?;
        let import_block_start = Regex::new(r"^import\s*\(")?;
        let import_line_re = Regex::new(r#"^\s*(?:[a-zA-Z_][a-zA-Z0-9_]*\s+)?"([^"]+)""#)?;

        let mut in_import_block = false;
        for line in content.lines() {
            if in_import_block {
                if line.trim() == ")" {
                    in_import_block = false;
                } else if let Some(caps) = import_line_re.captures(line) {
                    let module = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    imports.push(module.to_string());
                }
            } else if import_block_start.is_match(line) {
                in_import_block = true;
            } else if let Some(caps) = single_import_re.captures(line) {
                let module = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                imports.push(module.to_string());
            }
        }
    } else if is_cpp {
        // C++: #include <header> or #include "header"
        let include_re = Regex::new(r#"^\s*#include\s*[<"]([^>"]+)[>"]"#)?;
        for line in content.lines() {
            if let Some(caps) = include_re.captures(line) {
                let header = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                imports.push(header.to_string());
            }
        }
    } else {
        // Kotlin/Java/Swift: import statement
        let import_re = Regex::new(r"(?m)^import\s+(.+)")?;
        for line in content.lines() {
            if let Some(caps) = import_re.captures(line) {
                imports.push(caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string());
            }
        }
    }

    if imports.is_empty() {
        println!("  No imports found.");
    } else {
        for imp in &imports {
            println!("  {}", imp);
        }
        println!("\n  Total: {} imports", imports.len());
    }

    Ok(())
}

/// Show module public API
pub fn cmd_api(root: &Path, module_path: &str, limit: usize) -> Result<()> {
    let mut module_dir = root.join(module_path);

    // If path not found, try converting dots to slashes (module name → path)
    if !module_dir.exists() && module_path.contains('.') {
        let converted = module_path.replace('.', "/");
        let alt = root.join(&converted);
        if alt.exists() {
            module_dir = alt;
        }
    }

    // Also try looking up module path from DB
    if !module_dir.exists() {
        if let Some(_cache_lease) = crate::db::acquire_project_lease_if_initialized(root)? {
            let conn = crate::db::open_db_leased(root)?;
            let db_path: Option<String> = conn
                .query_row(
                    "SELECT path FROM modules WHERE name = ?1",
                    rusqlite::params![module_path],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(p) = db_path {
                let alt = root.join(&p);
                if alt.exists() {
                    module_dir = alt;
                }
            }
        }
    }

    if !module_dir.exists() {
        println!("{}", format!("Module not found: {}", module_path).red());
        return Ok(());
    }

    // Find public classes, interfaces, functions in the module
    let pattern = r"(public\s+)?(class|interface|object|fun)\s+\w+";

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files(
        &module_dir,
        pattern,
        &["kt", "java"],
        |path, line_num, line| {
            if items.len() >= limit {
                return;
            }

            // Skip private/internal
            if line.contains("private ") || line.contains("internal ") {
                return;
            }

            let rel_path = relative_path(root, path);
            let content: String = line.trim().chars().take(100).collect();
            items.push((rel_path, line_num, content));
        },
    )?;

    if items.len() < limit {
        items.extend(swift_public_api(root, &module_dir, limit - items.len())?);
    }

    println!(
        "{}",
        format!("Public API of '{}' ({}):", module_path, items.len()).bold()
    );

    for (path, line_num, content) in &items {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    if items.is_empty() {
        println!("  No public API found.");
    }

    Ok(())
}

/// Swift declarations explicitly marked `public`/`open`, from the index.
/// Swift defaults to `internal`, so nothing else is visible outside the module.
fn swift_public_api(
    root: &Path,
    module_dir: &Path,
    limit: usize,
) -> Result<Vec<(String, usize, String)>> {
    let Some(_cache_lease) = crate::db::acquire_project_lease_if_initialized(root)? else {
        return Ok(vec![]);
    };
    let conn = crate::db::open_db_leased(root)?;
    let dir = module_dir
        .strip_prefix(root)
        .unwrap_or(module_dir)
        .to_string_lossy();
    let mut items = vec![];
    for sym in crate::db::find_symbols_under(&conn, &dir, ".swift")? {
        if items.len() >= limit {
            break;
        }
        let signature = sym.signature.unwrap_or_default();
        if sym.kind != "import" && is_swift_public_declaration(&signature) {
            items.push((
                sym.path,
                sym.line as usize,
                signature.chars().take(100).collect(),
            ));
        }
    }
    Ok(items)
}

/// Whether the modifiers before the declaration keyword include `public`/`open`
/// (`@MainActor public final class X`, `open override func f()`).
fn is_swift_public_declaration(signature: &str) -> bool {
    const DECLARATION_KEYWORDS: &[&str] = &[
        "class",
        "struct",
        "enum",
        "protocol",
        "actor",
        "extension",
        "func",
        "init",
        "var",
        "let",
        "typealias",
        "subscript",
        "case",
    ];
    signature
        .split_whitespace()
        .filter(|token| !token.starts_with('@'))
        .take_while(|token| !DECLARATION_KEYWORDS.contains(token))
        .any(|token| token == "public" || token == "open")
}

/// Compatibility entry point for the legacy changed command API.
#[deprecated(note = "use commands::changed::cmd_changed for structured changed-file output")]
pub fn cmd_changed(root: &Path, base: &str) -> Result<()> {
    super::changed::cmd_changed(root, Some(base), 30_000, false, "text")
}

/// Compatibility entry point for callers that detect the repository VCS.
#[deprecated(note = "the changed command now detects its VCS internally")]
pub fn detect_vcs(root: &Path) -> &'static str {
    super::changed::detect_vcs_compat(root)
}

/// Compatibility entry point for callers that detect the default Git base.
#[deprecated(note = "omit --base to let commands::changed::cmd_changed detect the Git base")]
pub fn detect_git_default_branch(root: &Path) -> &'static str {
    super::changed::detect_git_default_branch_compat(root)
}

#[cfg(test)]
mod tests {
    #[test]
    fn swift_public_declaration_needs_public_or_open_modifier() {
        use super::is_swift_public_declaration as public;
        assert!(public("public final class Router: NSObject {"));
        assert!(public("@MainActor open func show()"));
        assert!(public("public private(set) var state: State"));
        assert!(!public("final class Internal {"));
        assert!(!public("func open(url: URL)"));
        assert!(!public("private let open = true"));
    }
}
