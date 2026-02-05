//! File operation commands
//!
//! Commands for working with files:
//! - file: Find files by pattern
//! - outline: Show file symbols outline
//! - imports: Show file imports
//! - api: Show module public API
//! - changed: Show changed symbols in git diff

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use colored::Colorize;
use regex::Regex;

use crate::db::SymbolKind;

use crate::db;
use super::{search_files, relative_path};

/// Find files by pattern
pub fn cmd_file(root: &Path, pattern: &str, exact: bool, limit: usize) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;

    let search_pattern = if exact { pattern.to_string() } else { pattern.to_string() };
    let files = db::find_files(&conn, &search_pattern, limit)?;

    println!("{}", format!("Files matching '{}':", pattern).bold());

    for path in &files {
        println!("  {}", path);
    }

    if files.is_empty() {
        println!("  No files found.");
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Show file symbols outline
pub fn cmd_outline(root: &Path, file: &str) -> Result<()> {
    let start = Instant::now();

    // Find the file
    let file_path = if file.starts_with('/') {
        PathBuf::from(file)
    } else {
        root.join(file)
    };

    if !file_path.exists() {
        println!("{}", format!("File not found: {}", file).red());
        return Ok(());
    }

    let content = std::fs::read_to_string(&file_path)?;

    // Detect file type
    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let is_perl = ext == "pm" || ext == "pl" || ext == "t";
    let is_python = ext == "py";
    let is_go = ext == "go";
    let is_cpp = ext == "cpp" || ext == "cc" || ext == "c" || ext == "hpp" || ext == "h";

    println!("{}", format!("Outline of {}:", file).bold());

    let mut found = false;

    if is_perl {
        // Perl patterns
        let package_re = Regex::new(r"^\s*package\s+([A-Za-z_][A-Za-z0-9_:]*)\s*;")?;
        let sub_re = Regex::new(r"^\s*sub\s+([A-Za-z_][A-Za-z0-9_]*)")?;
        let constant_re = Regex::new(r"^\s*use\s+constant\s+([A-Z_][A-Z0-9_]*)\s*=>")?;
        let our_re = Regex::new(r"^\s*our\s+([\$@%][A-Za-z_][A-Za-z0-9_]*)")?;

        for (line_num, line) in content.lines().enumerate() {
            let line_num = line_num + 1;

            if let Some(caps) = package_re.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                println!("  {} {} [package]", format!(":{}", line_num).dimmed(), name.cyan());
                found = true;
            }

            if let Some(caps) = sub_re.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                println!("  {} {} [sub]", format!(":{}", line_num).dimmed(), name);
                found = true;
            }

            if let Some(caps) = constant_re.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                println!("  {} {} [constant]", format!(":{}", line_num).dimmed(), name);
                found = true;
            }

            if let Some(caps) = our_re.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                println!("  {} {} [our]", format!(":{}", line_num).dimmed(), name);
                found = true;
            }
        }
    } else if is_python {
        // Python patterns
        let class_re = Regex::new(r"^class\s+([A-Za-z_][A-Za-z0-9_]*)")?;
        let func_re = Regex::new(r"^(async\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)")?;

        for (line_num, line) in content.lines().enumerate() {
            let line_num = line_num + 1;

            if let Some(caps) = class_re.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                println!("  {} {} [class]", format!(":{}", line_num).dimmed(), name.cyan());
                found = true;
            }

            if let Some(caps) = func_re.captures(line) {
                let is_async = caps.get(1).is_some();
                let name = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                let kind = if is_async { "async function" } else { "function" };
                println!("  {} {} [{}]", format!(":{}", line_num).dimmed(), name, kind);
                found = true;
            }
        }
    } else if is_go {
        // Go patterns
        let package_re = Regex::new(r"^package\s+([a-z][a-z0-9_]*)")?;
        let struct_re = Regex::new(r"^type\s+([A-Z][a-zA-Z0-9_]*)\s+struct")?;
        let interface_re = Regex::new(r"^type\s+([A-Z][a-zA-Z0-9_]*)\s+interface")?;
        let func_re = Regex::new(r"^func\s+(?:\([^)]+\)\s*)?([A-Za-z_][A-Za-z0-9_]*)\s*\(")?;

        for (line_num, line) in content.lines().enumerate() {
            let line_num = line_num + 1;

            if let Some(caps) = package_re.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                println!("  {} {} [package]", format!(":{}", line_num).dimmed(), name.cyan());
                found = true;
            }

            if let Some(caps) = struct_re.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                println!("  {} {} [struct]", format!(":{}", line_num).dimmed(), name.cyan());
                found = true;
            }

            if let Some(caps) = interface_re.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                println!("  {} {} [interface]", format!(":{}", line_num).dimmed(), name.cyan());
                found = true;
            }

            if let Some(caps) = func_re.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                println!("  {} {} [func]", format!(":{}", line_num).dimmed(), name);
                found = true;
            }
        }
    } else if is_cpp {
        // C++ patterns
        let namespace_re = Regex::new(r"^namespace\s+([\w:]+)\s*\{")?;
        let class_re = Regex::new(r"^(?:class|struct)\s+([A-Z][a-zA-Z0-9_]*)")?;
        let func_re = Regex::new(r"^(?:[\w:]+(?:<[^>]*>)?\s*[*&]?\s+)?([A-Z][a-zA-Z0-9_]*::)?([A-Za-z_][A-Za-z0-9_]*)\s*\([^)]*\)\s*(?:const)?\s*(?:override)?\s*\{")?;

        for (line_num, line) in content.lines().enumerate() {
            let line_num = line_num + 1;

            if let Some(caps) = namespace_re.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                println!("  {} {} [namespace]", format!(":{}", line_num).dimmed(), name.cyan());
                found = true;
            }

            if let Some(caps) = class_re.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                println!("  {} {} [class]", format!(":{}", line_num).dimmed(), name.cyan());
                found = true;
            }

            if let Some(caps) = func_re.captures(line) {
                let class_prefix = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let name = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                if !class_prefix.is_empty() {
                    println!("  {} {}::{} [method]", format!(":{}", line_num).dimmed(), class_prefix.trim_end_matches("::"), name);
                } else {
                    println!("  {} {} [function]", format!(":{}", line_num).dimmed(), name);
                }
                found = true;
            }
        }
    } else if ext == "dart" {
        // Dart patterns — delegate to parser for correct results
        let symbols = crate::parsers::parse_dart_symbols(&content)?;
        for sym in &symbols {
            // Skip imports/properties for outline (too noisy)
            match sym.kind {
                SymbolKind::Import => continue,
                SymbolKind::Property => continue,
                _ => {}
            }
            let kind_str = sym.kind.as_str();
            println!("  {} {} [{}]", format!(":{}", sym.line).dimmed(), sym.name.cyan(), kind_str);
            found = true;
        }
    } else {
        // Kotlin/Java patterns
        let class_re = Regex::new(r"(?m)^\s*((?:public|private|protected|internal|abstract|open|final|sealed|data)?\s*)(class|interface|object|enum\s+class)\s+(\w+)")?;
        let fun_re = Regex::new(r"(?m)^\s*((?:public|private|protected|internal|override|suspend)?\s*)fun\s+(?:<[^>]*>\s*)?(\w+)")?;
        let prop_re = Regex::new(r"(?m)^\s*((?:public|private|protected|internal|override|const|lateinit)?\s*)(val|var)\s+(\w+)")?;

        for (line_num, line) in content.lines().enumerate() {
            let line_num = line_num + 1;

            if let Some(caps) = class_re.captures(line) {
                let kind = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                let name = caps.get(3).map(|m| m.as_str()).unwrap_or("");
                println!("  {} {} [{}]", format!(":{}", line_num).dimmed(), name.cyan(), kind);
                found = true;
            }

            if let Some(caps) = fun_re.captures(line) {
                let name = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                println!("  {} {} [function]", format!(":{}", line_num).dimmed(), name);
                found = true;
            }

            if let Some(caps) = prop_re.captures(line) {
                let kind = caps.get(2).map(|m| m.as_str()).unwrap_or("val");
                let name = caps.get(3).map(|m| m.as_str()).unwrap_or("");
                if !name.is_empty() && name != "val" && name != "var" {
                    println!("  {} {} [{}]", format!(":{}", line_num).dimmed(), name, kind);
                    found = true;
                }
            }
        }
    }

    if !found {
        println!("  No symbols found.");
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Show file imports
pub fn cmd_imports(root: &Path, file: &str) -> Result<()> {
    let start = Instant::now();

    let file_path = if file.starts_with('/') {
        PathBuf::from(file)
    } else {
        root.join(file)
    };

    if !file_path.exists() {
        println!("{}", format!("File not found: {}", file).red());
        return Ok(());
    }

    let content = std::fs::read_to_string(&file_path)?;

    // Detect file type by extension
    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let is_perl = ext == "pm" || ext == "pl" || ext == "t";
    let is_python = ext == "py";
    let is_go = ext == "go";
    let is_cpp = ext == "cpp" || ext == "cc" || ext == "c" || ext == "hpp" || ext == "h";

    println!("{}", format!("Imports in {}:", file).bold());

    let mut imports: Vec<String> = vec![];

    if is_perl {
        // Perl: use Module; or require Module;
        let use_re = Regex::new(r"^\s*(use|require)\s+([A-Za-z][A-Za-z0-9_:]*)")?;
        for line in content.lines() {
            if let Some(caps) = use_re.captures(line) {
                let keyword = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let module = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                // Skip pragmas
                if module != "strict" && module != "warnings" && module != "utf8" &&
                   module != "constant" && module != "base" && module != "parent" &&
                   !module.starts_with("v5") && !module.starts_with("5.") {
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

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Show module public API
pub fn cmd_api(root: &Path, module_path: &str, limit: usize) -> Result<()> {
    let start = Instant::now();

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
        if let Ok(conn) = crate::db::open_db(root) {
            let db_path: Option<String> = conn.query_row(
                "SELECT path FROM modules WHERE name = ?1",
                rusqlite::params![module_path],
                |row| row.get(0),
            ).ok();
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

    search_files(&module_dir, pattern, &["kt", "java"], |path, line_num, line| {
        if items.len() >= limit { return; }

        // Skip private/internal
        if line.contains("private ") || line.contains("internal ") {
            return;
        }

        let rel_path = relative_path(root, path);
        let content: String = line.trim().chars().take(100).collect();
        items.push((rel_path, line_num, content));
    })?;

    println!("{}", format!("Public API of '{}' ({}):", module_path, items.len()).bold());

    for (path, line_num, content) in &items {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    if items.is_empty() {
        println!("  No public API found.");
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Detect which VCS is used in the project directory
pub fn detect_vcs(root: &Path) -> &'static str {
    let home = std::env::var("HOME").ok().map(PathBuf::from);

    for ancestor in root.ancestors() {
        // Stop at home directory to avoid false positives from ~/.arc
        if let Some(ref h) = home {
            if ancestor == h.as_path() {
                break;
            }
        }

        // .arc/HEAD distinguishes real arc repo from ~/.arc (client storage)
        if ancestor.join(".arc").join("HEAD").exists() || ancestor.join(".arcconfig").exists() {
            return "arc";
        }
        if ancestor.join(".git").exists() {
            return "git";
        }
    }
    "git"
}

/// Get merge-base between HEAD and the given base branch
fn get_merge_base(root: &Path, vcs: &str, base: &str) -> Result<String> {
    let output = std::process::Command::new(vcs)
        .args(["merge-base", "HEAD", base])
        .current_dir(root)
        .output()?;

    if !output.status.success() {
        // Fallback to direct base if merge-base fails
        return Ok(base.to_string());
    }

    Ok(std::str::from_utf8(&output.stdout)?.trim().to_string())
}

/// Detect default git remote branch (origin/main or origin/master)
pub fn detect_git_default_branch(root: &Path) -> &'static str {
    // Try symbolic-ref to get remote HEAD
    if let Ok(output) = std::process::Command::new("git")
        .args(["symbolic-ref", "refs/remotes/origin/HEAD"])
        .current_dir(root)
        .output()
    {
        if output.status.success() {
            let refname = String::from_utf8_lossy(&output.stdout);
            if refname.contains("master") {
                return "origin/master";
            }
            return "origin/main";
        }
    }

    // Fallback: check if origin/main exists
    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "origin/main"])
        .current_dir(root)
        .output()
    {
        if output.status.success() {
            return "origin/main";
        }
    }

    // Last resort: try origin/master
    "origin/master"
}

/// Normalize base branch for the given VCS
fn normalize_base_for_vcs(vcs: &str, base: &str) -> String {
    if vcs == "arc" {
        // Arc doesn't use origin/ prefix
        base.strip_prefix("origin/").unwrap_or(base).to_string()
    } else {
        base.to_string()
    }
}

/// Show changed symbols in git/arc diff
pub fn cmd_changed(root: &Path, base: &str) -> Result<()> {
    let start = Instant::now();

    let vcs = detect_vcs(root);
    let base = normalize_base_for_vcs(vcs, base);

    // Find merge-base to only show changes from the current branch
    let merge_base = get_merge_base(root, vcs, &base)?;

    // Get list of changed files
    let output = std::process::Command::new(vcs)
        .args(["diff", "--name-only", &merge_base])
        .current_dir(root)
        .output()?;

    if !output.status.success() {
        let stderr = std::str::from_utf8(&output.stderr).unwrap_or("");
        println!("{}", format!("Failed to get {} diff: {}", vcs, stderr.trim()).red());
        return Ok(());
    }

    let changed_files: Vec<&str> = std::str::from_utf8(&output.stdout)?
        .lines()
        .filter(|f| {
            f.ends_with(".kt") || f.ends_with(".java") ||
            f.ends_with(".swift") || f.ends_with(".m") || f.ends_with(".h") ||
            f.ends_with(".pm") || f.ends_with(".pl") || f.ends_with(".t")
        })
        .collect();

    if changed_files.is_empty() {
        println!("No supported files changed since {}", base);
        return Ok(());
    }

    println!("{}", format!("Changed symbols since '{}' ({} files):", base, changed_files.len()).bold());

    // Parse changed files for symbols
    let class_re = Regex::new(r"(?m)^\s*(class|interface|object|enum\s+class)\s+(\w+)")?;
    let fun_re = Regex::new(r"(?m)^\s*(?:override\s+)?(?:suspend\s+)?fun\s+(\w+)")?;

    for file in &changed_files {
        let file_path = root.join(file);
        if !file_path.exists() { continue; }

        let content = match std::fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let mut symbols: Vec<String> = vec![];

        for line in content.lines() {
            if let Some(caps) = class_re.captures(line) {
                let kind = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let name = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                symbols.push(format!("{} {}", kind, name));
            }
            if let Some(caps) = fun_re.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                symbols.push(format!("fun {}", name));
            }
        }

        if !symbols.is_empty() {
            println!("\n  {}:", file.cyan());
            for sym in symbols.iter().take(10) {
                println!("    {}", sym);
            }
            if symbols.len() > 10 {
                println!("    ... and {} more", symbols.len() - 10);
            }
        }
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}
