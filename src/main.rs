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
#[command(name = "ast-index")]
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
        /// Skip module dependencies indexing
        #[arg(long)]
        no_deps: bool,
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
    /// Find unused dependencies in a module
    UnusedDeps {
        /// Module name (e.g., features.payments.impl)
        module: String,
        /// Show details for each dependency
        #[arg(long, short)]
        verbose: bool,
        /// Skip transitive dependency checking
        #[arg(long)]
        no_transitive: bool,
        /// Skip XML layout checking
        #[arg(long)]
        no_xml: bool,
        /// Skip resource checking
        #[arg(long)]
        no_resources: bool,
        /// Strict mode: only check direct imports (same as --no-transitive --no-xml --no-resources)
        #[arg(long)]
        strict: bool,
    },
    /// Find class usages in XML layouts
    XmlUsages {
        /// Class name to search for
        class_name: String,
        /// Filter by module
        #[arg(long)]
        module: Option<String>,
    },
    /// Find resource usages
    ResourceUsages {
        /// Resource name (e.g., @drawable/ic_payment or R.string.app_name). Optional with --unused
        #[arg(default_value = "")]
        resource: String,
        /// Filter by module (required for --unused)
        #[arg(long)]
        module: Option<String>,
        /// Resource type filter (drawable, string, color, etc.)
        #[arg(long, short = 't')]
        r#type: Option<String>,
        /// Show unused resources in a module (requires --module)
        #[arg(long)]
        unused: bool,
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
    // === iOS Commands ===
    /// Find class usages in storyboards/xibs (iOS)
    StoryboardUsages {
        /// Class name to search for
        class_name: String,
        /// Filter by module
        #[arg(long)]
        module: Option<String>,
    },
    /// Find iOS asset usages (images, colors from xcassets)
    AssetUsages {
        /// Asset name to search for. Optional with --unused
        #[arg(default_value = "")]
        asset: String,
        /// Filter by module (required for --unused)
        #[arg(long)]
        module: Option<String>,
        /// Asset type filter (imageset, colorset, etc.)
        #[arg(long, short = 't')]
        r#type: Option<String>,
        /// Show unused assets in a module
        #[arg(long)]
        unused: bool,
    },
    /// Find SwiftUI views and state properties
    Swiftui {
        /// Filter by name or type (State, Binding, Published, ObservedObject)
        query: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Find async functions (Swift)
    AsyncFuncs {
        /// Filter by name
        query: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Find Combine publishers (Swift)
    Publishers {
        /// Filter by name
        query: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Find @MainActor functions and classes (Swift)
    MainActor {
        /// Filter by name
        query: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
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
        Commands::Rebuild { r#type, no_deps } => cmd_rebuild(&root, &r#type, !no_deps),
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
        Commands::UnusedDeps { module, verbose, no_transitive, no_xml, no_resources, strict } => {
            let check_transitive = !no_transitive && !strict;
            let check_xml = !no_xml && !strict;
            let check_resources = !no_resources && !strict;
            cmd_unused_deps(&root, &module, verbose, check_transitive, check_xml, check_resources)
        }
        Commands::XmlUsages { class_name, module } => cmd_xml_usages(&root, &class_name, module.as_deref()),
        Commands::ResourceUsages { resource, module, r#type, unused } => {
            cmd_resource_usages(&root, &resource, module.as_deref(), r#type.as_deref(), unused)
        }
        Commands::Usages { symbol, limit } => cmd_usages(&root, &symbol, limit),
        Commands::Outline { file } => cmd_outline(&root, &file),
        Commands::Imports { file } => cmd_imports(&root, &file),
        Commands::Api { module_path, limit } => cmd_api(&root, &module_path, limit),
        Commands::Changed { base } => cmd_changed(&root, &base),
        // iOS commands
        Commands::StoryboardUsages { class_name, module } => cmd_storyboard_usages(&root, &class_name, module.as_deref()),
        Commands::AssetUsages { asset, module, r#type, unused } => cmd_asset_usages(&root, &asset, module.as_deref(), r#type.as_deref(), unused),
        Commands::Swiftui { query, limit } => cmd_swiftui(&root, query.as_deref(), limit),
        Commands::AsyncFuncs { query, limit } => cmd_async_funcs(&root, query.as_deref(), limit),
        Commands::Publishers { query, limit } => cmd_publishers(&root, query.as_deref(), limit),
        Commands::MainActor { query, limit } => cmd_main_actor(&root, query.as_deref(), limit),
        Commands::Version => {
            println!("ast-index-rs v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}

fn find_project_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    for ancestor in cwd.ancestors() {
        // Android/Gradle markers
        if ancestor.join("settings.gradle").exists()
            || ancestor.join("settings.gradle.kts").exists()
        {
            return Ok(ancestor.to_path_buf());
        }
        // iOS/Swift markers
        if ancestor.join("Package.swift").exists() {
            return Ok(ancestor.to_path_buf());
        }
        // Check for .xcodeproj
        if let Ok(entries) = std::fs::read_dir(ancestor) {
            for entry in entries.flatten() {
                if entry.path().extension().map(|e| e == "xcodeproj").unwrap_or(false) {
                    return Ok(ancestor.to_path_buf());
                }
            }
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
#[allow(dead_code)]
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

    search_files(root, &search_pattern, &["kt", "java", "swift", "m", "h"], |path, line_num, line| {
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
    // Skip definitions in Kotlin/Java/Swift
    let def_pattern = Regex::new(&format!(r"\b(fun|func|def|void|private|public|protected|override|internal|fileprivate|open)\s+{function_name}\s*[<(]"))?;

    let mut by_file: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    let mut count = 0;

    search_files(root, &pattern, &["kt", "java", "swift", "m", "h"], |path, line_num, line| {
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
    // Kotlin/Java: @Deprecated, Swift: @available(*, deprecated)
    let pattern = r"@Deprecated|@available\s*\([^)]*deprecated";

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files(root, pattern, &["kt", "java", "swift", "m", "h"], |path, line_num, line| {
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

    search_files(root, &pattern, &["kt", "java", "swift", "m", "h"], |path, line_num, line| {
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

fn cmd_extensions(root: &Path, receiver_type: &str, limit: usize) -> Result<()> {
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

        if let Some(caps) = kotlin_regex.captures(&line) {
            let func_name = caps.get(1).unwrap().as_str().to_string();
            items.push((func_name, rel_path, line_num, "kt".to_string()));
        } else if swift_regex.is_match(&line) {
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
    println!("Run 'ast-index rebuild' to build the index.");

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_rebuild(root: &Path, index_type: &str, index_deps: bool) -> Result<()> {
    let start = Instant::now();

    let mut conn = db::open_db(root)?;
    db::init_db(&conn)?;

    // Detect project type
    let project_type = indexer::detect_project_type(root);
    let is_ios = matches!(project_type, indexer::ProjectType::IOS | indexer::ProjectType::Mixed);
    let is_android = matches!(project_type, indexer::ProjectType::Android | indexer::ProjectType::Mixed);

    match index_type {
        "all" => {
            println!("{}", "Rebuilding full index...".cyan());
            db::clear_db(&conn)?;
            let file_count = indexer::index_directory(&mut conn, root, true)?;
            let module_count = indexer::index_modules(&conn, root)?;

            // Index CocoaPods/Carthage for iOS
            if is_ios {
                let pkg_count = indexer::index_ios_package_managers(&conn, root, true)?;
                if pkg_count > 0 {
                    println!("{}", format!("Indexed {} CocoaPods/Carthage deps", pkg_count).dimmed());
                }
            }

            let mut dep_count = 0;
            let mut trans_count = 0;
            if index_deps && is_android {
                println!("{}", "Indexing module dependencies...".cyan());
                dep_count = indexer::index_module_dependencies(&mut conn, root, true)?;
                trans_count = indexer::build_transitive_deps(&mut conn, true)?;
            }

            // Android-specific: XML layouts and resources
            let mut xml_count = 0;
            let mut res_count = 0;
            let mut res_usage_count = 0;
            if is_android {
                println!("{}", "Indexing XML layouts...".cyan());
                xml_count = indexer::index_xml_usages(&mut conn, root, true)?;

                println!("{}", "Indexing resources...".cyan());
                let (rc, ruc) = indexer::index_resources(&mut conn, root, true)?;
                res_count = rc;
                res_usage_count = ruc;
            }

            // iOS-specific: storyboards and assets
            let mut sb_count = 0;
            let mut asset_count = 0;
            let mut asset_usage_count = 0;
            if is_ios {
                println!("{}", "Indexing storyboards/xibs...".cyan());
                sb_count = indexer::index_storyboard_usages(&mut conn, root, true)?;

                println!("{}", "Indexing iOS assets...".cyan());
                let (ac, auc) = indexer::index_ios_assets(&mut conn, root, true)?;
                asset_count = ac;
                asset_usage_count = auc;
            }

            // Print summary based on project type
            if is_android && is_ios {
                println!(
                    "{}",
                    format!(
                        "Indexed {} files, {} modules, {} deps, {} XML usages, {} resources, {} storyboard usages, {} assets",
                        file_count, module_count, dep_count, xml_count, res_count, sb_count, asset_count
                    ).green()
                );
            } else if is_ios {
                println!(
                    "{}",
                    format!(
                        "Indexed {} files, {} modules, {} storyboard usages, {} assets ({} usages)",
                        file_count, module_count, sb_count, asset_count, asset_usage_count
                    ).green()
                );
            } else {
                println!(
                    "{}",
                    format!(
                        "Indexed {} files, {} modules, {} deps, {} transitive, {} XML usages, {} resources ({} usages)",
                        file_count, module_count, dep_count, trans_count, xml_count, res_count, res_usage_count
                    ).green()
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
            "Index not found. Run 'ast-index init' first.".red()
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
            "Index not found. Run 'ast-index init' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;
    let stats = db::get_stats(&conn)?;
    let db_path = db::get_db_path(root)?;
    let db_size = std::fs::metadata(&db_path)
        .map(|m| m.len())
        .unwrap_or(0);

    // Detect project type
    let project_type = indexer::detect_project_type(root);

    println!("{}", "Index Statistics:".bold());
    println!("  Project:    {}", project_type.as_str());
    println!("  Files:      {}", stats.file_count);
    println!("  Symbols:    {}", stats.symbol_count);
    println!("  Refs:       {}", stats.refs_count);
    println!("  Modules:    {}", stats.module_count);

    // Show Android-specific stats if relevant
    if stats.xml_usages_count > 0 || stats.resources_count > 0 {
        println!("  XML usages: {}", stats.xml_usages_count);
        println!("  Resources:  {}", stats.resources_count);
    }

    // Show iOS-specific stats if relevant
    if stats.storyboard_usages_count > 0 || stats.ios_assets_count > 0 {
        println!("  Storyboard: {}", stats.storyboard_usages_count);
        println!("  iOS assets: {}", stats.ios_assets_count);
    }

    println!("  DB size:    {:.2} MB", db_size as f64 / 1024.0 / 1024.0);
    println!("  DB path:    {}", db_path.display());

    Ok(())
}

fn cmd_search(root: &Path, query: &str, limit: usize) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
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

fn cmd_symbol(root: &Path, name: &str, kind: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
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
            "Index not found. Run 'ast-index rebuild' first.".red()
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
            "Index not found. Run 'ast-index rebuild' first.".red()
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
            "Index not found. Run 'ast-index rebuild' first.".red()
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
            "Index not found. Run 'ast-index rebuild' first.".red()
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
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // Check if module deps are indexed
    let dep_count: i64 = conn.query_row("SELECT COUNT(*) FROM module_deps", [], |row| row.get(0))?;

    if dep_count == 0 {
        println!(
            "{}",
            "Module dependencies not indexed. Run 'ast-index rebuild' to index them.".yellow()
        );
        return Ok(());
    }

    let deps = indexer::get_module_deps(&conn, module)?;

    println!(
        "{}",
        format!("Dependencies of '{}' ({}):", module, deps.len()).bold()
    );

    // Group by kind
    let api_deps: Vec<_> = deps.iter().filter(|(_, _, k)| k == "api").collect();
    let impl_deps: Vec<_> = deps.iter().filter(|(_, _, k)| k == "implementation").collect();
    let other_deps: Vec<_> = deps.iter().filter(|(_, _, k)| k != "api" && k != "implementation").collect();

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
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // Check if module deps are indexed
    let dep_count: i64 = conn.query_row("SELECT COUNT(*) FROM module_deps", [], |row| row.get(0))?;

    if dep_count == 0 {
        println!(
            "{}",
            "Module dependencies not indexed. Run 'ast-index rebuild' to index them.".yellow()
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

fn cmd_unused_deps(
    root: &Path,
    module: &str,
    verbose: bool,
    check_transitive: bool,
    check_xml: bool,
    check_resources: bool,
) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!("{}", "Index not found. Run 'ast-index rebuild' first.".red());
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // Check if module deps are indexed
    let dep_count: i64 = conn.query_row("SELECT COUNT(*) FROM module_deps", [], |row| row.get(0))?;
    if dep_count == 0 {
        println!("{}", "Module dependencies not indexed. Run 'ast-index rebuild' first.".yellow());
        return Ok(());
    }

    // Get module id and path
    let module_info: Option<(i64, String)> = conn.query_row(
        "SELECT id, path FROM modules WHERE name = ?1",
        params![module],
        |row| Ok((row.get(0)?, row.get(1)?))
    ).ok();

    let (module_id, module_path) = match module_info {
        Some((id, p)) => (id, p),
        None => {
            println!("{}", format!("Module '{}' not found in index.", module).red());
            return Ok(());
        }
    };

    // Get all dependencies
    let deps = indexer::get_module_deps(&conn, module)?;

    if deps.is_empty() {
        println!("{}", format!("Module '{}' has no dependencies.", module).yellow());
        return Ok(());
    }

    println!("{}", format!("Analyzing {} dependencies of '{}'...", deps.len(), module).bold());
    if check_transitive || check_xml || check_resources {
        let checks: Vec<&str> = [
            if check_transitive { Some("transitive") } else { None },
            if check_xml { Some("XML") } else { None },
            if check_resources { Some("resources") } else { None },
        ].into_iter().flatten().collect();
        println!("  Checking: direct imports + {}\n", checks.join(", "));
    } else {
        println!("  Checking: direct imports only (strict mode)\n");
    }

    let module_dir = root.join(&module_path);

    // Results tracking
    #[derive(Default)]
    struct DepUsage {
        direct_count: usize,
        direct_symbols: Vec<String>,
        transitive_count: usize,
        transitive_via: Vec<(String, Vec<String>)>, // (intermediate_module, symbols)
        xml_count: usize,
        xml_usages: Vec<(String, i64)>, // (class_name, line)
        resource_count: usize,
        resource_usages: Vec<(String, String)>, // (resource_name, usage_type)
    }

    let mut dep_usages: HashMap<String, DepUsage> = HashMap::new();
    let mut unused: Vec<(String, String, String)> = vec![];
    let mut exported: Vec<(String, String, String)> = vec![]; // api deps not directly used
    let mut used_direct: Vec<(String, String, String, usize)> = vec![];
    let mut used_transitive: Vec<(String, String, String, usize)> = vec![];
    let mut used_xml: Vec<(String, String, String, usize)> = vec![];
    let mut used_resources: Vec<(String, String, String, usize)> = vec![];

    for (dep_name, dep_path, dep_kind) in &deps {
        let mut usage = DepUsage::default();

        // 1. Check direct usage
        let dep_symbols = get_module_public_symbols(&conn, root, dep_path)?;

        for symbol in &dep_symbols {
            if is_symbol_used_in_module(root, &module_dir, symbol)? {
                usage.direct_count += 1;
                if usage.direct_symbols.len() < 3 {
                    usage.direct_symbols.push(symbol.clone());
                }
            }
        }

        // 2. Check transitive usage (via api dependencies)
        if check_transitive && usage.direct_count == 0 {
            // Get transitive paths for this dependency
            let mut stmt = conn.prepare(
                "SELECT td.path FROM transitive_deps td
                 JOIN modules m ON td.dependency_id = m.id
                 WHERE td.module_id = ?1 AND m.name = ?2 AND td.depth > 1"
            )?;

            let paths: Vec<String> = stmt
                .query_map(params![module_id, dep_name], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();

            for path in paths {
                // Parse the path (e.g., "A -> B -> C")
                let parts: Vec<&str> = path.split(" -> ").collect();
                if parts.len() >= 2 {
                    let via_module = parts[1];
                    // Check if any symbols from dep are re-exported via the intermediate module
                    for symbol in &dep_symbols {
                        if is_symbol_used_in_module(root, &module_dir, symbol)? {
                            usage.transitive_count += 1;
                            let entry = usage.transitive_via.iter_mut()
                                .find(|(m, _)| m == via_module);
                            if let Some((_, symbols)) = entry {
                                if symbols.len() < 3 {
                                    symbols.push(symbol.clone());
                                }
                            } else {
                                usage.transitive_via.push((via_module.to_string(), vec![symbol.clone()]));
                            }
                            break; // Found transitive usage
                        }
                    }
                }
            }
        }

        // 3. Check XML usages
        if check_xml && usage.direct_count == 0 && usage.transitive_count == 0 {
            // Get classes from the dependency module
            let mut class_stmt = conn.prepare(
                "SELECT DISTINCT s.name FROM symbols s
                 JOIN files f ON s.file_id = f.id
                 WHERE f.path LIKE ?1 AND s.kind IN ('class', 'object')
                 LIMIT 50"
            )?;
            let dep_pattern = format!("{}%", dep_path);
            let classes: Vec<String> = class_stmt
                .query_map(params![dep_pattern], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();

            // Check if any class is used in XML layouts of the target module
            for class_name in &classes {
                let mut xml_stmt = conn.prepare(
                    "SELECT x.file_path, x.line FROM xml_usages x
                     JOIN modules m ON x.module_id = m.id
                     WHERE m.id = ?1 AND x.class_name LIKE ?2"
                )?;
                let class_pattern = format!("%{}", class_name);
                let xml_results: Vec<(String, i64)> = xml_stmt
                    .query_map(params![module_id, class_pattern], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .filter_map(|r| r.ok())
                    .collect();

                for (_file_path, line) in xml_results {
                    usage.xml_count += 1;
                    if usage.xml_usages.len() < 3 {
                        usage.xml_usages.push((class_name.clone(), line));
                    }
                }
            }
        }

        // 4. Check resource usages
        if check_resources && usage.direct_count == 0 && usage.transitive_count == 0 && usage.xml_count == 0 {
            // Get resources defined in the dependency module
            let mut res_stmt = conn.prepare(
                "SELECT r.type, r.name FROM resources r
                 JOIN modules m ON r.module_id = m.id
                 WHERE m.name = ?1
                 LIMIT 100"
            )?;
            let resources: Vec<(String, String)> = res_stmt
                .query_map(params![dep_name], |row| Ok((row.get(0)?, row.get(1)?)))?
                .filter_map(|r| r.ok())
                .collect();

            // Check if these resources are used in the target module
            for (res_type, res_name) in &resources {
                let mut usage_stmt = conn.prepare(
                    "SELECT ru.usage_type FROM resource_usages ru
                     JOIN resources r ON ru.resource_id = r.id
                     WHERE r.type = ?1 AND r.name = ?2
                     AND ru.usage_file LIKE ?3"
                )?;
                let module_pattern = format!("{}%", module_path);
                let usages: Vec<String> = usage_stmt
                    .query_map(params![res_type, res_name, module_pattern], |row| row.get(0))?
                    .filter_map(|r| r.ok())
                    .collect();

                if !usages.is_empty() {
                    usage.resource_count += usages.len();
                    if usage.resource_usages.len() < 3 {
                        usage.resource_usages.push((
                            format!("@{}/{}", res_type, res_name),
                            usages.first().cloned().unwrap_or_default()
                        ));
                    }
                }
            }
        }

        // Categorize the dependency
        let total_usage = usage.direct_count + usage.transitive_count + usage.xml_count + usage.resource_count;

        if total_usage == 0 {
            // Check if this is an api dependency (exported for consumers)
            if dep_kind == "api" {
                exported.push((dep_name.clone(), dep_path.clone(), dep_kind.clone()));
            } else {
                unused.push((dep_name.clone(), dep_path.clone(), dep_kind.clone()));
            }
        } else if usage.direct_count > 0 {
            used_direct.push((dep_name.clone(), dep_path.clone(), dep_kind.clone(), usage.direct_count));
        } else if usage.transitive_count > 0 {
            used_transitive.push((dep_name.clone(), dep_path.clone(), dep_kind.clone(), usage.transitive_count));
        } else if usage.xml_count > 0 {
            used_xml.push((dep_name.clone(), dep_path.clone(), dep_kind.clone(), usage.xml_count));
        } else if usage.resource_count > 0 {
            used_resources.push((dep_name.clone(), dep_path.clone(), dep_kind.clone(), usage.resource_count));
        }

        dep_usages.insert(dep_name.clone(), usage);
    }

    // Output results
    if verbose {
        println!("{}", "=== Direct Usage ===".cyan().bold());
        for (name, _, _, count) in &used_direct {
            let usage = dep_usages.get(name).unwrap();
            let symbols_str = if usage.direct_symbols.is_empty() {
                String::new()
            } else {
                format!(": {}", usage.direct_symbols.join(", "))
            };
            println!("  {} {} - {} symbols{}", "✓".green(), name, count, symbols_str);
        }
        if used_direct.is_empty() {
            println!("  (none)");
        }

        if check_transitive {
            println!("\n{}", "=== Transitive Usage ===".cyan().bold());
            for (name, _, _, count) in &used_transitive {
                let usage = dep_usages.get(name).unwrap();
                println!("  {} {} - {} symbols", "✓".green(), name, count);
                for (via, symbols) in &usage.transitive_via {
                    println!("    └─ via {}: {}", via, symbols.join(", "));
                }
            }
            if used_transitive.is_empty() {
                println!("  (none)");
            }
        }

        if check_xml {
            println!("\n{}", "=== XML Usage ===".cyan().bold());
            for (name, _, _, count) in &used_xml {
                let usage = dep_usages.get(name).unwrap();
                println!("  {} {} - {} usages", "✓".green(), name, count);
                for (class, line) in &usage.xml_usages {
                    println!("    └─ {}:{}", class, line);
                }
            }
            if used_xml.is_empty() {
                println!("  (none)");
            }
        }

        if check_resources {
            println!("\n{}", "=== Resource Usage ===".cyan().bold());
            for (name, _, _, count) in &used_resources {
                let usage = dep_usages.get(name).unwrap();
                println!("  {} {} - {} usages", "✓".green(), name, count);
                for (res, usage_type) in &usage.resource_usages {
                    println!("    └─ {} ({})", res, usage_type);
                }
            }
            if used_resources.is_empty() {
                println!("  (none)");
            }
        }
    }

    // Exported (api deps not directly used but intentionally re-exported)
    if !exported.is_empty() {
        println!("\n{}", "=== Exported (not directly used) ===".yellow().bold());
        for (name, _path, _kind) in &exported {
            println!("  {} {} (api)", "⚡".yellow(), name);
            if verbose {
                // Find consumers who use this exported dep
                let mut stmt = conn.prepare(
                    "SELECT DISTINCT m.name FROM module_deps md
                     JOIN modules m ON md.module_id = m.id
                     JOIN modules dep ON md.dep_module_id = dep.id
                     WHERE dep.name = ?1 AND m.name != ?2
                     LIMIT 5"
                )?;
                let consumers: Vec<String> = stmt
                    .query_map(params![name, module], |row| row.get(0))?
                    .filter_map(|r| r.ok())
                    .collect();
                if !consumers.is_empty() {
                    println!("    └─ used by: {}", consumers.join(", "));
                }
            }
        }
    }

    // Unused
    println!("\n{}", "=== Unused ===".red().bold());
    if !unused.is_empty() {
        for (name, _path, kind) in &unused {
            println!("  {} {} ({})", "✗".red(), name, kind);
            if verbose {
                println!("    - No direct imports");
                if check_transitive { println!("    - No transitive usage"); }
                if check_xml { println!("    - No XML usage"); }
                if check_resources { println!("    - No resource usage"); }
            }
        }
    } else {
        println!("  (none - all dependencies are used)");
    }

    println!("\n{}", "=== Summary ===".bold());
    let total_used = used_direct.len() + used_transitive.len() + used_xml.len() + used_resources.len();
    println!(
        "Total: {} unused, {} exported, {} used of {} dependencies",
        unused.len(),
        exported.len(),
        total_used,
        deps.len()
    );
    println!("  - Direct: {}", used_direct.len());
    if check_transitive {
        println!("  - Transitive: {}", used_transitive.len());
    }
    if check_xml {
        println!("  - XML: {}", used_xml.len());
    }
    if check_resources {
        println!("  - Resources: {}", used_resources.len());
    }
    if !exported.is_empty() {
        println!("  - Exported (api): {}", exported.len());
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Get public symbols (classes, interfaces) from a module
fn get_module_public_symbols(conn: &Connection, root: &Path, module_path: &str) -> Result<Vec<String>> {
    let mut symbols = vec![];

    // First try to get from index
    let mut stmt = conn.prepare(
        "SELECT DISTINCT s.name FROM symbols s
         JOIN files f ON s.file_id = f.id
         WHERE f.path LIKE ?1 AND s.kind IN ('class', 'interface', 'object')
         LIMIT 100"
    )?;

    let pattern = format!("{}%", module_path);
    let rows = stmt.query_map(params![pattern], |row| row.get::<_, String>(0))?;

    for row in rows {
        if let Ok(name) = row {
            symbols.push(name);
        }
    }

    // If no symbols in index, try to find by scanning files
    if symbols.is_empty() {
        let module_dir = root.join(module_path);
        if module_dir.exists() {
            let class_re = Regex::new(r"(?m)^\s*(?:public\s+)?(?:abstract\s+)?(?:data\s+)?(?:class|interface|object)\s+(\w+)")?;

            for entry in walkdir::WalkDir::new(&module_dir)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path().extension()
                        .map(|ext| ext == "kt" || ext == "java")
                        .unwrap_or(false)
                })
            {
                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                    for caps in class_re.captures_iter(&content) {
                        if let Some(name) = caps.get(1) {
                            symbols.push(name.as_str().to_string());
                        }
                    }
                }
                if symbols.len() >= 100 {
                    break;
                }
            }
        }
    }

    Ok(symbols)
}

/// Check if a symbol is used in the module directory
fn is_symbol_used_in_module(_root: &Path, module_dir: &Path, symbol: &str) -> Result<bool> {
    if !module_dir.exists() {
        return Ok(false);
    }

    let mut found = false;

    for entry in walkdir::WalkDir::new(module_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path().extension()
                .map(|ext| ext == "kt" || ext == "java")
                .unwrap_or(false)
        })
    {
        if found { break; }

        let path = entry.path();
        if let Ok(content) = std::fs::read_to_string(path) {
            // Skip if file is in the same module as the symbol definition
            // (we want usages, not definitions)
            if content.contains(&format!("class {}", symbol))
                || content.contains(&format!("interface {}", symbol))
                || content.contains(&format!("object {}", symbol)) {
                continue;
            }

            // Check for import or direct usage
            if content.contains(symbol) {
                found = true;
            }
        }
    }

    Ok(found)
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

// === New v3.2.0 Commands ===

fn cmd_xml_usages(root: &Path, class_name: &str, module_filter: Option<&str>) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!("{}", "Index not found. Run 'ast-index rebuild' first.".red());
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // Check if XML usages are indexed
    let xml_count: i64 = conn.query_row("SELECT COUNT(*) FROM xml_usages", [], |row| row.get(0))?;
    if xml_count == 0 {
        println!("{}", "XML usages not indexed. Run 'ast-index rebuild' first.".yellow());
        return Ok(());
    }

    // Search for class in XML usages
    let pattern = format!("%{}%", class_name);

    let results: Vec<(String, String, i64, String, Option<String>)> = if let Some(module) = module_filter {
        let mut stmt = conn.prepare(
            "SELECT m.name, x.file_path, x.line, x.class_name, x.element_id
             FROM xml_usages x
             JOIN modules m ON x.module_id = m.id
             WHERE x.class_name LIKE ?1 AND m.name = ?2
             ORDER BY m.name, x.file_path, x.line"
        )?;
        let rows = stmt.query_map(params![pattern, module], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
        })?;
        rows.filter_map(|r| r.ok()).collect()
    } else {
        let mut stmt = conn.prepare(
            "SELECT m.name, x.file_path, x.line, x.class_name, x.element_id
             FROM xml_usages x
             JOIN modules m ON x.module_id = m.id
             WHERE x.class_name LIKE ?1
             ORDER BY m.name, x.file_path, x.line
             LIMIT 100"
        )?;
        let rows = stmt.query_map(params![pattern], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
        })?;
        rows.filter_map(|r| r.ok()).collect()
    };

    println!("{}", format!("XML usages of '{}' ({}):", class_name, results.len()).bold());

    // Group by module
    let mut by_module: HashMap<String, Vec<(String, i64, String, Option<String>)>> = HashMap::new();
    for (module, file, line, class, element_id) in results {
        by_module.entry(module).or_default().push((file, line, class, element_id));
    }

    for (module, usages) in &by_module {
        println!("\n{}:", module.cyan());
        for (file, line, class, element_id) in usages {
            let id_str = element_id.as_ref()
                .map(|id| format!(" ({})", id))
                .unwrap_or_default();
            println!("  {}:{}", file, line);
            println!("    <{} ...{} />", class, id_str);
        }
    }

    if by_module.is_empty() {
        println!("  No XML usages found.");
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_resource_usages(
    root: &Path,
    resource: &str,
    module_filter: Option<&str>,
    type_filter: Option<&str>,
    show_unused: bool,
) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!("{}", "Index not found. Run 'ast-index rebuild' first.".red());
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // Check if resources are indexed
    let res_count: i64 = conn.query_row("SELECT COUNT(*) FROM resources", [], |row| row.get(0))?;
    if res_count == 0 {
        println!("{}", "Resources not indexed. Run 'ast-index rebuild' first.".yellow());
        return Ok(());
    }

    if show_unused {
        // Show unused resources in the module
        let module = module_filter.unwrap_or("");
        if module.is_empty() {
            println!("{}", "Please specify --module to find unused resources.".yellow());
            return Ok(());
        }
    } else if resource.is_empty() {
        println!("{}", "Please specify a resource name (e.g., @drawable/ic_payment or use --unused).".yellow());
        return Ok(());
    }

    if show_unused {
        let module = module_filter.unwrap_or("");
        println!("{}", format!("Unused resources in '{}':", module).bold());

        // Find resources defined in module that have no usages
        let mut stmt = conn.prepare(
            "SELECT r.type, r.name, r.file_path
             FROM resources r
             JOIN modules m ON r.module_id = m.id
             LEFT JOIN resource_usages ru ON r.id = ru.resource_id
             WHERE m.name = ?1 AND ru.id IS NULL
             ORDER BY r.type, r.name"
        )?;

        let unused: Vec<(String, String, String)> = stmt
            .query_map(params![module], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .filter_map(|r| r.ok())
            .collect();

        // Group by type
        let mut by_type: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (rtype, name, path) in unused {
            if type_filter.map(|t| t == rtype).unwrap_or(true) {
                by_type.entry(rtype).or_default().push((name, path));
            }
        }

        let mut total = 0;
        for (rtype, items) in &by_type {
            println!("\n{} ({}):", rtype.cyan(), items.len());
            for (name, path) in items.iter().take(10) {
                println!("  {} @{}/{}", "⚠".yellow(), rtype, name);
                println!("    defined in: {}", path);
            }
            if items.len() > 10 {
                println!("  ... and {} more", items.len() - 10);
            }
            total += items.len();
        }

        println!("\n{}", format!("Total unused: {} resources", total).bold());

    } else {
        // Parse resource reference (e.g., @drawable/ic_payment or R.string.app_name)
        let (res_type, res_name) = parse_resource_reference(resource);

        let res_type = type_filter.unwrap_or(&res_type);

        println!("{}", format!("Usages of '@{}/{}':", res_type, res_name).bold());

        // Find resource usages
        let results: Vec<(String, i64, String)> = if let Some(module) = module_filter {
            let mut stmt = conn.prepare(
                "SELECT ru.usage_file, ru.usage_line, ru.usage_type
                 FROM resource_usages ru
                 JOIN resources r ON ru.resource_id = r.id
                 WHERE r.type = ?1 AND r.name = ?2 AND ru.usage_file LIKE ?3
                 ORDER BY ru.usage_file, ru.usage_line
                 LIMIT 100"
            )?;
            let module_pattern = format!("%{}%", module);
            let rows = stmt.query_map(params![res_type, res_name, module_pattern], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
            rows.filter_map(|r| r.ok()).collect()
        } else {
            let mut stmt = conn.prepare(
                "SELECT ru.usage_file, ru.usage_line, ru.usage_type
                 FROM resource_usages ru
                 JOIN resources r ON ru.resource_id = r.id
                 WHERE r.type = ?1 AND r.name = ?2
                 ORDER BY ru.usage_file, ru.usage_line
                 LIMIT 100"
            )?;
            let rows = stmt.query_map(params![res_type, res_name], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
            rows.filter_map(|r| r.ok()).collect()
        };

        // Group by usage type
        let code_usages: Vec<_> = results.iter().filter(|(_, _, t)| t == "code").collect();
        let xml_usages: Vec<_> = results.iter().filter(|(_, _, t)| t == "xml").collect();

        if !code_usages.is_empty() {
            println!("\n{} ({}):", "Kotlin/Java".cyan(), code_usages.len());
            for (file, line, _) in code_usages.iter().take(10) {
                println!("  {}:{}", file, line);
            }
            if code_usages.len() > 10 {
                println!("  ... and {} more", code_usages.len() - 10);
            }
        }

        if !xml_usages.is_empty() {
            println!("\n{} ({}):", "XML".cyan(), xml_usages.len());
            for (file, line, _) in xml_usages.iter().take(10) {
                println!("  {}:{}", file, line);
            }
            if xml_usages.len() > 10 {
                println!("  ... and {} more", xml_usages.len() - 10);
            }
        }

        if results.is_empty() {
            println!("  No usages found.");
        } else {
            println!("\n{}", format!("Total: {} usages", results.len()).bold());
        }
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Parse resource reference like @drawable/ic_name or R.string.name
fn parse_resource_reference(resource: &str) -> (String, String) {
    // Format: @type/name
    if resource.starts_with('@') {
        let parts: Vec<&str> = resource[1..].splitn(2, '/').collect();
        if parts.len() == 2 {
            return (parts[0].to_string(), parts[1].to_string());
        }
    }

    // Format: R.type.name
    if resource.starts_with("R.") {
        let parts: Vec<&str> = resource[2..].splitn(2, '.').collect();
        if parts.len() == 2 {
            return (parts[0].to_string(), parts[1].to_string());
        }
    }

    // Assume it's just a name, try to guess type from prefix
    let resource = resource.trim_start_matches('@');
    if resource.starts_with("ic_") || resource.starts_with("img_") {
        return ("drawable".to_string(), resource.to_string());
    }
    if resource.starts_with("color_") {
        return ("color".to_string(), resource.to_string());
    }

    // Default: assume it's a string resource
    ("string".to_string(), resource.to_string())
}

// === iOS Commands ===

fn cmd_storyboard_usages(root: &Path, class_name: &str, module: Option<&str>) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;

    let query = if let Some(m) = module {
        format!(
            r#"
            SELECT su.file_path, su.line, su.class_name, su.usage_type, su.storyboard_id
            FROM storyboard_usages su
            LEFT JOIN modules mod ON su.module_id = mod.id
            WHERE su.class_name LIKE '%{}%'
            AND (mod.name LIKE '%{}%' OR mod.path LIKE '%{}%')
            ORDER BY su.file_path, su.line
            "#,
            class_name, m, m
        )
    } else {
        format!(
            r#"
            SELECT file_path, line, class_name, usage_type, storyboard_id
            FROM storyboard_usages
            WHERE class_name LIKE '%{}%'
            ORDER BY file_path, line
            "#,
            class_name
        )
    };

    let mut stmt = conn.prepare(&query)?;
    let results: Vec<(String, i64, String, Option<String>, Option<String>)> = stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    if results.is_empty() {
        println!("{}", format!("No storyboard usages found for '{}'", class_name).yellow());
    } else {
        println!(
            "{}",
            format!("Storyboard usages for '{}' ({}):", class_name, results.len()).bold()
        );
        for (path, line, cls, usage_type, sb_id) in &results {
            let type_str = usage_type.as_deref().unwrap_or("unknown");
            let id_str = sb_id.as_deref().map(|s| format!(" (id: {})", s)).unwrap_or_default();
            println!("  {}:{} {} [{}]{}", path.cyan(), line, cls, type_str, id_str);
        }
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_asset_usages(root: &Path, asset: &str, module: Option<&str>, asset_type: Option<&str>, unused: bool) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;

    if unused {
        // Find unused assets
        if module.is_none() {
            println!("{}", "Error: --unused requires --module".red());
            return Ok(());
        }

        let m = module.unwrap();
        let type_filter = asset_type.map(|t| format!("AND a.type = '{}'", t)).unwrap_or_default();

        let query = format!(
            r#"
            SELECT a.name, a.type, a.file_path
            FROM ios_assets a
            LEFT JOIN modules mod ON a.module_id = mod.id
            LEFT JOIN ios_asset_usages au ON a.id = au.asset_id
            WHERE (mod.name LIKE '%{}%' OR mod.path LIKE '%{}%')
            AND au.id IS NULL
            {}
            ORDER BY a.type, a.name
            "#,
            m, m, type_filter
        );

        let mut stmt = conn.prepare(&query)?;
        let results: Vec<(String, String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        if results.is_empty() {
            println!("{}", format!("No unused assets found in module '{}'", m).green());
        } else {
            println!(
                "{}",
                format!("Unused assets in '{}' ({}):", m, results.len()).bold()
            );
            for (name, atype, path) in &results {
                println!("  {} [{}]: {}", name.cyan(), atype, path.dimmed());
            }
        }
    } else if asset.is_empty() {
        // List all assets
        let type_filter = asset_type.map(|t| format!("WHERE type = '{}'", t)).unwrap_or_default();
        let query = format!(
            "SELECT name, type, file_path FROM ios_assets {} ORDER BY type, name LIMIT 100",
            type_filter
        );

        let mut stmt = conn.prepare(&query)?;
        let results: Vec<(String, String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        println!("{}", format!("iOS assets ({}):", results.len()).bold());
        for (name, atype, path) in &results {
            println!("  {} [{}]: {}", name.cyan(), atype, path.dimmed());
        }
    } else {
        // Find usages of specific asset
        let query = format!(
            r#"
            SELECT a.name, a.type, au.usage_file, au.usage_line
            FROM ios_assets a
            JOIN ios_asset_usages au ON a.id = au.asset_id
            WHERE a.name LIKE '%{}%'
            ORDER BY au.usage_file, au.usage_line
            "#,
            asset
        );

        let mut stmt = conn.prepare(&query)?;
        let results: Vec<(String, String, String, i64)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        if results.is_empty() {
            println!("{}", format!("No usages found for asset '{}'", asset).yellow());
        } else {
            println!(
                "{}",
                format!("Usages of '{}' ({}):", asset, results.len()).bold()
            );
            for (name, atype, file, line) in &results {
                println!("  {} [{}]: {}:{}", name.cyan(), atype, file, line);
            }
        }
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_swiftui(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();

    // Search for SwiftUI state properties: @State, @Binding, @Published, @ObservedObject, @StateObject, @EnvironmentObject
    let pattern = r"@(State|Binding|Published|ObservedObject|StateObject|EnvironmentObject)\s+(private\s+)?(var|let)\s+\w+";

    let prop_regex = Regex::new(r"@(State|Binding|Published|ObservedObject|StateObject|EnvironmentObject)\s+(?:private\s+)?(?:var|let)\s+(\w+)")?;

    let mut results: Vec<(String, String, String, usize)> = vec![];

    search_files(root, pattern, &["swift"], |path, line_num, line| {
        if results.len() >= limit {
            return;
        }

        if let Some(caps) = prop_regex.captures(&line) {
            let prop_type = caps.get(1).unwrap().as_str().to_string();
            let prop_name = caps.get(2).unwrap().as_str().to_string();

            if let Some(q) = query {
                let q_lower = q.to_lowercase();
                if !prop_name.to_lowercase().contains(&q_lower)
                    && !prop_type.to_lowercase().contains(&q_lower)
                {
                    return;
                }
            }

            let rel_path = relative_path(root, path);
            results.push((prop_type, prop_name, rel_path, line_num));
        }
    })?;

    println!(
        "{}",
        format!("SwiftUI state properties ({}):", results.len()).bold()
    );

    // Group by type
    let mut by_type: HashMap<String, Vec<(String, String, usize)>> = HashMap::new();
    for (prop_type, prop_name, path, line) in results {
        by_type
            .entry(prop_type)
            .or_default()
            .push((prop_name, path, line));
    }

    for (prop_type, props) in &by_type {
        println!("\n  {} ({}):", format!("@{}", prop_type).cyan(), props.len());
        for (name, path, line) in props.iter().take(10) {
            println!("    {}: {}:{}", name, path, line);
        }
        if props.len() > 10 {
            println!("    ... and {} more", props.len() - 10);
        }
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_async_funcs(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();

    // Search for async functions in Swift
    let pattern = r"func\s+\w+[^{]*\basync\b";

    let func_regex = Regex::new(r"func\s+(\w+)\s*(?:<[^>]*>)?\s*\([^)]*\)[^{]*\basync\b")?;

    let mut results: Vec<(String, String, usize)> = vec![];

    search_files(root, pattern, &["swift"], |path, line_num, line| {
        if results.len() >= limit {
            return;
        }

        if let Some(caps) = func_regex.captures(&line) {
            let func_name = caps.get(1).unwrap().as_str().to_string();

            if let Some(q) = query {
                if !func_name.to_lowercase().contains(&q.to_lowercase()) {
                    return;
                }
            }

            let rel_path = relative_path(root, path);
            results.push((func_name, rel_path, line_num));
        }
    })?;

    println!(
        "{}",
        format!("Async functions ({}):", results.len()).bold()
    );

    for (func_name, path, line_num) in &results {
        println!("  {}: {}:{}", func_name.cyan(), path, line_num);
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_publishers(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();

    // Search for Combine publishers: PassthroughSubject, CurrentValueSubject, AnyPublisher, Published
    let pattern = r"(PassthroughSubject|CurrentValueSubject|AnyPublisher|@Published)\s*[<(]";

    let pub_regex = Regex::new(r"(PassthroughSubject|CurrentValueSubject|AnyPublisher)(?:\s*<[^>]+>)?\s*(?:\(\)|[,;=])|@Published\s+(?:private\s+)?var\s+(\w+)")?;

    let mut results: Vec<(String, String, String, usize)> = vec![];

    search_files(root, pattern, &["swift"], |path, line_num, line| {
        if results.len() >= limit {
            return;
        }

        if let Some(caps) = pub_regex.captures(&line) {
            let pub_type = caps.get(1).map(|m| m.as_str()).unwrap_or("@Published");
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("");

            if let Some(q) = query {
                let q_lower = q.to_lowercase();
                if !pub_type.to_lowercase().contains(&q_lower)
                    && !name.to_lowercase().contains(&q_lower)
                    && !line.to_lowercase().contains(&q_lower)
                {
                    return;
                }
            }

            let rel_path = relative_path(root, path);
            let content: String = line.trim().chars().take(80).collect();
            results.push((pub_type.to_string(), content, rel_path, line_num));
        }
    })?;

    println!(
        "{}",
        format!("Combine publishers ({}):", results.len()).bold()
    );

    for (pub_type, content, path, line_num) in &results {
        println!("  {} {}:{}", pub_type.cyan(), path, line_num);
        println!("    {}", content.dimmed());
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

fn cmd_main_actor(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();

    // Search for @MainActor
    let pattern = r"@MainActor";

    let mut results: Vec<(String, usize, String)> = vec![];

    search_files(root, pattern, &["swift"], |path, line_num, line| {
        if results.len() >= limit {
            return;
        }

        if let Some(q) = query {
            if !line.to_lowercase().contains(&q.to_lowercase()) {
                return;
            }
        }

        let rel_path = relative_path(root, path);
        let content: String = line.trim().chars().take(100).collect();
        results.push((rel_path, line_num, content));
    })?;

    println!(
        "{}",
        format!("@MainActor usages ({}):", results.len()).bold()
    );

    for (path, line_num, content) in &results {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}
