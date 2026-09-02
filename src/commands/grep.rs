//! Grep-based search commands
//!
//! General pattern-based search commands:
//! - todo: Find TODO/FIXME/HACK comments
//! - callers: Find function callers
//! - provides: Find Dagger @Provides/@Binds for a type
//! - suspend: Find suspend functions
//! - composables: Find @Composable functions
//! - deprecated: Find @Deprecated annotations
//! - suppress: Find @Suppress annotations
//! - inject: Find @Inject points for a type
//! - annotations: Find uses of specific annotation
//! - deeplinks: Find deeplink definitions
//! - extensions: Find extension functions/types
//! - flows: Find Flow declarations
//! - previews: Find @Preview functions

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use colored::Colorize;
use regex::Regex;

use super::{print_truncation_notice, relative_path, search_files_limited, search_files_page};

/// All source code extensions (for grep-based commands: todo, search, callers, etc.)
pub const ALL_SOURCE_EXTENSIONS: [&str; 58] = [
    "kt", "java", "swift", "m", "h",    // Mobile
    "dart", // Flutter
    "gd",   // GDScript (Godot)
    "pm", "pl", "t", "rb", // Scripting
    "ts", "tsx", "mts", "js", "jsx", "mjs", "cjs", // JavaScript/TypeScript
    "vue", "svelte", // Web frameworks
    "css", "pcss", "postcss", "scss", "less", // CSS family
    "py",   // Python
    "go",   // Go
    "rs",   // Rust
    "zig",  // Zig
    "cs",   // C#
    "cpp", "cc", "c", "hpp", // C++
    "scala", "sc", // Scala
    "php", "phtml", // PHP
    "groovy", "gradle", // Groovy
    "lua",    // Lua
    "ex", "exs", // Elixir
    "sh", "bash", "zsh", // Shell
    "sql", // SQL
    "r", "R", // R
    "bsl", "os", // BSL
    "lisp", "lsp", "cl", "asd", // Common Lisp
    "proto", "wsdl", "xsd", // Schema
];

/// Trailing word boundary: `\b` for normal names, empty for Ruby bang/question methods
fn trailing_boundary(function_name: &str) -> &str {
    if function_name.ends_with('!') || function_name.ends_with('?') {
        "" // ! and ? are non-word chars — natural boundary, \b would fail here
    } else {
        r"\b"
    }
}

/// Build regex pattern that matches function/method calls across languages
fn build_caller_pattern(function_name: &str) -> String {
    let fn_escaped = regex::escape(function_name);
    let tb = trailing_boundary(function_name);
    format!(
        concat!(
            r"[.>]{fn}\s*\(",          // obj.func( or obj->func(
            r"|\b{fn}\s*\(",           // bare func( anywhere in line
            r"|->{fn}\s*\(",           // ->func(
            r"|&{fn}\s*\(",            // &func(
            r"|this\.{fn}\s*\(",       // this.func(
            r"|super\.{fn}\s*\(",      // super.func(
            r"|\.{fn}(?:\s|$)",        // Ruby: obj.method (no parens)
            r"|:{fn}{tb}",             // Ruby: :method_name (symbol ref in callbacks)
            r"|\b{fn}\.",             // Ruby: bare method.chain (e.g. scope.where)
            r"|\bawait\s+{fn}\s*\(",               // TS: await func(
            r"|\bawait\s+[\w.]+\.{fn}\s*\(",       // TS: await obj.func(
            r"|\breturn\s+{fn}\s*\(",              // TS: return func(
            r"|\breturn\s+[\w.]+\.{fn}\s*\(",      // TS: return obj.func(
        ),
        fn = fn_escaped,
        tb = tb
    )
}

/// Build regex pattern that skips function/method definitions
fn build_def_skip_pattern(function_name: &str) -> Regex {
    let fn_escaped = regex::escape(function_name);
    let tb = trailing_boundary(function_name);
    Regex::new(&format!(
        concat!(
            r"\b(?:fun|func|sub)\s+{fn}\s*[<({{\[]",           // Kotlin/Swift/Perl
            r"|\bdef\s+(?:self\.)?{fn}{tb}",                    // Ruby: def method / def self.method
            r"|\b(?:(?:public|private|protected|static|final|abstract|synchronized|override)\s+)*",
            r"(?:void|int|long|boolean|char|byte|short|float|double|[\w.]+(?:<[^{{;]*>)?(?:\[\])*)\s+{fn}\s*\(", // Java
        ),
        fn = fn_escaped,
        tb = tb
    ))
    .expect("Invalid def skip pattern")
}

