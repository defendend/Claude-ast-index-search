mod db;
mod indexer;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::*;
use crossbeam_channel as channel;
use grep_regex::RegexMatcher;
use grep_searcher::sinks::UTF8;
use grep_searcher::{MmapChoice, SearcherBuilder};
use ignore::WalkBuilder;
use regex::Regex;
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser)]
#[command(name = "kotlin-index")]
#[command(about = "Fast code search for Android/Kotlin/Java projects")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Find TODO/FIXME/HACK comments
    Todo {
        /// Pattern to search
        #[arg(default_value = "TODO|FIXME|HACK")]
        pattern: String,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Find callers of a function
    Callers {
        /// Function name
        function_name: String,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Find @Provides/@Binds for a type
    Provides {
        /// Type name
        type_name: String,
        /// Max results
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Find suspend functions
    Suspend {
        /// Filter by name
        query: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Find @Composable functions
    Composables {
        /// Filter by name
        query: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Find @Deprecated items
    Deprecated {
        /// Filter by name
        query: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Find @Suppress annotations
    Suppress {
        /// Filter by suppression type (e.g., UNCHECKED_CAST)
        query: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Find @Inject points for a type
    Inject {
        /// Type name to search
        type_name: String,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Find classes with annotation
    Annotations {
        /// Annotation name (e.g., @Module, @Inject)
        annotation: String,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Find deeplinks
    Deeplinks {
        /// Filter by pattern
        query: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Find extension functions
    Extensions {
        /// Receiver type (e.g., String, View)
        receiver_type: String,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Find Flow/StateFlow/SharedFlow
    Flows {
        /// Filter by name
        query: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Find @Preview functions
    Previews {
        /// Filter by name
        query: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    // === Index Commands ===
    /// Initialize index for current project
    Init,
    /// Rebuild index (full reindex)
    Rebuild {
        /// Index type: files, symbols, modules, or all
        #[arg(long, default_value = "all")]
        r#type: String,
        /// Also index module dependencies from build.gradle
        #[arg(long)]
        deps: bool,
    },
    /// Update index (incremental)
    Update,
    /// Show index statistics
    Stats,
    /// Universal search (files + symbols)
    Search {
        /// Search query
        query: String,
        /// Max results
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Find files by name
    File {
        /// File name pattern
        pattern: String,
        /// Exact match
        #[arg(long)]
        exact: bool,
        /// Max results
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Find symbols (classes, interfaces, functions)
    Symbol {
        /// Symbol name
        name: String,
        /// Symbol type: class, interface, function, property
        #[arg(long, short = 't')]
        r#type: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Find class or interface
    Class {
        /// Class name
        name: String,
        /// Max results
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Find implementations (subclasses/implementors)
    Implementations {
        /// Parent class/interface name
        parent: String,
        /// Max results
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Show class hierarchy
    Hierarchy {
        /// Class name
        name: String,
    },
    /// Find modules
    Module {
        /// Module name pattern
        pattern: String,
        /// Max results
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Show module dependencies
    Deps {
        /// Module name
        module: String,
    },
    /// Show modules that depend on this module
    Dependents {
        /// Module name
        module: String,
    },
    /// Find usages of a symbol
    Usages {
        /// Symbol name
        symbol: String,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Show symbols in a file
    Outline {
        /// File path
        file: String,
    },
    /// Show imports in a file
    Imports {
        /// File path
        file: String,
    },
    /// Show public API of a module
    Api {
        /// Module path (e.g., features/payments/api)
        module_path: String,
        /// Max results
        #[arg(short, long, default_value = "100")]
        limit: usize,
    },
    /// Show changed symbols (git diff)
    Changed {
        /// Base branch
        #[arg(long, default_value = "origin/trunk")]
        base: String,
    },
    /// Show version
    Version,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = find_project_root()?;

    match cli.command {
        Commands::Todo { pattern, limit } => cmd_todo(&root, &pattern, limit),
        Commands::Callers { function_name, limit } => cmd_callers(&root, &function_name, limit),
        Commands::Provides { type_name, limit } => cmd_provides(&root, &type_name, limit),
        Commands::Suspend { query, limit } => cmd_suspend(&root, query.as_deref(), limit),
        Commands::Composables { query, limit } => cmd_composables(&root, query.as_deref(), limit),
        Commands::Deprecated { query, limit } => cmd_deprecated(&root, query.as_deref(), limit),
        Commands::Suppress { query, limit } => cmd_suppress(&root, query.as_deref(), limit),
        Commands::Inject { type_name, limit } => cmd_inject(&root, &type_name, limit),
        Commands::Annotations { annotation, limit } => cmd_annotations(&root, &annotation, limit),
        Commands::Deeplinks { query, limit } => cmd_deeplinks(&root, query.as_deref(), limit),
        Commands::Extensions { receiver_type, limit } => cmd_extensions(&root, &receiver_type, limit),
        Commands::Flows { query, limit } => cmd_flows(&root, query.as_deref(), limit),
        Commands::Previews { query, limit } => cmd_previews(&root, query.as_deref(), limit),
        // Index commands
        Commands::Init => cmd_init(&root),
        Commands::Rebuild { r#type, deps } => cmd_rebuild(&root, &r#type, deps),
        Commands::Update => cmd_update(&root),
        Commands::Stats => cmd_stats(&root),
        Commands::Search { query, limit } => cmd_search(&root, &query, limit),
        Commands::File { pattern, exact, limit } => cmd_file(&root, &pattern, exact, limit),
        Commands::Symbol { name, r#type, limit } => cmd_symbol(&root, &name, r#type.as_deref(), limit),
        Commands::Class { name, limit } => cmd_class(&root, &name, limit),
        Commands::Implementations { parent, limit } => cmd_implementations(&root, &parent, limit),
        Commands::Hierarchy { name } => cmd_hierarchy(&root, &name),
        Commands::Module { pattern, limit } => cmd_module(&root, &pattern, limit),
        Commands::Deps { module } => cmd_deps(&root, &module),
        Commands::Dependents { module } => cmd_dependents(&root, &module),
        Commands::Usages { symbol, limit } => cmd_usages(&root, &symbol, limit),
        Commands::Outline { file } => cmd_outline(&root, &file),
        Commands::Imports { file } => cmd_imports(&root, &file),
        Commands::Api { module_path, limit } => cmd_api(&root, &module_path, limit),
        Commands::Changed { base } => cmd_changed(&root, &base),
        Commands::Version => {
            println!("kotlin-index-rs v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}

fn find_project_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    for ancestor in cwd.ancestors() {
        if ancestor.join("settings.gradle").exists()
            || ancestor.join("settings.gradle.kts").exists()
        {
            return Ok(ancestor.to_path_buf());
        }
    }
    Ok(cwd)
}

/// Fast parallel file search using grep-searcher (ripgrep internals)
fn search_files<F>(root: &Path, pattern: &str, extensions: &[&str], mut handler: F) -> Result<()>
where
    F: FnMut(&Path, usize, &str),
{
    use std::collections::HashSet;
    use std::sync::Arc;

    let matcher = RegexMatcher::new(pattern).context("Invalid regex pattern")?;

    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .threads(num_cpus())
        .build_parallel();

    // Use crossbeam for faster channel (bounded to prevent memory bloat)
    let (tx, rx) = channel::bounded(10000);

    // Use HashSet for O(1) extension lookup instead of O(n) linear search
    let extensions: Arc<HashSet<String>> = Arc::new(
        extensions.iter().map(|s| s.to_string()).collect()
    );

    walker.run(|| {
        let tx = tx.clone();
        let matcher = matcher.clone();
        let extensions = Arc::clone(&extensions);

        // Create optimized searcher ONCE per thread (not per file!)
        // SAFETY: memory-mapped files are safe when files aren't modified during search
        let mut searcher = SearcherBuilder::new()
            .memory_map(unsafe { MmapChoice::auto() })
            .line_number(true)
            .build();

        Box::new(move |entry| {
            if let Ok(entry) = entry {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    // Fast O(1) HashSet lookup
                    if extensions.contains(ext.to_str().unwrap_or("")) {
                        let path_buf = path.to_path_buf();

                        let _ = searcher.search_path(
                            &matcher,
                            path,
                            UTF8(|line_num, line| {
                                let _ = tx.send((path_buf.clone(), line_num as usize, line.trim_end().to_string()));
                                Ok(true)
                            }),
                        );
                    }
                }
            }
            ignore::WalkState::Continue
        })
    });

    drop(tx);

    for (path, line_num, line) in rx {
        handler(&path, line_num, &line);
    }

    Ok(())
}

/// Fast parallel file search with early termination support
fn search_files_limited<F>(
    root: &Path,
    pattern: &str,
    extensions: &[&str],
    limit: usize,
    mut handler: F,
) -> Result<()>
where
    F: FnMut(&Path, usize, &str),
{
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    let matcher = RegexMatcher::new(pattern).context("Invalid regex pattern")?;

    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .threads(num_cpus())
        .build_parallel();

    let (tx, rx) = channel::bounded(limit.max(1000));

    let extensions: Arc<HashSet<String>> = Arc::new(
        extensions.iter().map(|s| s.to_string()).collect()
    );

    // Shared counter for early termination
    let found_count = Arc::new(AtomicUsize::new(0));
    let should_stop = Arc::new(AtomicBool::new(false));

    walker.run(|| {
        let tx = tx.clone();
        let matcher = matcher.clone();
        let extensions = Arc::clone(&extensions);
        let found_count = Arc::clone(&found_count);
        let should_stop = Arc::clone(&should_stop);

        // SAFETY: memory-mapped files are safe when files aren't modified during search
        let mut searcher = SearcherBuilder::new()
            .memory_map(unsafe { MmapChoice::auto() })
            .line_number(true)
            .build();

        Box::new(move |entry| {
            // Check early termination
            if should_stop.load(Ordering::Relaxed) {
                return ignore::WalkState::Quit;
            }

            if let Ok(entry) = entry {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    if extensions.contains(ext.to_str().unwrap_or("")) {
                        let path_buf = path.to_path_buf();
                        let found_count = Arc::clone(&found_count);
                        let should_stop = Arc::clone(&should_stop);

                        let _ = searcher.search_path(
                            &matcher,
                            path,
                            UTF8(|line_num, line| {
                                // Check if we should stop
                                if should_stop.load(Ordering::Relaxed) {
                                    return Ok(false); // Stop searching this file
                                }

                                let count = found_count.fetch_add(1, Ordering::Relaxed);
                                if count >= limit {
                                    should_stop.store(true, Ordering::Relaxed);
                                    return Ok(false);
                                }

                                let _ = tx.send((path_buf.clone(), line_num as usize, line.trim_end().to_string()));
                                Ok(true)
                            }),
                        );
                    }
                }
            }
            ignore::WalkState::Continue
        })
    });

    drop(tx);

    let mut count = 0;
    for (path, line_num, line) in rx {
        if count >= limit {
            break;
        }
        handler(&path, line_num, &line);
        count += 1;
    }

    Ok(())
}

fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

// === Commands ===

fn cmd_todo(root: &Path, pattern: &str, limit: usize) -> Result<()> {
    let start = Instant::now();
    let search_pattern = format!(r"//.*({pattern})|#.*({pattern})");

    let mut todos: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
    todos.insert("TODO".to_string(), vec![]);
    todos.insert("FIXME".to_string(), vec![]);
    todos.insert("HACK".to_string(), vec![]);
    todos.insert("OTHER".to_string(), vec![]);

    let mut count = 0;

    search_files(root, &search_pattern, &["kt", "java"], |path, line_num, line| {
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

fn cmd_callers(root: &Path, function_name: &str, limit: usize) -> Result<()> {
    let start = Instant::now();
    let pattern = format!(r"[.>]{function_name}\s*\(|^\s*{function_name}\s*\(");
    let def_pattern = Regex::new(&format!(r"\b(fun|def|void|private|public|protected|override)\s+{function_name}\s*\("))?;

    let mut by_file: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    let mut count = 0;

    search_files(root, &pattern, &["kt", "java"], |path, line_num, line| {
        if count >= limit { return; }
        if def_pattern.is_match(&line) { return; } // Skip definitions

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

fn cmd_provides(root: &Path, type_name: &str, limit: usize) -> Result<()> {
    let start = Instant::now();

    // Search for @Provides or @Binds, then look at the function signature
    // Pattern: @Binds followed by function returning the type
    let pattern = format!(r"@(Provides|Binds)[^\n]*\n[^\n]*:\s*{}\b", regex::escape(type_name));

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

fn cmd_suspend(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();
    let pattern = r"suspend\s+fun\s+\w+";
    let func_regex = Regex::new(r"suspend\s+fun\s+(\w+)")?;

    let mut suspends: Vec<(String, String, usize)> = vec![];

    search_files(root, pattern, &["kt"], |path, line_num, line| {
        if suspends.len() >= limit { return; }

        if let Some(caps) = func_regex.captures(&line) {
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

fn cmd_composables(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
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
            if let Some(caps) = func_regex.captures(&line) {
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

fn cmd_deprecated(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();
    let pattern = r"@Deprecated";

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files(root, pattern, &["kt", "java"], |path, line_num, line| {
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

fn cmd_suppress(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
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

fn cmd_inject(root: &Path, type_name: &str, limit: usize) -> Result<()> {
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

fn cmd_annotations(root: &Path, annotation: &str, limit: usize) -> Result<()> {
    let start = Instant::now();
    // Normalize annotation (add @ if missing)
    let search_annotation = if annotation.starts_with('@') {
        annotation.to_string()
    } else {
        format!("@{}", annotation)
    };
    let pattern = regex::escape(&search_annotation);

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files(root, &pattern, &["kt", "java"], |path, line_num, line| {
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

fn cmd_deeplinks(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();
    // Search for deeplink patterns (common Android deeplink annotations and schemes)
    let pattern = r#"://|deeplink|@DeepLink|DeepLinkHandler|@AppLink|NavDeepLink"#;

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files(root, pattern, &["kt", "java", "xml"], |path, line_num, line| {
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

fn cmd_extensions(root: &Path, receiver_type: &str, limit: usize) -> Result<()> {
    let start = Instant::now();
    // Pattern: fun ReceiverType.functionName
    let pattern = format!(r"fun\s+{}\.(\w+)", regex::escape(receiver_type));
    let func_regex = Regex::new(&format!(r"fun\s+{}\.(\w+)", regex::escape(receiver_type)))?;

    let mut items: Vec<(String, String, usize)> = vec![];

    search_files(root, &pattern, &["kt"], |path, line_num, line| {
        if items.len() >= limit { return; }

        if let Some(caps) = func_regex.captures(&line) {
            let func_name = caps.get(1).unwrap().as_str().to_string();
            let rel_path = relative_path(root, path);
            items.push((func_name, rel_path, line_num));
        }
    })?;

    println!("{}", format!("Extension functions for {} ({}):", receiver_type, items.len()).bold());

    for (func_name, path, line_num) in &items {
        println!("  {}.{}: {}:{}", receiver_type.cyan(), func_name, path, line_num);
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_flows(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();
    let pattern = r"(StateFlow|SharedFlow|MutableStateFlow|MutableSharedFlow|Flow<)";
    let flow_regex = Regex::new(r"(StateFlow|SharedFlow|MutableStateFlow|MutableSharedFlow|Flow)<")?;

    let mut items: Vec<(String, String, usize, String)> = vec![];

    search_files(root, pattern, &["kt"], |path, line_num, line| {
        if items.len() >= limit { return; }

        if let Some(caps) = flow_regex.captures(&line) {
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

fn cmd_previews(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
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
            if let Some(caps) = func_regex.captures(&line) {
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

// === Index Commands ===

fn cmd_init(root: &Path) -> Result<()> {
    let start = Instant::now();

    if db::db_exists(root) {
        println!("{}", "Index already exists. Use 'rebuild' to reindex.".yellow());
        return Ok(());
    }

    let conn = db::open_db(root)?;
    db::init_db(&conn)?;

    println!("{}", "Initialized empty index.".green());
    println!("Run 'kotlin-index rebuild' to build the index.");

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_rebuild(root: &Path, index_type: &str, index_deps: bool) -> Result<()> {
    let start = Instant::now();

    let mut conn = db::open_db(root)?;
    db::init_db(&conn)?;

    match index_type {
        "all" => {
            println!("{}", "Rebuilding full index...".cyan());
            db::clear_db(&conn)?;
            let file_count = indexer::index_directory(&mut conn, root, true)?;
            let module_count = indexer::index_modules(&conn, root)?;

            if index_deps {
                println!("{}", "Indexing module dependencies...".cyan());
                let dep_count = indexer::index_module_dependencies(&mut conn, root, true)?;
                println!(
                    "{}",
                    format!("Indexed {} files, {} modules, {} dependencies", file_count, module_count, dep_count).green()
                );
            } else {
                println!(
                    "{}",
                    format!("Indexed {} files, {} modules", file_count, module_count).green()
                );
            }
        }
        "files" | "symbols" => {
            println!("{}", "Rebuilding symbols index...".cyan());
            conn.execute("DELETE FROM symbols", [])?;
            conn.execute("DELETE FROM files", [])?;
            let file_count = indexer::index_directory(&mut conn, root, true)?;
            println!("{}", format!("Indexed {} files", file_count).green());
        }
        "modules" => {
            println!("{}", "Rebuilding modules index...".cyan());
            conn.execute("DELETE FROM module_deps", [])?;
            conn.execute("DELETE FROM modules", [])?;
            let module_count = indexer::index_modules(&conn, root)?;

            if index_deps {
                println!("{}", "Indexing module dependencies...".cyan());
                let dep_count = indexer::index_module_dependencies(&mut conn, root, true)?;
                println!(
                    "{}",
                    format!("Indexed {} modules, {} dependencies", module_count, dep_count).green()
                );
            } else {
                println!("{}", format!("Indexed {} modules", module_count).green());
            }
        }
        "deps" => {
            println!("{}", "Indexing module dependencies...".cyan());
            let dep_count = indexer::index_module_dependencies(&mut conn, root, true)?;
            println!("{}", format!("Indexed {} dependencies", dep_count).green());
        }
        _ => {
            println!("{}", format!("Unknown index type: {}", index_type).red());
        }
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_update(root: &Path) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'kotlin-index init' first.".red()
        );
        return Ok(());
    }

    let mut conn = db::open_db(root)?;

    println!("{}", "Checking for changes...".cyan());
    let (updated, changed, deleted) = indexer::update_directory_incremental(&mut conn, root, true)?;

    if updated == 0 && deleted == 0 {
        println!("{}", "Index is up to date.".green());
    } else {
        println!(
            "{}",
            format!(
                "Updated: {} files ({} changed, {} deleted)",
                updated + deleted,
                changed,
                deleted
            )
            .green()
        );
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_stats(root: &Path) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'kotlin-index init' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;
    let stats = db::get_stats(&conn)?;
    let db_path = db::get_db_path(root)?;
    let db_size = std::fs::metadata(&db_path)
        .map(|m| m.len())
        .unwrap_or(0);

    println!("{}", "Index Statistics:".bold());
    println!("  Files:   {}", stats.file_count);
    println!("  Symbols: {}", stats.symbol_count);
    println!("  Refs:    {}", stats.refs_count);
    println!("  Modules: {}", stats.module_count);
    println!("  DB size: {:.2} MB", db_size as f64 / 1024.0 / 1024.0);
    println!("  DB path: {}", db_path.display());

    Ok(())
}

fn cmd_search(root: &Path, query: &str, limit: usize) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'kotlin-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // Search in files
    let files = db::find_files(&conn, query, limit)?;

    // Search in symbols using FTS
    let fts_query = format!("{}*", query); // Prefix search
    let symbols = db::search_symbols(&conn, &fts_query, limit)?;

    println!("{}", format!("Search results for '{}':", query).bold());

    if !files.is_empty() {
        println!("\n{}", "Files:".cyan());
        for path in files.iter().take(10) {
            println!("  {}", path);
        }
        if files.len() > 10 {
            println!("  ... and {} more", files.len() - 10);
        }
    }

    if !symbols.is_empty() {
        println!("\n{}", "Symbols:".cyan());
        for s in symbols.iter().take(limit) {
            println!("  {} [{}]: {}:{}", s.name.cyan(), s.kind, s.path, s.line);
        }
    }

    if files.is_empty() && symbols.is_empty() {
        println!("  No results found.");
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_file(root: &Path, pattern: &str, exact: bool, limit: usize) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'kotlin-index rebuild' first.".red()
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

fn cmd_symbol(root: &Path, name: &str, kind: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'kotlin-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;
    let symbols = db::find_symbols_by_name(&conn, name, kind, limit)?;

    let kind_str = kind.map(|k| format!(" ({})", k)).unwrap_or_default();
    println!(
        "{}",
        format!("Symbols matching '{}'{}:", name, kind_str).bold()
    );

    for s in &symbols {
        println!("  {} [{}]: {}:{}", s.name.cyan(), s.kind, s.path, s.line);
        if let Some(sig) = &s.signature {
            let truncated: String = sig.chars().take(70).collect();
            println!("    {}", truncated.dimmed());
        }
    }

    if symbols.is_empty() {
        println!("  No symbols found.");
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_class(root: &Path, name: &str, limit: usize) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'kotlin-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // Single query for all class-like symbols
    let results = db::find_class_like(&conn, name, limit)?;

    println!("{}", format!("Classes matching '{}':", name).bold());

    for s in &results {
        println!("  {} [{}]: {}:{}", s.name.cyan(), s.kind, s.path, s.line);
    }

    if results.is_empty() {
        println!("  No classes found.");
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_implementations(root: &Path, parent: &str, limit: usize) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'kotlin-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;
    let impls = db::find_implementations(&conn, parent, limit)?;

    println!(
        "{}",
        format!("Implementations of '{}':", parent).bold()
    );

    for s in &impls {
        println!("  {} [{}]: {}:{}", s.name.cyan(), s.kind, s.path, s.line);
    }

    if impls.is_empty() {
        println!("  No implementations found.");
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_hierarchy(root: &Path, name: &str) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'kotlin-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // Find the class
    let classes = db::find_symbols_by_name(&conn, name, Some("class"), 1)?;
    let interfaces = db::find_symbols_by_name(&conn, name, Some("interface"), 1)?;

    let target = classes.first().or(interfaces.first());

    if target.is_none() {
        println!("{}", format!("Class '{}' not found.", name).red());
        return Ok(());
    }

    println!("{}", format!("Hierarchy for '{}':", name).bold());

    // Find parents
    let mut stmt = conn.prepare(
        "SELECT i.parent_name, i.kind FROM inheritance i JOIN symbols s ON i.child_id = s.id WHERE s.name = ?1",
    )?;
    let parents: Vec<(String, String)> = stmt
        .query_map([name], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;

    if !parents.is_empty() {
        println!("\n  {}", "Parents:".cyan());
        for (parent, kind) in &parents {
            println!("    {} ({})", parent, kind);
        }
    }

    // Find children
    let children = db::find_implementations(&conn, name, 20)?;
    if !children.is_empty() {
        println!("\n  {}", "Children:".cyan());
        for c in &children {
            println!("    {} [{}]", c.name, c.kind);
        }
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_module(root: &Path, pattern: &str, limit: usize) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'kotlin-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;

    let mut stmt = conn.prepare("SELECT name, path FROM modules WHERE name LIKE ?1 LIMIT ?2")?;
    let pattern = format!("%{}%", pattern);
    let modules: Vec<(String, String)> = stmt
        .query_map(rusqlite::params![pattern, limit as i64], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<Result<_, _>>()?;

    println!("{}", format!("Modules matching '{}':", pattern).bold());

    for (name, path) in &modules {
        println!("  {}: {}", name.cyan(), path);
    }

    if modules.is_empty() {
        println!("  No modules found.");
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_deps(root: &Path, module: &str) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'kotlin-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // Check if module deps are indexed
    let dep_count: i64 = conn.query_row("SELECT COUNT(*) FROM module_deps", [], |row| row.get(0))?;

    if dep_count == 0 {
        println!(
            "{}",
            "Module dependencies not indexed. Run 'kotlin-index rebuild --deps' to index them.".yellow()
        );
        return Ok(());
    }

    let deps = indexer::get_module_deps(&conn, module)?;

    println!(
        "{}",
        format!("Dependencies of '{}' ({}):", module, deps.len()).bold()
    );

    // Group by kind
    let mut api_deps: Vec<_> = deps.iter().filter(|(_, _, k)| k == "api").collect();
    let mut impl_deps: Vec<_> = deps.iter().filter(|(_, _, k)| k == "implementation").collect();
    let mut other_deps: Vec<_> = deps.iter().filter(|(_, _, k)| k != "api" && k != "implementation").collect();

    if !api_deps.is_empty() {
        println!("  {}:", "api".cyan());
        for (name, path, _) in &api_deps {
            println!("    {} ({})", name, path);
        }
    }

    if !impl_deps.is_empty() {
        println!("  {}:", "implementation".cyan());
        for (name, path, _) in &impl_deps {
            println!("    {} ({})", name, path);
        }
    }

    if !other_deps.is_empty() {
        println!("  {}:", "other".cyan());
        for (name, path, kind) in &other_deps {
            println!("    {} ({}) [{}]", name, path, kind);
        }
    }

    if deps.is_empty() {
        println!("  No dependencies found.");
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_dependents(root: &Path, module: &str) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'kotlin-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // Check if module deps are indexed
    let dep_count: i64 = conn.query_row("SELECT COUNT(*) FROM module_deps", [], |row| row.get(0))?;

    if dep_count == 0 {
        println!(
            "{}",
            "Module dependencies not indexed. Run 'kotlin-index rebuild --deps' to index them.".yellow()
        );
        return Ok(());
    }

    let dependents = indexer::get_module_dependents(&conn, module)?;

    println!(
        "{}",
        format!("Modules depending on '{}' ({}):", module, dependents.len()).bold()
    );

    // Group by kind
    let api_deps: Vec<_> = dependents.iter().filter(|(_, _, k)| k == "api").collect();
    let impl_deps: Vec<_> = dependents.iter().filter(|(_, _, k)| k == "implementation").collect();
    let other_deps: Vec<_> = dependents.iter().filter(|(_, _, k)| k != "api" && k != "implementation").collect();

    if !api_deps.is_empty() {
        println!("  {} ({}):", "via api".cyan(), api_deps.len());
        for (name, path, _) in &api_deps {
            println!("    {} ({})", name, path);
        }
    }

    if !impl_deps.is_empty() {
        println!("  {} ({}):", "via implementation".cyan(), impl_deps.len());
        for (name, path, _) in &impl_deps {
            println!("    {} ({})", name, path);
        }
    }

    if !other_deps.is_empty() {
        println!("  {} ({}):", "via other".cyan(), other_deps.len());
        for (name, path, kind) in &other_deps {
            println!("    {} ({}) [{}]", name, path, kind);
        }
    }

    if dependents.is_empty() {
        println!("  No dependents found.");
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_usages(root: &Path, symbol: &str, limit: usize) -> Result<()> {
    let start = Instant::now();

    // Try to use index first
    let db_path = db::get_db_path(root)?;
    if db_path.exists() {
        let conn = Connection::open(&db_path)?;

        // Check if refs table has data
        let refs_count: i64 = conn.query_row("SELECT COUNT(*) FROM refs WHERE name = ?1 LIMIT 1", params![symbol], |row| row.get(0)).unwrap_or(0);

        if refs_count > 0 {
            // Use indexed references
            let refs = db::find_references(&conn, symbol, limit)?;

            println!("{}", format!("Usages of '{}' ({}):", symbol, refs.len()).bold());

            for r in &refs {
                println!("  {}:{}", r.path.cyan(), r.line);
                if let Some(ctx) = &r.context {
                    let truncated: String = ctx.chars().take(80).collect();
                    println!("    {}", truncated);
                }
            }

            if refs.is_empty() {
                println!("  No usages found in index.");
            }

            eprintln!("\n{}", format!("Time: {:?} (indexed)", start.elapsed()).dimmed());
            return Ok(());
        }
    }

    // Fallback to grep-based search
    let pattern = format!(r"\b{}\b", regex::escape(symbol));
    let def_pattern = Regex::new(&format!(
        r"(class|interface|object|fun|val|var|typealias)\s+{}\b",
        regex::escape(symbol)
    ))?;

    let mut usages: Vec<(String, usize, String)> = vec![];

    search_files(root, &pattern, &["kt", "java"], |path, line_num, line| {
        if usages.len() >= limit { return; }

        // Skip definitions
        if def_pattern.is_match(&line) { return; }

        let rel_path = relative_path(root, path);
        let content: String = line.trim().chars().take(80).collect();
        usages.push((rel_path, line_num, content));
    })?;

    println!("{}", format!("Usages of '{}' ({}):", symbol, usages.len()).bold());

    for (path, line_num, content) in &usages {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    if usages.is_empty() {
        println!("  No usages found.");
    }

    eprintln!("\n{}", format!("Time: {:?} (grep)", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_outline(root: &Path, file: &str) -> Result<()> {
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

    // Parse symbols from file
    let class_re = Regex::new(r"(?m)^\s*((?:public|private|protected|internal|abstract|open|final|sealed|data)?\s*)(class|interface|object|enum\s+class)\s+(\w+)")?;
    let fun_re = Regex::new(r"(?m)^\s*((?:public|private|protected|internal|override|suspend)?\s*)fun\s+(?:<[^>]*>\s*)?(\w+)")?;
    let prop_re = Regex::new(r"(?m)^\s*((?:public|private|protected|internal|override|const|lateinit)?\s*)(val|var)\s+(\w+)")?;

    println!("{}", format!("Outline of {}:", file).bold());

    let mut found = false;
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

    if !found {
        println!("  No symbols found.");
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_imports(root: &Path, file: &str) -> Result<()> {
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
    let import_re = Regex::new(r"(?m)^import\s+(.+)")?;

    println!("{}", format!("Imports in {}:", file).bold());

    let mut imports: Vec<&str> = vec![];
    for line in content.lines() {
        if let Some(caps) = import_re.captures(line) {
            imports.push(caps.get(1).map(|m| m.as_str()).unwrap_or(""));
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

fn cmd_api(root: &Path, module_path: &str, limit: usize) -> Result<()> {
    let start = Instant::now();

    let module_dir = root.join(module_path);
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

fn cmd_changed(root: &Path, base: &str) -> Result<()> {
    let start = Instant::now();

    // Get list of changed files from git
    let output = std::process::Command::new("git")
        .args(["diff", "--name-only", base])
        .current_dir(root)
        .output()?;

    if !output.status.success() {
        println!("{}", format!("Failed to get git diff: {:?}", output.status).red());
        return Ok(());
    }

    let changed_files: Vec<&str> = std::str::from_utf8(&output.stdout)?
        .lines()
        .filter(|f| f.ends_with(".kt") || f.ends_with(".java"))
        .collect();

    if changed_files.is_empty() {
        println!("No Kotlin/Java files changed since {}", base);
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
