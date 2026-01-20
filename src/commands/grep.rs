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
use std::time::Instant;

use anyhow::Result;
use colored::Colorize;
use regex::Regex;

use super::{search_files, relative_path};

/// Find TODO/FIXME/HACK comments
pub fn cmd_todo(root: &Path, pattern: &str, limit: usize) -> Result<()> {
    let start = Instant::now();
    let search_pattern = format!(r"//.*({pattern})|#.*({pattern})");

    let mut todos: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
    todos.insert("TODO".to_string(), vec![]);
    todos.insert("FIXME".to_string(), vec![]);
    todos.insert("HACK".to_string(), vec![]);
    todos.insert("OTHER".to_string(), vec![]);

    let mut count = 0;

    search_files(root, &search_pattern, &["kt", "java", "swift", "m", "h", "pm", "pl", "t"], |path, line_num, line| {
        if count >= limit { return; }

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

        todos.get_mut(category).unwrap().push((rel_path, line_num, content));
        count += 1;
    })?;

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

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find function callers
pub fn cmd_callers(root: &Path, function_name: &str, limit: usize) -> Result<()> {
    let start = Instant::now();
    // Pattern for function calls: obj.func(), ->func(), func()
    let pattern = format!(r"[.>]{function_name}\s*\(|^\s*{function_name}\s*\(|->{function_name}\s*\(|&{function_name}\s*\(");
    // Skip definitions in Kotlin/Java/Swift/Perl
    let def_pattern = Regex::new(&format!(r"\b(fun|func|def|void|private|public|protected|override|internal|fileprivate|open|sub)\s+{function_name}\s*[<({{\[]"))?;

    let mut by_file: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    let mut count = 0;

    search_files(root, &pattern, &["kt", "java", "swift", "m", "h", "pm", "pl", "t"], |path, line_num, line| {
        if count >= limit { return; }
        if def_pattern.is_match(line) { return; } // Skip definitions

        let rel_path = relative_path(root, path);
        let content: String = line.chars().take(70).collect();

        by_file.entry(rel_path).or_default().push((line_num, content));
        count += 1;
    })?;

    let total: usize = by_file.values().map(|v| v.len()).sum();
    println!("{}", format!("Callers of '{}' ({}):", function_name, total).bold());

    for (path, items) in by_file.iter() {
        println!("\n  {}:", path.cyan());
        for (line_num, content) in items {
            println!("    :{} {}", line_num, content);
        }
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Show call hierarchy (callers tree) for a function
pub fn cmd_call_tree(root: &Path, function_name: &str, max_depth: usize, limit_per_level: usize) -> Result<()> {
    let start = Instant::now();

    println!("{}", format!("Call tree for '{}':", function_name).bold());
    println!("  {}", function_name.cyan());

    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    visited.insert(function_name.to_string());

    build_call_tree(root, function_name, 1, max_depth, limit_per_level, &mut visited)?;

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
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
            println!("{}← {} ({}:{})", indent, caller_func.yellow(), file_path, line_num);
            // Recursively find callers of this function
            build_call_tree(root, &caller_func, current_depth + 1, max_depth, limit, visited)?;
        } else {
            println!("{}← {} (recursive)", indent, caller_func.dimmed());
        }
    }

    Ok(())
}

/// Find functions that call the given function
fn find_caller_functions(root: &Path, function_name: &str, limit: usize) -> Result<Vec<(String, String, usize)>> {
    let pattern = format!(r"[.>]{function_name}\s*\(|^\s*{function_name}\s*\(|->{function_name}\s*\(|&{function_name}\s*\(");
    let def_pattern = Regex::new(&format!(r"\b(fun|func|def|void|private|public|protected|override|internal|fileprivate|open|sub)\s+{function_name}\s*[<({{\[]"))?;

    // Pattern to find function definitions
    let func_def_re = Regex::new(r"(?:fun|func|def|void|private|public|protected|override|internal|open|sub)\s+(\w+)\s*[<(\[]")?;

    let mut results: Vec<(String, String, usize)> = vec![];
    let mut files_with_calls: HashMap<PathBuf, Vec<usize>> = HashMap::new();

    // First pass: find all files and line numbers with calls
    search_files(root, &pattern, &["kt", "java", "swift", "m", "h", "pm", "pl", "t"], |path, line_num, line| {
        if results.len() >= limit * 3 { return; } // Collect more to filter later
        if def_pattern.is_match(line) { return; }

        files_with_calls.entry(path.to_path_buf()).or_default().push(line_num);
    })?;

    // Second pass: for each call location, find the containing function
    for (file_path, call_lines) in files_with_calls {
        if results.len() >= limit { break; }

        let content = match std::fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let lines: Vec<&str> = content.lines().collect();
        let rel_path = relative_path(root, &file_path);

        for call_line in call_lines {
            if results.len() >= limit { break; }

            // Search backwards to find the containing function
            if let Some((func_name, func_line)) = find_containing_function(&lines, call_line, &func_def_re) {
                // Avoid adding the same function twice for this target
                if !results.iter().any(|(f, p, _)| f == &func_name && p == &rel_path) {
                    results.push((func_name, rel_path.clone(), func_line));
                }
            }
        }
    }

    Ok(results)
}

/// Find the function that contains a given line number
fn find_containing_function(lines: &[&str], target_line: usize, func_def_re: &Regex) -> Option<(String, usize)> {
    // Search backwards from the target line to find a function definition
    let start_idx = (target_line.saturating_sub(1)).min(lines.len().saturating_sub(1));

    for i in (0..=start_idx).rev() {
        let line = lines[i];
        if let Some(caps) = func_def_re.captures(line) {
            if let Some(name) = caps.get(1) {
                return Some((name.as_str().to_string(), i + 1));
            }
        }
    }

    None
}

/// Find Dagger @Provides/@Binds for a type
pub fn cmd_provides(root: &Path, type_name: &str, limit: usize) -> Result<()> {
    let start = Instant::now();

    let mut results: Vec<(String, usize, String)> = vec![];

    // Walk files and search with context
    use ignore::WalkBuilder;
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .build();

    for entry in walker.filter_map(|e| e.ok()) {
        if results.len() >= limit {
            break;
        }
        let path = entry.path();
        if !path.extension().map(|e| e == "kt" || e == "java").unwrap_or(false) {
            continue;
        }

        if let Ok(content) = std::fs::read_to_string(path) {
            let lines: Vec<&str> = content.lines().collect();
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
                    let kotlin_pattern = format!(r":\s*\w*{}\b", regex::escape(type_name));
                    let java_pattern = format!(r"\b\w*{}\s+\w+\s*\(", regex::escape(type_name));
                    let matches_kotlin = Regex::new(&kotlin_pattern).map(|re| re.is_match(&context)).unwrap_or(false);
                    let matches_java = Regex::new(&java_pattern).map(|re| re.is_match(&context)).unwrap_or(false);
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

    println!("{}", format!("Providers for '{}' ({}):", type_name, results.len()).bold());

    for (path, line_num, content) in &results {
        println!("  {}:{}", path, line_num);
        let truncated: String = content.chars().take(100).collect();
        println!("    {}", truncated);
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find suspend functions
pub fn cmd_suspend(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();
    let pattern = r"suspend\s+fun\s+\w+";
    let func_regex = Regex::new(r"suspend\s+fun\s+(\w+)")?;

    let mut suspends: Vec<(String, String, usize)> = vec![];

    search_files(root, pattern, &["kt"], |path, line_num, line| {
        if suspends.len() >= limit { return; }

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

    println!("{}", format!("Suspend functions ({}):", suspends.len()).bold());

    for (func_name, path, line_num) in &suspends {
        println!("  {}: {}:{}", func_name.cyan(), path, line_num);
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find @Composable functions
pub fn cmd_composables(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();
    let pattern = r"@Composable";

    let mut composables: Vec<(String, String, usize)> = vec![];
    let mut pending_composable: Option<(PathBuf, usize)> = None;
    let func_regex = Regex::new(r"fun\s+(\w+)\s*\(")?;

    // This is a simplified version - proper impl would need context
    search_files(root, pattern, &["kt"], |path, line_num, line| {
        if composables.len() >= limit { return; }

        if line.contains("@Composable") {
            pending_composable = Some((path.to_path_buf(), line_num));
        }

        if pending_composable.is_some() {
            if let Some(caps) = func_regex.captures(line) {
                let func_name = caps.get(1).unwrap().as_str().to_string();

                if let Some(q) = query {
                    if !func_name.to_lowercase().contains(&q.to_lowercase()) {
                        pending_composable = None;
                        return;
                    }
                }

                let (p, ln) = pending_composable.take().unwrap();
                let rel_path = relative_path(root, &p);
                composables.push((func_name, rel_path, ln));
            }
        }
    })?;

    println!("{}", format!("@Composable functions ({}):", composables.len()).bold());

    for (func_name, path, line_num) in &composables {
        println!("  {}: {}:{}", func_name.cyan(), path, line_num);
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find @Deprecated annotations
pub fn cmd_deprecated(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();
    // Kotlin/Java: @Deprecated, Swift: @available(*, deprecated)
    // Perl: DEPRECATED in comments or POD =head DEPRECATED
    let pattern = r"@Deprecated|@available\s*\([^)]*deprecated|#.*DEPRECATED|=head.*DEPRECATED";

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files(root, pattern, &["kt", "java", "swift", "m", "h", "pm", "pl", "t"], |path, line_num, line| {
        if items.len() >= limit { return; }

        if let Some(q) = query {
            if !line.to_lowercase().contains(&q.to_lowercase()) {
                return;
            }
        }

        let rel_path = relative_path(root, path);
        let content: String = line.trim().chars().take(80).collect();
        items.push((rel_path, line_num, content));
    })?;

    println!("{}", format!("@Deprecated items ({}):", items.len()).bold());

    for (path, line_num, content) in &items {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find @Suppress annotations
pub fn cmd_suppress(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();
    let pattern = r"@Suppress";

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files(root, pattern, &["kt"], |path, line_num, line| {
        if items.len() >= limit { return; }

        if let Some(q) = query {
            if !line.to_lowercase().contains(&q.to_lowercase()) {
                return;
            }
        }

        let rel_path = relative_path(root, path);
        let content: String = line.trim().chars().take(80).collect();
        items.push((rel_path, line_num, content));
    })?;

    println!("{}", format!("@Suppress annotations ({}):", items.len()).bold());

    for (path, line_num, content) in &items {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find @Inject points for a type
pub fn cmd_inject(root: &Path, type_name: &str, limit: usize) -> Result<()> {
    let start = Instant::now();
    let pattern = r"@Inject";

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files(root, pattern, &["kt", "java"], |path, line_num, line| {
        if items.len() >= limit { return; }

        if !line.contains(type_name) && !line.contains("@Inject") {
            return;
        }

        let rel_path = relative_path(root, path);
        let content: String = line.trim().chars().take(80).collect();
        items.push((rel_path, line_num, content));
    })?;

    // Filter to those containing type_name
    let filtered: Vec<_> = items.iter()
        .filter(|(_, _, line)| line.contains(type_name))
        .take(limit)
        .collect();

    println!("{}", format!("@Inject points for '{}' ({}):", type_name, filtered.len()).bold());

    for (path, line_num, content) in &filtered {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find uses of specific annotation
pub fn cmd_annotations(root: &Path, annotation: &str, limit: usize) -> Result<()> {
    let start = Instant::now();
    // Normalize annotation (add @ if missing for Java/Kotlin/Swift/ObjC)
    // For Perl, attributes are like :lvalue, :method
    let search_annotation = if annotation.starts_with('@') || annotation.starts_with(':') {
        annotation.to_string()
    } else {
        format!("@{}", annotation)
    };
    let pattern = regex::escape(&search_annotation);

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files(root, &pattern, &["kt", "java", "swift", "m", "h", "pm", "pl", "t"], |path, line_num, line| {
        if items.len() >= limit { return; }

        let rel_path = relative_path(root, path);
        let content: String = line.trim().chars().take(80).collect();
        items.push((rel_path, line_num, content));
    })?;

    println!("{}", format!("Classes with {} ({}):", search_annotation, items.len()).bold());

    for (path, line_num, content) in &items {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find deeplink definitions
pub fn cmd_deeplinks(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();
    // Search for deeplink patterns
    // Android: @DeepLink, DeepLinkHandler, @AppLink, NavDeepLink
    // iOS: openURL, application(_:open:, handleOpen, CFBundleURLSchemes, UniversalLink
    let pattern = r#"://|deeplink|@DeepLink|DeepLinkHandler|@AppLink|NavDeepLink|openURL|application\([^)]*open:|handleOpen|CFBundleURLSchemes|UniversalLink|NSUserActivity"#;

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files(root, pattern, &["kt", "java", "xml", "swift", "m", "h", "plist"], |path, line_num, line| {
        if items.len() >= limit { return; }

        if let Some(q) = query {
            if !line.to_lowercase().contains(&q.to_lowercase()) {
                return;
            }
        }

        let rel_path = relative_path(root, path);
        let content: String = line.trim().chars().take(100).collect();
        items.push((rel_path, line_num, content));
    })?;

    println!("{}", format!("Deeplinks ({}):", items.len()).bold());

    for (path, line_num, content) in &items {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find extension functions/types
pub fn cmd_extensions(root: &Path, receiver_type: &str, limit: usize) -> Result<()> {
    let start = Instant::now();
    // Kotlin: fun ReceiverType.functionName
    // Swift: extension ReceiverType
    let kotlin_pattern = format!(r"fun\s+{}\.(\w+)", regex::escape(receiver_type));
    let swift_pattern = format!(r"extension\s+{}", regex::escape(receiver_type));
    let pattern = format!(r"{}|{}", kotlin_pattern, swift_pattern);

    let kotlin_regex = Regex::new(&kotlin_pattern)?;
    let swift_regex = Regex::new(&swift_pattern)?;

    let mut items: Vec<(String, String, usize, String)> = vec![]; // (name, path, line, lang)

    search_files(root, &pattern, &["kt", "swift"], |path, line_num, line| {
        if items.len() >= limit { return; }

        let rel_path = relative_path(root, path);

        if let Some(caps) = kotlin_regex.captures(line) {
            let func_name = caps.get(1).unwrap().as_str().to_string();
            items.push((func_name, rel_path, line_num, "kt".to_string()));
        } else if swift_regex.is_match(line) {
            let content: String = line.trim().chars().take(60).collect();
            items.push((content, rel_path, line_num, "swift".to_string()));
        }
    })?;

    println!("{}", format!("Extensions for {} ({}):", receiver_type, items.len()).bold());

    for (name, path, line_num, lang) in &items {
        if lang == "kt" {
            println!("  {}.{}: {}:{}", receiver_type.cyan(), name, path, line_num);
        } else {
            println!("  {}:{} {}", path.cyan(), line_num, name);
        }
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find Flow declarations
pub fn cmd_flows(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();
    let pattern = r"(StateFlow|SharedFlow|MutableStateFlow|MutableSharedFlow|Flow<)";
    let flow_regex = Regex::new(r"(StateFlow|SharedFlow|MutableStateFlow|MutableSharedFlow|Flow)<")?;

    let mut items: Vec<(String, String, usize, String)> = vec![];

    search_files(root, pattern, &["kt"], |path, line_num, line| {
        if items.len() >= limit { return; }

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

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find @Preview functions
pub fn cmd_previews(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();
    let pattern = r"@Preview";
    let func_regex = Regex::new(r"fun\s+(\w+)\s*\(")?;

    let mut items: Vec<(String, String, usize)> = vec![];
    let mut pending_preview: Option<(PathBuf, usize)> = None;

    search_files(root, pattern, &["kt"], |path, line_num, line| {
        if items.len() >= limit { return; }

        if line.contains("@Preview") {
            pending_preview = Some((path.to_path_buf(), line_num));
        }

        if pending_preview.is_some() {
            if let Some(caps) = func_regex.captures(line) {
                let func_name = caps.get(1).unwrap().as_str().to_string();

                if let Some(q) = query {
                    if !func_name.to_lowercase().contains(&q.to_lowercase()) {
                        pending_preview = None;
                        return;
                    }
                }

                let (p, ln) = pending_preview.take().unwrap();
                let rel_path = relative_path(root, &p);
                items.push((func_name, rel_path, ln));
            }
        }
    })?;

    println!("{}", format!("@Preview functions ({}):", items.len()).bold());

    for (func_name, path, line_num) in &items {
        println!("  {}: {}:{}", func_name.cyan(), path, line_num);
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}