/// Find TODO/FIXME/HACK comments
pub fn cmd_todo(root: &Path, pattern: &str, limit: usize) -> Result<()> {
    let search_pattern = format!(r"//.*({pattern})|#.*({pattern})");

    let mut todos: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
    todos.insert("TODO".to_string(), vec![]);
    todos.insert("FIXME".to_string(), vec![]);
    todos.insert("HACK".to_string(), vec![]);
    todos.insert("OTHER".to_string(), vec![]);

    let mut count = 0;

    search_files_limited(
        root,
        &search_pattern,
        &ALL_SOURCE_EXTENSIONS,
        limit,
        |path, line_num, line| {
            let rel_path = relative_path(root, path);
            let content: String = line.chars().take(80).collect();
            let upper = content.to_uppercase();

            let category = if upper.contains("TODO") {
                "TODO"
            } else if upper.contains("FIXME") {
                "FIXME"
            } else if upper.contains("HACK") {
                "HACK"
            } else {
                "OTHER"
            };

            todos
                .get_mut(category)
                .unwrap()
                .push((rel_path, line_num, content));
            count += 1;
        },
    )?;

    let total: usize = todos.values().map(|v| v.len()).sum();
    println!("{}", format!("Found {} comments:", total).bold());

    for (category, items) in &todos {
        if !items.is_empty() {
            println!("\n{}", format!("{} ({}):", category, items.len()).cyan());
            for (path, line_num, content) in items.iter().take(20) {
                println!("  {}:{}", path, line_num);
                println!("    {}", content);
            }
            if items.len() > 20 {
                println!("  ... and {} more", items.len() - 20);
            }
        }
    }

    Ok(())
}

/// Find function callers
pub fn cmd_callers(root: &Path, function_name: &str, limit: usize, format: &str) -> Result<()> {
    let pattern = build_caller_pattern(function_name);
    let def_pattern = build_def_skip_pattern(function_name);

    let page = search_files_page(
        root,
        &pattern,
        &ALL_SOURCE_EXTENSIONS,
        limit,
        |path, line_num, line| {
            if def_pattern.is_match(line) {
                return None;
            } // Skip definitions

            let rel_path = relative_path(root, path);
            let content: String = line.chars().take(70).collect();
            Some((rel_path, line_num, content))
        },
    )?;

    if format == "json" {
        let items: Vec<_> = page
            .items
            .iter()
            .map(|(path, line, content)| {
                serde_json::json!({"path": path, "line": line, "content": content})
            })
            .collect();
        let result = serde_json::json!({
            "schema_version": super::PAGINATED_JSON_SCHEMA_VERSION,
            "items": items,
            "pagination": page.pagination,
        });
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    println!(
        "{}",
        format!(
            "Callers of '{}' (showing {} of {}):",
            function_name, page.pagination.returned, page.pagination.total
        )
        .bold()
    );

    let mut by_file: HashMap<&str, Vec<(usize, &str)>> = HashMap::new();
    for (path, line, content) in &page.items {
        by_file
            .entry(path)
            .or_default()
            .push((*line, content.as_str()));
    }
    let mut paths: Vec<_> = by_file.into_iter().collect();
    paths.sort_by(|left, right| left.0.cmp(right.0));
    for (path, items) in paths {
        println!("\n  {}:", path.cyan());
        for (line_num, content) in items {
            println!("    :{} {}", line_num, content);
        }
    }
    print_truncation_notice(page.pagination);

    Ok(())
}

/// Show call hierarchy (callers tree) for a function
pub fn cmd_call_tree(
    root: &Path,
    function_name: &str,
    max_depth: usize,
    limit_per_level: usize,
) -> Result<()> {
    println!("{}", format!("Call tree for '{}':", function_name).bold());
    println!("  {}", function_name.cyan());

    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    visited.insert(function_name.to_string());

    build_call_tree(
        root,
        function_name,
        1,
        max_depth,
        limit_per_level,
        &mut visited,
    )?;

    Ok(())
}

/// Recursively build call tree
fn build_call_tree(
    root: &Path,
    function_name: &str,
    current_depth: usize,
    max_depth: usize,
    limit: usize,
    visited: &mut std::collections::HashSet<String>,
) -> Result<()> {
    if current_depth > max_depth {
        return Ok(());
    }

    let indent = "  ".repeat(current_depth + 1);
    let callers = find_caller_functions(root, function_name, limit)?;

    if callers.is_empty() {
        return Ok(());
    }

    for (caller_func, file_path, line_num) in callers {
        let is_new = visited.insert(caller_func.clone());

        if is_new {
            println!(
                "{}← {} ({}:{})",
                indent,
                caller_func.yellow(),
                file_path,
                line_num
            );
            // Recursively find callers of this function
            build_call_tree(
                root,
                &caller_func,
                current_depth + 1,
                max_depth,
                limit,
                visited,
            )?;
        } else {
            println!("{}← {} (recursive)", indent, caller_func.dimmed());
        }
    }

    Ok(())
}

/// Find functions that call the given function
fn find_caller_functions(
    root: &Path,
    function_name: &str,
    limit: usize,
) -> Result<Vec<(String, String, usize)>> {
    let pattern = build_caller_pattern(function_name);
    let def_pattern = build_def_skip_pattern(function_name);

    // Pattern to find function definitions (for locating the containing function)
    // Group 1: fun/func/function/def/sub style, Group 2: Ruby def/def self., Group 3: Java return-type style, Group 4: TS arrow function
    let func_def_re = Regex::new(concat!(
        r"(?:fun|function|func|sub)\s+(\w+)\s*[<(\[]",
        r"|\bdef\s+(?:self\.)?(\w[!\w?]*)",
        r"|(?:(?:public|private|protected|static|final|abstract|synchronized|override|export|async)\s+)*",
        r"(?:void|int|long|boolean|char|byte|short|float|double|[\w.]+(?:<[^{;]*>)?(?:\[\])*)\s+(\w+)\s*\(",
        r"|(?:const|let)\s+(\w+)\s*=\s*(?:async\s+)?(?:\([^)]*\)|[a-zA-Z_]\w*)\s*(?::\s*[^=]+)?\s*=>",
    ))?;

    let mut results: Vec<(String, String, usize)> = vec![];
    let mut files_with_calls: HashMap<PathBuf, Vec<usize>> = HashMap::new();

    // First pass: find all files and line numbers with calls
    search_files_limited(
        root,
        &pattern,
        &ALL_SOURCE_EXTENSIONS,
        limit * 3,
        |path, line_num, line| {
            if def_pattern.is_match(line) {
                return;
            }

            files_with_calls
                .entry(path.to_path_buf())
                .or_default()
                .push(line_num);
        },
    )?;

    // Second pass: for each call location, find the containing function
    for (file_path, call_lines) in files_with_calls {
        if results.len() >= limit {
            break;
        }

        let content = match std::fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let lines: Vec<&str> = content.lines().collect();
        let rel_path = relative_path(root, &file_path);

        for call_line in call_lines {
            if results.len() >= limit {
                break;
            }

            // Search backwards to find the containing function
            if let Some((func_name, func_line)) =
                find_containing_function(&lines, call_line, &func_def_re)
            {
                // Avoid adding the same function twice for this target
                if !results
                    .iter()
                    .any(|(f, p, _)| f == &func_name && p == &rel_path)
                {
                    results.push((func_name, rel_path.clone(), func_line));
                }
            }
        }
    }

    Ok(results)
}

/// Find the function that contains a given line number
fn find_containing_function(
    lines: &[&str],
    target_line: usize,
    func_def_re: &Regex,
) -> Option<(String, usize)> {
    // Search backwards from the target line to find a function definition
    let start_idx = (target_line.saturating_sub(1)).min(lines.len().saturating_sub(1));

    for i in (0..=start_idx).rev() {
        let line = lines[i];
        if let Some(caps) = func_def_re.captures(line) {
            // Group 1: fun/function/func/sub, Group 2: Ruby def, Group 3: Java return-type, Group 4: TS arrow
            if let Some(name) = caps
                .get(1)
                .or_else(|| caps.get(2))
                .or_else(|| caps.get(3))
                .or_else(|| caps.get(4))
            {
                return Some((name.as_str().to_string(), i + 1));
            }
        }
    }

    None
}

/// Find Dagger @Provides/@Binds for a type
pub fn cmd_provides(root: &Path, type_name: &str, limit: usize) -> Result<()> {
    let mut results: Vec<(String, usize, String)> = vec![];

    // Walk files and search with context
    use ignore::WalkBuilder;
    let is_git = crate::indexer::has_git_repo(root);
    let arc_root = crate::indexer::find_arc_root(root);
    let mut wb = WalkBuilder::new(root);
    wb.hidden(true)
        .git_ignore(is_git)
        .filter_entry(|entry| !crate::indexer::is_excluded_dir(entry));
    if let Some(ref arc) = arc_root {
        wb.add_custom_ignore_filename(".gitignore");
        wb.add_custom_ignore_filename(".arcignore");
        let root_gitignore = arc.join(".gitignore");
        if root_gitignore.exists() {
            wb.add_ignore(root_gitignore);
        }
    }
    let walker = wb.build();

    for entry in walker.filter_map(|e| e.ok()) {
        if results.len() >= limit {
            break;
        }
        let path = entry.path();
        if !path
            .extension()
            .map(|e| e == "kt" || e == "java")
            .unwrap_or(false)
        {
            continue;
        }

        if let Ok(content) = std::fs::read_to_string(path) {
            let lines: Vec<&str> = content.lines().collect();
            let kotlin_re = Regex::new(&format!(r":\s*\w*{}\b", regex::escape(type_name))).ok();
            let java_re =
                Regex::new(&format!(r"\b\w*{}\s+\w+\s*\(", regex::escape(type_name))).ok();
            for (i, line) in lines.iter().enumerate() {
                if results.len() >= limit {
                    break;
                }
                // Check if this line has @Provides or @Binds
                if line.contains("@Provides") || line.contains("@Binds") {
                    // Look at this line and next few lines for the return type
                    let context: String = lines[i..std::cmp::min(i + 5, lines.len())].join(" ");
                    // Check if return type matches (allow prefix like AppIconInteractor matches Interactor)
                    // Kotlin pattern: `: ReturnType` (colon before type)
                    // Java pattern: `ReturnType methodName(` (type before method name)
                    let matches_kotlin = kotlin_re
                        .as_ref()
                        .map(|re| re.is_match(&context))
                        .unwrap_or(false);
                    let matches_java = java_re
                        .as_ref()
                        .map(|re| re.is_match(&context))
                        .unwrap_or(false);
                    if matches_kotlin || matches_java {
                        let rel_path = relative_path(root, path);
                        // Get the function line (usually next line after annotation)
                        // Kotlin: `fun name()`, Java: method signature without `fun`
                        let func_line = if i + 1 < lines.len() {
                            let next_line = lines[i + 1].trim();
                            if next_line.contains("fun ") || next_line.contains("(") {
                                next_line.to_string()
                            } else if i + 2 < lines.len() && lines[i + 2].trim().contains("(") {
                                // Java: annotation -> modifiers -> method
                                lines[i + 2].trim().to_string()
                            } else {
                                line.trim().to_string()
                            }
                        } else {
                            line.trim().to_string()
                        };
                        results.push((rel_path, i + 1, func_line));
                    }
                }
            }
        }
    }

    println!(
        "{}",
        format!("Providers for '{}' ({}):", type_name, results.len()).bold()
    );

    for (path, line_num, content) in &results {
        println!("  {}:{}", path, line_num);
        let truncated: String = content.chars().take(100).collect();
        println!("    {}", truncated);
    }

    Ok(())
}

/// Find suspend functions
pub fn cmd_suspend(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let pattern = r"suspend\s+fun\s+\w+";
    let func_regex = Regex::new(r"suspend\s+fun\s+(\w+)")?;

    let mut suspends: Vec<(String, String, usize)> = vec![];

    search_files_limited(root, pattern, &["kt"], limit, |path, line_num, line| {
        if let Some(caps) = func_regex.captures(line) {
            let func_name = caps.get(1).unwrap().as_str().to_string();

            if let Some(q) = query {
                if !func_name.to_lowercase().contains(&q.to_lowercase()) {
                    return;
                }
            }

            let rel_path = relative_path(root, path);
            suspends.push((func_name, rel_path, line_num));
        }
    })?;

    println!(
        "{}",
        format!("Suspend functions ({}):", suspends.len()).bold()
    );

    for (func_name, path, line_num) in &suspends {
        println!("  {}: {}:{}", func_name.cyan(), path, line_num);
    }

    Ok(())
}

/// Find @Composable functions
pub fn cmd_composables(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let func_regex = Regex::new(r"fun\s+(\w+)\s*\(")?;

    // Phase 1: find all .kt files containing @Composable
    let mut file_set: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    search_files_limited(
        root,
        r"@Composable",
        &["kt"],
        100_000,
        |path, _line_num, _line| {
            file_set.insert(path.to_path_buf());
        },
    )?;

    // Phase 2: read each file and find @Composable + fun pairs (multi-line aware)
    let mut composables: Vec<(String, String, usize)> = vec![];
    let mut sorted_files: Vec<_> = file_set.into_iter().collect();
    sorted_files.sort();

    for file_path in &sorted_files {
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let lines: Vec<&str> = content.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            if lines[i].contains("@Composable") {
                // Look at current and next few lines for fun definition
                for j in i..=(i + 5).min(lines.len() - 1) {
                    if let Some(caps) = func_regex.captures(lines[j]) {
                        let func_name = caps.get(1).unwrap().as_str().to_string();

                        if let Some(q) = query {
                            if !func_name.to_lowercase().contains(&q.to_lowercase()) {
                                break;
                            }
                        }

                        let rel_path = relative_path(root, file_path);
                        composables.push((func_name, rel_path, j + 1));
                        i = j;
                        break;
                    }
                }
            }
            i += 1;
        }

        if composables.len() >= limit {
            composables.truncate(limit);
            break;
        }
    }

    composables.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));

    println!(
        "{}",
        format!("@Composable functions ({}):", composables.len()).bold()
    );

    for (func_name, path, line_num) in &composables {
        println!("  {}: {}:{}", func_name.cyan(), path, line_num);
    }

    Ok(())
}

/// Find @Deprecated annotations
pub fn cmd_deprecated(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    // Kotlin/Java/C#: @Deprecated/@Obsolete, Swift: @available(*, deprecated)
    // Python: @deprecated, Perl: DEPRECATED, Rust: #[deprecated], Go: // Deprecated:
    // JS/TS: @deprecated (JSDoc), PHP: @deprecated (PHPDoc), C++: [[deprecated]]
    let pattern = r"@Deprecated|@Obsolete|@available\s*\([^)]*deprecated|#\[deprecated|#.*DEPRECATED|=head.*DEPRECATED|@deprecated|\[\[deprecated";

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files_limited(
        root,
        pattern,
        &ALL_SOURCE_EXTENSIONS,
        limit,
        |path, line_num, line| {
            if let Some(q) = query {
                if !line.to_lowercase().contains(&q.to_lowercase()) {
                    return;
                }
            }

            let rel_path = relative_path(root, path);
            let content: String = line.trim().chars().take(80).collect();
            items.push((rel_path, line_num, content));
        },
    )?;

    println!("{}", format!("@Deprecated items ({}):", items.len()).bold());

    for (path, line_num, content) in &items {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    Ok(())
}

/// Find @Suppress annotations
pub fn cmd_suppress(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let pattern = r"@Suppress";

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files_limited(root, pattern, &["kt"], limit, |path, line_num, line| {
        if let Some(q) = query {
            if !line.to_lowercase().contains(&q.to_lowercase()) {
                return;
            }
        }

        let rel_path = relative_path(root, path);
        let content: String = line.trim().chars().take(80).collect();
        items.push((rel_path, line_num, content));
    })?;

    println!(
        "{}",
        format!("@Suppress annotations ({}):", items.len()).bold()
    );

    for (path, line_num, content) in &items {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    Ok(())
}

/// Find @Inject/@Autowired points for a type
pub fn cmd_inject(root: &Path, type_name: &str, limit: usize) -> Result<()> {
    let pattern = r"@Inject|@Autowired";

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files_limited(
        root,
        pattern,
        &["kt", "java"],
        limit,
        |path, line_num, line| {
            let has_di = line.contains("@Inject") || line.contains("@Autowired");
            if !line.contains(type_name) && !has_di {
                return;
            }

            let rel_path = relative_path(root, path);
            let content: String = line.trim().chars().take(80).collect();
            items.push((rel_path, line_num, content));
        },
    )?;

    // Filter to those containing type_name
    let filtered: Vec<_> = items
        .iter()
        .filter(|(_, _, line)| line.contains(type_name))
        .take(limit)
        .collect();

    println!(
        "{}",
        format!("Injection points for '{}' ({}):", type_name, filtered.len()).bold()
    );

    for (path, line_num, content) in &filtered {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    Ok(())
}

/// Find uses of specific annotation
pub fn cmd_annotations(root: &Path, annotation: &str, limit: usize) -> Result<()> {
    // Normalize annotation (add @ if missing for Java/Kotlin/Swift/ObjC)
    // For Perl, attributes are like :lvalue, :method
    let search_annotation = if annotation.starts_with('@') || annotation.starts_with(':') {
        annotation.to_string()
    } else {
        format!("@{}", annotation)
    };
    let pattern = regex::escape(&search_annotation);

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files_limited(
        root,
        &pattern,
        &ALL_SOURCE_EXTENSIONS,
        limit,
        |path, line_num, line| {
            let rel_path = relative_path(root, path);
            let content: String = line.trim().chars().take(80).collect();
            items.push((rel_path, line_num, content));
        },
    )?;

    println!(
        "{}",
        format!("Classes with {} ({}):", search_annotation, items.len()).bold()
    );

    for (path, line_num, content) in &items {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    Ok(())
}

/// Find deeplink definitions
pub fn cmd_deeplinks(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    // Search for specific deeplink patterns (NOT generic :// URLs)
    // Android: @DeepLink, DeepLinkHandler, @AppLink, NavDeepLink, intent-filter with android:scheme
    // iOS: openURL, application(_:open:, handleOpen, CFBundleURLSchemes, UniversalLink
    let pattern = r#"[Dd]eep[Ll]ink|@DeepLink|DeepLinkHandler|@AppLink|NavDeepLink|android:scheme|openURL|application\([^)]*open:|handleOpen|CFBundleURLSchemes|UniversalLink|NSUserActivity"#;

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files_limited(
        root,
        pattern,
        &["kt", "java", "xml", "swift", "m", "h", "plist"],
        limit,
        |path, line_num, line| {
            if let Some(q) = query {
                if !line.to_lowercase().contains(&q.to_lowercase()) {
                    return;
                }
            }

            let rel_path = relative_path(root, path);
            let content: String = line.trim().chars().take(100).collect();
            items.push((rel_path, line_num, content));
        },
    )?;

    println!("{}", format!("Deeplinks ({}):", items.len()).bold());

    for (path, line_num, content) in &items {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    Ok(())
}

/// Find extension functions/types
pub fn cmd_extensions(root: &Path, receiver_type: &str, limit: usize) -> Result<()> {
    // Kotlin: fun ReceiverType.functionName
    // Swift: extension ReceiverType
    let kotlin_pattern = format!(r"fun\s+{}\.(\w+)", regex::escape(receiver_type));
    let swift_pattern = format!(r"extension\s+{}", regex::escape(receiver_type));
    let pattern = format!(r"{}|{}", kotlin_pattern, swift_pattern);

    let kotlin_regex = Regex::new(&kotlin_pattern)?;
    let swift_regex = Regex::new(&swift_pattern)?;

    let mut items: Vec<(String, String, usize, String)> = vec![]; // (name, path, line, lang)

    search_files_limited(
        root,
        &pattern,
        &["kt", "swift"],
        limit,
        |path, line_num, line| {
            let rel_path = relative_path(root, path);

            if let Some(caps) = kotlin_regex.captures(line) {
                let func_name = caps.get(1).unwrap().as_str().to_string();
                items.push((func_name, rel_path, line_num, "kt".to_string()));
            } else if swift_regex.is_match(line) {
                let content: String = line.trim().chars().take(60).collect();
                items.push((content, rel_path, line_num, "swift".to_string()));
            }
        },
    )?;

    println!(
        "{}",
        format!("Extensions for {} ({}):", receiver_type, items.len()).bold()
    );

    for (name, path, line_num, lang) in &items {
        if lang == "kt" {
            println!("  {}.{}: {}:{}", receiver_type.cyan(), name, path, line_num);
        } else {
            println!("  {}:{} {}", path.cyan(), line_num, name);
        }
    }

    Ok(())
}

/// Find Flow declarations
pub fn cmd_flows(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let pattern = r"(StateFlow|SharedFlow|MutableStateFlow|MutableSharedFlow|Flow<)";
    let flow_regex =
        Regex::new(r"(StateFlow|SharedFlow|MutableStateFlow|MutableSharedFlow|Flow)<")?;

    let mut items: Vec<(String, String, usize, String)> = vec![];

    search_files_limited(root, pattern, &["kt"], limit, |path, line_num, line| {
        if let Some(caps) = flow_regex.captures(line) {
            let flow_type = caps.get(1).unwrap().as_str().to_string();

            if let Some(q) = query {
                if !line.to_lowercase().contains(&q.to_lowercase()) {
                    return;
                }
            }

            let rel_path = relative_path(root, path);
            let content: String = line.trim().chars().take(70).collect();
            items.push((flow_type, rel_path, line_num, content));
        }
    })?;

    println!("{}", format!("Flow declarations ({}):", items.len()).bold());

    for (flow_type, path, line_num, content) in &items {
        println!("  [{}] {}:{}", flow_type.cyan(), path, line_num);
        println!("    {}", content);
    }

    Ok(())
}

/// Find @Preview functions
pub fn cmd_previews(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let func_regex = Regex::new(r"fun\s+(\w+)\s*\(")?;

    // Phase 1: find all .kt files containing @Preview
    let mut file_set: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    search_files_limited(
        root,
        r"@Preview",
        &["kt"],
        100_000,
        |path, _line_num, _line| {
            file_set.insert(path.to_path_buf());
        },
    )?;

    // Phase 2: read each file and find @Preview + fun pairs (multi-line aware)
    let mut items: Vec<(String, String, usize)> = vec![];
    let mut sorted_files: Vec<_> = file_set.into_iter().collect();
    sorted_files.sort();

    for file_path in &sorted_files {
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let lines: Vec<&str> = content.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            if lines[i].contains("@Preview") {
                // Look at current and next few lines for fun definition
                for j in i..=(i + 5).min(lines.len() - 1) {
                    if let Some(caps) = func_regex.captures(lines[j]) {
                        let func_name = caps.get(1).unwrap().as_str().to_string();

                        if let Some(q) = query {
                            if !func_name.to_lowercase().contains(&q.to_lowercase()) {
                                break;
                            }
                        }

                        let rel_path = relative_path(root, file_path);
                        items.push((func_name, rel_path, j + 1));
                        i = j;
                        break;
                    }
                }
            }
            i += 1;
        }

        if items.len() >= limit {
            items.truncate(limit);
            break;
        }
    }

    items.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));

    println!(
        "{}",
        format!("@Preview functions ({}):", items.len()).bold()
    );

    for (func_name, path, line_num) in &items {
        println!("  {}: {}:{}", func_name.cyan(), path, line_num);
    }

    Ok(())
}

/// Structural code search via ast-grep (requires `sg` or `ast-grep` installed)
pub fn cmd_ast_grep(root: &Path, pattern: &str, lang: Option<&str>, json: bool) -> Result<()> {
    // Find ast-grep binary
    let binary = find_ast_grep_binary()
        .ok_or_else(|| anyhow::anyhow!(
            "ast-grep not found. Install it:\n  brew install ast-grep    # macOS\n  npm i -g @ast-grep/cli   # npm\n  cargo install ast-grep    # cargo"
        ))?;

    let mut cmd = std::process::Command::new(&binary);
    cmd.arg("run")
        .arg("--pattern")
        .arg(pattern)
        .current_dir(root);

    if let Some(lang) = lang {
        cmd.arg("--lang").arg(lang);
    }

    if json {
        cmd.arg("--json=compact");
    }

    let status = cmd.status()?;

    if !status.success() && status.code() != Some(1) {
        // Exit code 1 = no matches (normal for grep), anything else is an error
        anyhow::bail!("ast-grep exited with code {:?}", status.code());
    }

    Ok(())
}

fn find_ast_grep_binary() -> Option<String> {
    for name in &["sg", "ast-grep"] {
        if std::process::Command::new(name)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
        {
            return Some(name.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- build_caller_pattern tests ---

    fn matches(pattern: &str, text: &str) -> bool {
        Regex::new(pattern).unwrap().is_match(text)
    }

    #[test]
    fn test_caller_pattern_dot_call_with_parens() {
        let pat = build_caller_pattern("perform_async");
        assert!(matches(&pat, "  MyWorker.perform_async(id)"));
        assert!(matches(&pat, "  worker.perform_async(1, 2)"));
    }

    #[test]
    fn test_caller_pattern_dot_call_without_parens() {
        let pat = build_caller_pattern("process");
        // Ruby: obj.method without parens
        assert!(matches(&pat, "  new(*args).process"));
        assert!(matches(&pat, "  service.process"));
    }

    #[test]
    fn test_caller_pattern_bare_call_with_parens() {
        let pat = build_caller_pattern("normalize_phone");
        assert!(matches(&pat, "  normalized = normalize_phone(number)"));
        assert!(matches(&pat, "    if result = normalize_phone(input)"));
    }

    #[test]
    fn test_caller_pattern_symbol_ref() {
        let pat = build_caller_pattern("set_timestamps");
        // Ruby callbacks: before_action :method_name
        assert!(matches(&pat, "  before_save :set_timestamps"));
        assert!(matches(
            &pat,
            "  after_create :set_timestamps, if: :active?"
        ));
    }

    #[test]
    fn test_caller_pattern_method_chain() {
        let pat = build_caller_pattern("recalc_counters");
        // Ruby: bare method.chain
        assert!(matches(&pat, "    recalc_counters.where(job_id: job.id)"));
    }

    #[test]
    fn test_caller_pattern_no_false_positives_in_substring() {
        let pat = build_caller_pattern("process");
        // Should NOT match "preprocess" as a bare call with parens
        assert!(!matches(&pat, "  preprocess(data)"));
    }

    #[test]
    fn test_caller_pattern_ruby_bang_method() {
        let pat = build_caller_pattern("authenticate_user!");
        // Ruby callbacks with bang methods
        assert!(matches(&pat, "  before_action :authenticate_user!"));
        assert!(matches(
            &pat,
            "  skip_before_action :authenticate_user!, only: [:index]"
        ));
        // Direct calls
        assert!(matches(&pat, "  authenticate_user!(request)"));
        assert!(matches(&pat, "  current_user.authenticate_user!"));
    }

    #[test]
    fn test_caller_pattern_ruby_question_method() {
        let pat = build_caller_pattern("valid?");
        assert!(matches(&pat, "  record.valid?"));
        assert!(matches(&pat, "  valid?(params)"));
    }

    #[test]
    fn test_caller_pattern_await_bare_call() {
        let pat = build_caller_pattern("fetchCategories");
        assert!(matches(&pat, "  await fetchCategories()"));
        assert!(matches(&pat, "  const result = await fetchCategories()"));
    }

    #[test]
    fn test_caller_pattern_await_method_call() {
        let pat = build_caller_pattern("loadEventRecords");
        assert!(matches(&pat, "  await store.loadEventRecords()"));
        assert!(matches(&pat, "  await this.loadEventRecords()"));
    }

    #[test]
    fn test_caller_pattern_return_bare_call() {
        let pat = build_caller_pattern("pluralize");
        assert!(matches(&pat, "  return pluralize(count, forms)"));
    }

    #[test]
    fn test_caller_pattern_return_method_call() {
        let pat = build_caller_pattern("serialize");
        assert!(matches(&pat, "  return serializer.serialize()"));
    }

    #[test]
    fn test_caller_pattern_await_chained() {
        let pat = build_caller_pattern("addAction");
        assert!(matches(&pat, "  await syncQueue.addAction(action)"));
    }

    #[test]
    fn test_caller_pattern_same_file_calls() {
        // Common Pinia store / composable pattern: functions calling each other
        let pat = build_caller_pattern("loadFromDB");
        // Bare call (no await)
        assert!(matches(&pat, "    loadFromDB()"));
        // Await call (most common in stores)
        assert!(matches(&pat, "    await loadFromDB()"));
        // Definition line should NOT match
        assert!(!matches(&pat, "  const loadFromDB = async () => {"));
        // Return call
        assert!(matches(&pat, "    return loadFromDB()"));
    }

    // --- build_def_skip_pattern tests ---

    #[test]
    fn test_def_skip_ruby_bang_method() {
        let pat = build_def_skip_pattern("authenticate_user!");
        assert!(pat.is_match("  def authenticate_user!"));
        assert!(pat.is_match("  def self.authenticate_user!"));
    }

    #[test]
    fn test_def_skip_ruby_instance_method() {
        let pat = build_def_skip_pattern("process");
        assert!(pat.is_match("  def process"));
        assert!(pat.is_match("  def process(args)"));
    }

    #[test]
    fn test_def_skip_ruby_self_method() {
        let pat = build_def_skip_pattern("call");
        assert!(pat.is_match("  def self.call(params)"));
        assert!(pat.is_match("  def self.call"));
    }

    #[test]
    fn test_def_skip_does_not_match_calls() {
        let pat = build_def_skip_pattern("process");
        assert!(!pat.is_match("  service.process"));
        assert!(!pat.is_match("  result = process(data)"));
    }

    #[test]
    fn test_def_skip_kotlin_fun() {
        let pat = build_def_skip_pattern("calculate");
        assert!(pat.is_match("  fun calculate(x: Int)"));
    }

    // --- find_containing_function tests ---

    #[test]
    fn test_find_containing_ruby_method() {
        let code = vec![
            "class MyService",
            "  def process",
            "    result = other_service.call(data)",
            "    transform(result)",
            "  end",
            "end",
        ];
        let func_def_re = Regex::new(
            concat!(
                r"(?:fun|func|sub)\s+(\w+)\s*[<(\[]",
                r"|\bdef\s+(?:self\.)?(\w[!\w?]*)",
                r"|(?:(?:public|private|protected|static|final|abstract|synchronized|override)\s+)*",
                r"(?:void|int|long|boolean|char|byte|short|float|double|[\w.]+(?:<[^{;]*>)?(?:\[\])*)\s+(\w+)\s*\(",
            )
        ).unwrap();

        // Line 3 (0-indexed) = "    result = other_service.call(data)"
        let result = find_containing_function(&code, 3, &func_def_re);
        assert_eq!(result, Some(("process".to_string(), 2)));
    }

    #[test]
    fn test_find_containing_ruby_self_method() {
        let code = vec![
            "class MyService",
            "  def self.call(params)",
            "    new(params).process",
            "  end",
            "end",
        ];
        let func_def_re = Regex::new(
            concat!(
                r"(?:fun|func|sub)\s+(\w+)\s*[<(\[]",
                r"|\bdef\s+(?:self\.)?(\w[!\w?]*)",
                r"|(?:(?:public|private|protected|static|final|abstract|synchronized|override)\s+)*",
                r"(?:void|int|long|boolean|char|byte|short|float|double|[\w.]+(?:<[^{;]*>)?(?:\[\])*)\s+(\w+)\s*\(",
            )
        ).unwrap();

        let result = find_containing_function(&code, 3, &func_def_re);
        assert_eq!(result, Some(("call".to_string(), 2)));
    }

    #[test]
    fn test_find_containing_ruby_bang_method() {
        let code = vec![
            "class Updater",
            "  def update!",
            "    record.save!",
            "  end",
            "end",
        ];
        let func_def_re = Regex::new(
            concat!(
                r"(?:fun|func|sub)\s+(\w+)\s*[<(\[]",
                r"|\bdef\s+(?:self\.)?(\w[!\w?]*)",
                r"|(?:(?:public|private|protected|static|final|abstract|synchronized|override)\s+)*",
                r"(?:void|int|long|boolean|char|byte|short|float|double|[\w.]+(?:<[^{;]*>)?(?:\[\])*)\s+(\w+)\s*\(",
            )
        ).unwrap();

        let result = find_containing_function(&code, 3, &func_def_re);
        assert_eq!(result, Some(("update!".to_string(), 2)));
    }
}
