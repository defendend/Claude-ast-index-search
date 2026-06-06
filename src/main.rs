use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use ast_index::{commands, db};

#[derive(Parser)]
#[command(name = "ast-index")]
#[command(about = "Fast code search for multi-language projects")]
#[command(version)]
#[command(help_template = "\
{before-help}{name} v{version}
{about}

{usage-heading} {usage}

Index Management:
  rebuild                Rebuild index (full reindex)
  update                 Update index (incremental)
  stats                  Show index statistics
  clear                  Clear index database
  version                Show version
  watch                  Watch for file changes and auto-update

Search & Navigation:
  search                 Universal search (files + symbols)
  file                   Find files by name
  symbol                 Find symbols (classes, interfaces, functions)
  class                  Find class or interface
  hierarchy              Show class hierarchy
  implementations        Find implementations (subclasses/implementors)
  refs                   Cross-references: definitions, imports, usages
  usages                 Find usages of a symbol
  outline                Show symbols in a file
  imports                Show imports in a file
  changed                Show changed symbols (git/arc diff)

Module Commands:
  module                 Find modules
  deps                   Show module dependencies
  dependents             Show reverse dependencies
  module-route           Show dependency path(s) between two modules
  unused-deps            Find unused dependencies in a module
  api                    Show public API of a module
  unused-symbols         Find potentially unused symbols

Code Patterns (grep-based):
  todo                   Find TODO/FIXME/HACK comments
  callers                Find callers of a function
  call-tree              Show call hierarchy tree
  annotations            Find classes with annotation
  deprecated             Find @Deprecated items
  suppress               Find @Suppress annotations
  provides               Find @Provides/@Binds (Dagger)
  inject                 Find @Inject points
  composables            Find @Composable functions
  suspend                Find suspend functions
  flows                  Find Flow/StateFlow/SharedFlow
  extensions             Find extension functions
  deeplinks              Find deeplinks
  previews               Find @Preview functions

Android:
  xml-usages             Find class usages in XML layouts
  resource-usages        Find resource usages

iOS (Swift/ObjC):
  storyboard-usages      Find class usages in storyboards/xibs
  asset-usages           Find iOS asset usages (xcassets)
  swiftui                Find SwiftUI views and state properties
  async-funcs            Find async functions (Swift)
  publishers             Find Combine publishers
  main-actor             Find @MainActor annotations

Perl:
  perl-exports           Find exported functions (@EXPORT)
  perl-subs              Find subroutines
  perl-pod               Find POD documentation
  perl-tests             Find test assertions
  perl-imports           Find use/require statements

Project Insights:
  map                    Show compact project map (key types per directory)
  conventions            Detect project conventions (architecture, frameworks, naming)

Project Configuration:
  add-root               Add additional source root
  remove-root            Remove source root
  list-roots             List configured source roots
  install-claude-plugin  Install Claude Code plugin
  install-codex-mcp      Register ast-index MCP server in Codex

Programmatic Access:
  agrep                  Structural code search via ast-grep
  query                  Execute raw SQL against the index DB
  db-path                Print path to the SQLite index database
  schema                 Show database schema (tables and columns)

Options:
{options}{after-help}\
")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Output format: text, json (all commands); mermaid, dot (module-route only)
    #[arg(long, global = true, default_value = "text")]
    format: String,

    /// Prefer any existing parent-directory index over nested project/VCS
    /// markers. Useful in monorepos where subdirectories carry their own
    /// markers but share a root-level index. Can also be enabled via
    /// AST_INDEX_WALK_UP=1.
    #[arg(long, global = true)]
    walk_up: bool,
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
    /// Show call hierarchy (callers tree up) for a function
    CallTree {
        /// Function name
        function_name: String,
        /// Max depth of the tree
        #[arg(short, long, default_value = "3")]
        depth: usize,
        /// Max callers per level
        #[arg(short, long, default_value = "10")]
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
    /// Rebuild index (full reindex)
    Rebuild {
        /// Index type: files, symbols, modules, or all
        #[arg(long, default_value = "all")]
        r#type: String,
        /// Skip module dependencies indexing
        #[arg(long)]
        no_deps: bool,
        /// Include gitignored files (e.g., build/ directories)
        #[arg(long)]
        no_ignore: bool,
        /// Index each sub-project separately (for large monorepo directories)
        #[arg(long)]
        sub_projects: bool,
        /// Verbose logging with timing for each step
        #[arg(long, short)]
        verbose: bool,
        /// Compatibility flag; fast rebuild heuristics are now enabled by default
        #[arg(long)]
        experimental_fast_rebuild: bool,
        /// Number of parallel threads (default: CPU cores, max 8; increase for network filesystems)
        #[arg(long, short = 'j')]
        threads: Option<usize>,
        /// Only index these directories (allow-list, can be repeated).
        /// Overrides config `include`. Example: --include smart_devices --include lib
        #[arg(long = "include")]
        include: Vec<String>,
        /// Exclude directories matching gitignore-style patterns (can be repeated).
        /// Merged with config `exclude`. Example: --exclude vendor --exclude "proto/gen"
        #[arg(long = "exclude")]
        exclude: Vec<String>,
        /// Additional paths to index (can be specified multiple times).
        #[arg(long = "path")]
        paths: Vec<String>,
        /// Bypass the candidate-file cap (AST_INDEX_MAX_FILES) for this run.
        /// Use when you really want to index a monorepo / VCS root that
        /// would otherwise be aborted with an oversize warning.
        #[arg(long)]
        force: bool,
        /// Persist the cap bypass for this project, so future
        /// `ast-index rebuild` runs on the same root no longer abort.
        /// Requires `--force` to take effect.
        #[arg(long)]
        remember: bool,
        /// Override the candidate-file cap for this run only.
        /// Default 500_000; set to 0 to disable. See AST_INDEX_MAX_FILES.
        #[arg(long = "max-files")]
        max_files: Option<usize>,
    },
    /// Update index (incremental)
    Update {
        /// Verbose logging with timing
        #[arg(long, short)]
        verbose: bool,
    },
    /// Restore index from a .db file
    Restore {
        /// Path to the .db file to restore
        path: String,
    },
    /// Show index statistics
    Stats,
    /// Universal search (files + symbols)
    Search {
        /// Search query
        query: String,
        /// Filter symbols by type: class, interface, function, property
        #[arg(long, short = 't')]
        r#type: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Filter by file path
        #[arg(long)]
        in_file: Option<String>,
        /// Filter by module path
        #[arg(long)]
        module: Option<String>,
        /// Fuzzy search (exact → prefix → contains)
        #[arg(long)]
        fuzzy: bool,
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
        /// Symbol name (exact match; omit when using --pattern)
        name: Option<String>,
        /// Glob pattern for symbol name (e.g. "*Mailer", "*Email*Service*")
        #[arg(long, short = 'p')]
        pattern: Option<String>,
        /// Symbol type: class, interface, function, property
        #[arg(long, short = 't')]
        r#type: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Filter by file path
        #[arg(long)]
        in_file: Option<String>,
        /// Filter by module path
        #[arg(long)]
        module: Option<String>,
        /// Fuzzy search (exact → prefix → contains)
        #[arg(long)]
        fuzzy: bool,
    },
    /// Find class or interface
    Class {
        /// Class name (exact match; omit when using --pattern)
        name: Option<String>,
        /// Glob pattern for class name (e.g. "*Mailer", "*Email*Service*")
        #[arg(long, short = 'p')]
        pattern: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Filter by file path
        #[arg(long)]
        in_file: Option<String>,
        /// Filter by module path
        #[arg(long)]
        module: Option<String>,
        /// Fuzzy search (exact → prefix → contains)
        #[arg(long)]
        fuzzy: bool,
    },
    /// Find implementations (subclasses/implementors)
    Implementations {
        /// Parent class/interface name
        parent: String,
        /// Max results
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Filter by file path
        #[arg(long)]
        in_file: Option<String>,
        /// Filter by module path
        #[arg(long)]
        module: Option<String>,
    },
    /// Show class hierarchy
    Hierarchy {
        /// Class name
        name: String,
        /// Filter children by file path
        #[arg(long)]
        in_file: Option<String>,
        /// Filter children by module path
        #[arg(long)]
        module: Option<String>,
        /// Maximum number of children to display
        #[arg(long, default_value = "200")]
        limit: usize,
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
    /// Show transitive dependency path(s) between two modules
    ModuleRoute {
        /// Source module (the one whose dependencies are followed)
        #[arg(long)]
        from: String,
        /// Target module to reach
        #[arg(long)]
        to: String,
        /// Show all simple paths (default: shortest only)
        #[arg(long)]
        all: bool,
        /// Cap on number of paths returned
        #[arg(long, default_value = "50")]
        max_paths: usize,
        /// Cap on path length in hops
        #[arg(long, default_value = "20")]
        max_depth: usize,
        /// Wall-clock guard for path search (milliseconds)
        #[arg(long, default_value = "5000")]
        timeout_ms: u64,
        /// Restrict traversal to a given dep kind ("api", "implementation", "all")
        #[arg(long, default_value = "all")]
        via_kind: String,
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
    /// Show cross-references: definitions, imports, usages
    Refs {
        /// Symbol name
        symbol: String,
        /// Max results per section
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Find usages of a symbol
    Usages {
        /// Symbol name
        symbol: String,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
        /// Filter by file path
        #[arg(long)]
        in_file: Option<String>,
        /// Filter by module path
        #[arg(long)]
        module: Option<String>,
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
    /// Show changed symbols (git/arc diff)
    Changed {
        /// Base branch (auto-detected: trunk for arc, origin/main for git)
        #[arg(long)]
        base: Option<String>,
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
    // === Perl Commands ===
    /// Find Perl exported functions (@EXPORT, @EXPORT_OK)
    PerlExports {
        /// Filter by module/function name
        query: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Find Perl subroutines
    PerlSubs {
        /// Filter by name
        query: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Find POD documentation sections
    PerlPod {
        /// Filter by heading text
        query: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Find Perl test assertions (Test::More, Test::Simple)
    PerlTests {
        /// Filter by test name or pattern
        query: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Find Perl use/require statements
    PerlImports {
        /// Filter by module name
        query: Option<String>,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    // === Project Insights ===
    /// Show compact project map (key types per directory)
    Map {
        /// Filter by module (enables detailed mode with symbols)
        #[arg(short, long)]
        module: Option<String>,
        /// Max symbols per directory group (detailed mode)
        #[arg(long, default_value = "5")]
        per_dir: usize,
        /// Max directory groups to show
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Detect project conventions (architecture, frameworks, naming)
    Conventions,
    /// Find potentially unused symbols
    UnusedSymbols {
        /// Filter by module path
        #[arg(long)]
        module: Option<String>,
        /// Only check exported (capitalized) symbols
        #[arg(long)]
        export_only: bool,
        /// Max results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Add additional source root to project
    AddRoot {
        /// Path to add as source root
        path: String,
        /// Force add even if path overlaps with project root
        #[arg(long)]
        force: bool,
    },
    /// Remove source root from project
    RemoveRoot {
        /// Path to remove
        path: String,
    },
    /// List configured source roots
    ListRoots,
    /// Watch for file changes and auto-update index
    Watch,
    /// Clear index database for current project
    Clear,
    /// Show version
    Version,
    /// Install Claude Code plugin to ~/.claude/plugins/
    InstallClaudePlugin,
    /// Register ast-index MCP server in Codex
    InstallCodexMcp {
        /// Print the Codex command and config fallback without changing Codex config
        #[arg(long)]
        dry_run: bool,
    },
    /// Detect every stack present in the current project root by marker files
    DetectStacks,
    /// Install git hooks that auto-update the index on checkout / merge / rewrite
    InstallGitHooks {
        /// Overwrite existing ast-index hook scripts
        #[arg(long)]
        force: bool,
        /// Print the hook script paths and contents without writing anything
        #[arg(long)]
        dry_run: bool,
    },
    // === Programmatic Access ===
    /// Structural code search via ast-grep (requires `sg` installed)
    Agrep {
        /// AST pattern to match (e.g., "router.launch($$$)")
        pattern: String,
        /// Language filter (kotlin, java, typescript, swift, python, rust, go, etc.)
        #[arg(short, long)]
        lang: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Execute raw SQL query against the index database (SELECT only)
    Query {
        /// SQL query (SELECT statements only)
        sql: String,
        /// Max rows to return
        #[arg(short, long, default_value = "100")]
        limit: usize,
    },
    /// Print path to the SQLite index database
    DbPath,
    /// Show database schema (tables and columns)
    Schema,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // Walk-up is opt-in via CLI flag OR AST_INDEX_WALK_UP env var. CLI wins.
    let walk_up = cli.walk_up
        || std::env::var("AST_INDEX_WALK_UP")
            .map(|v| {
                let t = v.trim();
                !t.is_empty() && t != "0" && !t.eq_ignore_ascii_case("false")
            })
            .unwrap_or(false);
    let root = match &cli.command {
        Commands::Rebuild { .. } | Commands::Clear => find_project_root_for_write()?,
        _ => find_project_root_for_read(walk_up)?,
    };
    let format = cli.format.as_str();

    // Migrate project DB from old kotlin-index to ast-index
    db::migrate_legacy_project(&root);

    // Compute directory scope: if cwd is inside project root, limit search to cwd subtree
    let cwd = std::env::current_dir().unwrap_or_default();
    let dir_prefix = if cwd != root {
        cwd.strip_prefix(&root).ok().map(|rel| {
            let mut s = rel.to_string_lossy().to_string();
            if !s.ends_with('/') {
                s.push('/');
            }
            s
        })
    } else {
        None
    };
    let dir_prefix_ref = dir_prefix.as_deref();

    match cli.command {
        // Grep commands
        Commands::Todo { pattern, limit } => commands::grep::cmd_todo(&root, &pattern, limit),
        Commands::Callers {
            function_name,
            limit,
        } => commands::grep::cmd_callers(&root, &function_name, limit),
        Commands::CallTree {
            function_name,
            depth,
            limit,
        } => commands::grep::cmd_call_tree(&root, &function_name, depth, limit),
        Commands::Provides { type_name, limit } => {
            commands::grep::cmd_provides(&root, &type_name, limit)
        }
        Commands::Suspend { query, limit } => {
            commands::grep::cmd_suspend(&root, query.as_deref(), limit)
        }
        Commands::Composables { query, limit } => {
            commands::grep::cmd_composables(&root, query.as_deref(), limit)
        }
        Commands::Deprecated { query, limit } => {
            commands::grep::cmd_deprecated(&root, query.as_deref(), limit)
        }
        Commands::Suppress { query, limit } => {
            commands::grep::cmd_suppress(&root, query.as_deref(), limit)
        }
        Commands::Inject { type_name, limit } => {
            commands::grep::cmd_inject(&root, &type_name, limit)
        }
        Commands::Annotations { annotation, limit } => {
            commands::grep::cmd_annotations(&root, &annotation, limit)
        }
        Commands::Deeplinks { query, limit } => {
            commands::grep::cmd_deeplinks(&root, query.as_deref(), limit)
        }
        Commands::Extensions {
            receiver_type,
            limit,
        } => commands::grep::cmd_extensions(&root, &receiver_type, limit),
        Commands::Flows { query, limit } => {
            commands::grep::cmd_flows(&root, query.as_deref(), limit)
        }
        Commands::Previews { query, limit } => {
            commands::grep::cmd_previews(&root, query.as_deref(), limit)
        }
        // Management commands
        Commands::Rebuild {
            r#type,
            no_deps,
            no_ignore,
            sub_projects,
            verbose,
            experimental_fast_rebuild,
            threads,
            include,
            exclude,
            paths,
            force,
            remember,
            max_files,
        } => {
            if let Some(t) = threads {
                std::env::set_var("AST_INDEX_THREADS", t.to_string());
            }
            // Convert per-run flags → env vars so they reach the walker
            // without threading them through six function signatures.
            if force {
                std::env::set_var("AST_INDEX_MAX_FILES", "0");
            } else if let Some(n) = max_files {
                std::env::set_var("AST_INDEX_MAX_FILES", n.to_string());
            }
            if remember && force {
                std::env::set_var("AST_INDEX_REMEMBER_BYPASS", "1");
            } else if remember && !force {
                eprintln!(
                    "[ast-index] --remember has no effect without --force; ignoring"
                );
            }
            commands::management::cmd_rebuild(
                &root,
                &r#type,
                !no_deps,
                no_ignore,
                sub_projects,
                verbose,
                experimental_fast_rebuild,
                &include,
                &exclude,
                &paths,
            )
        }
        Commands::Update { verbose } => commands::management::cmd_update(&root, verbose),
        Commands::Restore { path } => commands::management::cmd_restore(&root, &path),
        Commands::Stats => commands::management::cmd_stats(&root, format),
        // Index commands
        Commands::Search {
            query,
            r#type,
            limit,
            in_file,
            module,
            fuzzy,
        } => {
            let scope = db::SearchScope {
                in_file: in_file.as_deref(),
                module: module.as_deref(),
                dir_prefix: dir_prefix_ref,
            };
            commands::index::cmd_search(
                &root,
                &query,
                r#type.as_deref(),
                limit,
                format,
                &scope,
                fuzzy,
            )
        }
        Commands::Symbol {
            name,
            pattern,
            r#type,
            limit,
            in_file,
            module,
            fuzzy,
        } => {
            let scope = db::SearchScope {
                in_file: in_file.as_deref(),
                module: module.as_deref(),
                dir_prefix: dir_prefix_ref,
            };
            commands::index::cmd_symbol(
                &root,
                name.as_deref(),
                pattern.as_deref(),
                r#type.as_deref(),
                limit,
                format,
                &scope,
                fuzzy,
            )
        }
        Commands::Class {
            name,
            pattern,
            limit,
            in_file,
            module,
            fuzzy,
        } => {
            let scope = db::SearchScope {
                in_file: in_file.as_deref(),
                module: module.as_deref(),
                dir_prefix: dir_prefix_ref,
            };
            commands::index::cmd_class(
                &root,
                name.as_deref(),
                pattern.as_deref(),
                limit,
                format,
                &scope,
                fuzzy,
            )
        }
        Commands::Implementations {
            parent,
            limit,
            in_file,
            module,
        } => {
            let scope = db::SearchScope {
                in_file: in_file.as_deref(),
                module: module.as_deref(),
                dir_prefix: dir_prefix_ref,
            };
            commands::index::cmd_implementations(&root, &parent, limit, format, &scope)
        }
        Commands::Refs { symbol, limit } => {
            commands::index::cmd_refs(&root, &symbol, limit, format)
        }
        Commands::Hierarchy {
            name,
            in_file,
            module,
            limit,
        } => {
            let scope = db::SearchScope {
                in_file: in_file.as_deref(),
                module: module.as_deref(),
                dir_prefix: dir_prefix_ref,
            };
            commands::index::cmd_hierarchy(&root, &name, limit, &scope)
        }
        Commands::Usages {
            symbol,
            limit,
            in_file,
            module,
        } => {
            let scope = db::SearchScope {
                in_file: in_file.as_deref(),
                module: module.as_deref(),
                dir_prefix: dir_prefix_ref,
            };
            commands::index::cmd_usages(&root, &symbol, limit, format, &scope)
        }
        // Module commands
        Commands::Module { pattern, limit } => {
            commands::modules::cmd_module(&root, &pattern, limit)
        }
        Commands::Deps { module } => commands::modules::cmd_deps(&root, &module),
        Commands::Dependents { module } => commands::modules::cmd_dependents(&root, &module),
        Commands::ModuleRoute {
            from,
            to,
            all,
            max_paths,
            max_depth,
            timeout_ms,
            via_kind,
        } => commands::modules::cmd_module_route(
            &root, &from, &to, all, max_paths, max_depth, timeout_ms, &via_kind, format,
        ),
        Commands::UnusedDeps {
            module,
            verbose,
            no_transitive,
            no_xml,
            no_resources,
            strict,
        } => {
            let check_transitive = !no_transitive && !strict;
            let check_xml = !no_xml && !strict;
            let check_resources = !no_resources && !strict;
            commands::modules::cmd_unused_deps(
                &root,
                &module,
                verbose,
                check_transitive,
                check_xml,
                check_resources,
            )
        }
        // File commands
        Commands::File {
            pattern,
            exact,
            limit,
        } => commands::files::cmd_file(&root, &pattern, exact, limit, format),
        Commands::Outline { file } => commands::files::cmd_outline(&root, &file),
        Commands::Imports { file } => commands::files::cmd_imports(&root, &file),
        Commands::Api { module_path, limit } => {
            commands::files::cmd_api(&root, &module_path, limit)
        }
        Commands::Changed { base } => {
            let vcs = commands::files::detect_vcs(&root);
            let default_base = if vcs == "arc" {
                "trunk"
            } else {
                commands::files::detect_git_default_branch(&root)
            };
            let base = base.as_deref().unwrap_or(default_base);
            commands::files::cmd_changed(&root, base)
        }
        // Android commands
        Commands::XmlUsages { class_name, module } => {
            commands::android::cmd_xml_usages(&root, &class_name, module.as_deref())
        }
        Commands::ResourceUsages {
            resource,
            module,
            r#type,
            unused,
        } => commands::android::cmd_resource_usages(
            &root,
            &resource,
            module.as_deref(),
            r#type.as_deref(),
            unused,
        ),
        // iOS commands
        Commands::StoryboardUsages { class_name, module } => {
            commands::ios::cmd_storyboard_usages(&root, &class_name, module.as_deref())
        }
        Commands::AssetUsages {
            asset,
            module,
            r#type,
            unused,
        } => commands::ios::cmd_asset_usages(
            &root,
            &asset,
            module.as_deref(),
            r#type.as_deref(),
            unused,
        ),
        Commands::Swiftui { query, limit } => {
            commands::ios::cmd_swiftui(&root, query.as_deref(), limit)
        }
        Commands::AsyncFuncs { query, limit } => {
            commands::ios::cmd_async_funcs(&root, query.as_deref(), limit)
        }
        Commands::Publishers { query, limit } => {
            commands::ios::cmd_publishers(&root, query.as_deref(), limit)
        }
        Commands::MainActor { query, limit } => {
            commands::ios::cmd_main_actor(&root, query.as_deref(), limit)
        }
        // Perl commands
        Commands::PerlExports { query, limit } => {
            commands::perl::cmd_perl_exports(&root, query.as_deref(), limit)
        }
        Commands::PerlSubs { query, limit } => {
            commands::perl::cmd_perl_subs(&root, query.as_deref(), limit)
        }
        Commands::PerlPod { query, limit } => {
            commands::perl::cmd_perl_pod(&root, query.as_deref(), limit)
        }
        Commands::PerlTests { query, limit } => {
            commands::perl::cmd_perl_tests(&root, query.as_deref(), limit)
        }
        Commands::PerlImports { query, limit } => {
            commands::perl::cmd_perl_imports(&root, query.as_deref(), limit)
        }
        // Project insights
        Commands::Map {
            module,
            per_dir,
            limit,
        } => commands::project_info::cmd_map(&root, module.as_deref(), per_dir, limit, format),
        Commands::Conventions => commands::project_info::cmd_conventions(&root, format),
        Commands::UnusedSymbols {
            module,
            export_only,
            limit,
        } => commands::analysis::cmd_unused_symbols(
            &root,
            module.as_deref(),
            export_only,
            limit,
            format,
        ),
        Commands::AddRoot { path, force } => {
            commands::management::cmd_add_root(&root, &path, force)
        }
        Commands::RemoveRoot { path } => commands::management::cmd_remove_root(&root, &path),
        Commands::ListRoots => commands::management::cmd_list_roots(&root),
        Commands::Watch => commands::watch::cmd_watch(&root),
        Commands::Clear => commands::management::cmd_clear(&root),
        Commands::Version => {
            println!("ast-index v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Commands::InstallClaudePlugin => cmd_install_claude_plugin(),
        Commands::InstallCodexMcp { dry_run } => cmd_install_codex_mcp(&root, dry_run),
        Commands::DetectStacks => cmd_detect_stacks(&root, format),
        Commands::InstallGitHooks { force, dry_run } => {
            cmd_install_git_hooks(&root, force, dry_run)
        }
        // Programmatic access
        Commands::Agrep {
            pattern,
            lang,
            json,
        } => commands::grep::cmd_ast_grep(&root, &pattern, lang.as_deref(), json),
        Commands::Query { sql, limit } => commands::management::cmd_query(&root, &sql, limit),
        Commands::DbPath => commands::management::cmd_db_path(&root),
        Commands::Schema => commands::management::cmd_schema(&root),
    }
}

fn cmd_install_claude_plugin() -> Result<()> {
    use std::process::Command;

    println!("Adding ast-index marketplace...");
    let status = Command::new("claude")
        .args([
            "plugin",
            "marketplace",
            "add",
            "defendend/Claude-ast-index-search",
        ])
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("Marketplace added successfully.");
        }
        Ok(s) => {
            eprintln!("Warning: marketplace add exited with {}", s);
        }
        Err(e) => {
            eprintln!("Error: could not run 'claude' CLI: {}", e);
            eprintln!("Make sure Claude Code is installed: https://docs.anthropic.com/en/docs/claude-code");
            return Err(anyhow::anyhow!("claude CLI not found"));
        }
    }

    println!("Installing ast-index plugin...");
    let status = Command::new("claude")
        .args(["plugin", "install", "ast-index"])
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("Plugin installed successfully.");
            println!("\nRestart Claude Code to activate the plugin.");
        }
        Ok(s) => {
            eprintln!("Plugin install exited with {}", s);
        }
        Err(e) => {
            return Err(anyhow::anyhow!(
                "Failed to run claude plugin install: {}",
                e
            ));
        }
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexMcpInstall {
    project_root: PathBuf,
    ast_index_bin: PathBuf,
    ast_index_mcp_bin: PathBuf,
}

impl CodexMcpInstall {
    fn from_env(project_root: &Path) -> Result<Self> {
        let path_env = std::env::var_os("PATH");
        let current_exe = std::env::current_exe()
            .context("could not determine current ast-index executable path")?;
        let cwd = std::env::current_dir().context("could not determine current directory")?;
        let argv0 = std::env::args_os().next();
        let ast_index_bin = resolve_ast_index_bin_from(argv0.as_deref(), &cwd, path_env.as_ref())
            .unwrap_or_else(|| current_exe.clone());
        let ast_index_mcp_bin = resolve_ast_index_mcp_bin_from(&ast_index_bin, path_env.as_ref())
            .or_else(|err| {
            if current_exe == ast_index_bin {
                Err(err)
            } else {
                resolve_ast_index_mcp_bin_from(&current_exe, path_env.as_ref()).map_err(|_| err)
            }
        })?;
        let project_root = db::safe_canonicalize(project_root);

        Ok(Self {
            project_root,
            ast_index_bin,
            ast_index_mcp_bin,
        })
    }

    fn codex_args(&self) -> Vec<String> {
        vec![
            "mcp".to_string(),
            "add".to_string(),
            "--env".to_string(),
            format!("AST_INDEX_ROOT={}", path_string(&self.project_root)),
            "--env".to_string(),
            format!("AST_INDEX_BIN={}", path_string(&self.ast_index_bin)),
            "ast-index".to_string(),
            path_string(&self.ast_index_mcp_bin),
        ]
    }

    fn fallback_config_toml(&self) -> String {
        format!(
            "[mcp_servers.ast-index]\ncommand = {}\nenv = {{ AST_INDEX_ROOT = {}, AST_INDEX_BIN = {} }}\n",
            toml_string(&path_string(&self.ast_index_mcp_bin)),
            toml_string(&path_string(&self.project_root)),
            toml_string(&path_string(&self.ast_index_bin)),
        )
    }

    fn dry_run_output(&self) -> String {
        let mut command = vec!["codex".to_string()];
        command.extend(self.codex_args());
        format!(
            "Would run:\n  {}\n\nFallback ~/.codex/config.toml:\n{}",
            shell_join(&command),
            self.fallback_config_toml()
        )
    }
}

fn cmd_install_codex_mcp(root: &Path, dry_run: bool) -> Result<()> {
    let install = CodexMcpInstall::from_env(root)?;

    if dry_run {
        print!("{}", install.dry_run_output());
        return Ok(());
    }

    println!("Registering ast-index MCP server in Codex...");
    let args = install.codex_args();
    let status = Command::new("codex").args(&args).status();

    match status {
        Ok(s) if s.success() => {
            println!("Codex MCP server 'ast-index' registered.");
            println!("Run `ast-index rebuild` in this project before querying it from Codex.");
            Ok(())
        }
        Ok(s) => {
            eprintln!("codex mcp add exited with {s}.");
            print_codex_fallback(&install);
            Err(anyhow::anyhow!(
                "failed to register ast-index MCP server in Codex"
            ))
        }
        Err(e) => {
            eprintln!("could not run `codex`: {e}");
            print_codex_fallback(&install);
            Err(anyhow::anyhow!("codex CLI not found or not executable"))
        }
    }
}

fn print_codex_fallback(install: &CodexMcpInstall) {
    eprintln!("\nAdd this to ~/.codex/config.toml manually:");
    eprintln!("{}", install.fallback_config_toml());
}

fn cmd_detect_stacks(root: &Path, format: &str) -> Result<()> {
    let detection = ast_index::indexer::detect_stacks(root);

    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&detection)?);
        return Ok(());
    }

    if detection.stacks.is_empty() {
        println!("{}", "No known project stacks detected at this root.".yellow());
        println!(
            "Markers checked: settings.gradle*, package.json, Cargo.toml, Package.swift, \
             go.mod, pyproject.toml, Gemfile, composer.json, *.csproj, build.sbt, build.zig, \
             CMakeLists.txt, Makefile.PL, pubspec.yaml."
        );
        return Ok(());
    }

    let kind = if detection.is_kmp {
        "Kotlin Multiplatform"
    } else if detection.is_polyglot {
        "Polyglot / monorepo"
    } else {
        "Single-stack"
    };
    println!("Detected setup: {}", kind.bold());
    println!();
    for stack in &detection.stacks {
        println!("  {} ({})", stack.label.cyan(), stack.kind);
        for marker in &stack.markers {
            println!("    - {}", marker.dimmed());
        }
    }
    Ok(())
}

fn cmd_install_git_hooks(root: &Path, force: bool, dry_run: bool) -> Result<()> {
    let git_dir = resolve_git_hooks_dir(root)?;
    let events = ["post-checkout", "post-merge", "post-rewrite"];
    let script_body = git_hook_script();

    if dry_run {
        println!("Would write the following hooks under {}:", git_dir.display());
        for event in events {
            println!("\n=== {} ===", event);
            println!("{}", script_body);
        }
        return Ok(());
    }

    std::fs::create_dir_all(&git_dir)?;
    let mut wrote = 0;
    let mut skipped: Vec<String> = Vec::new();
    for event in events {
        let path = git_dir.join(event);
        if path.exists() && !force {
            if let Ok(existing) = std::fs::read_to_string(&path) {
                if existing.contains(GIT_HOOK_MARKER) {
                    skipped.push(event.to_string());
                    continue;
                }
            }
            eprintln!(
                "Hook {} already exists; rerun with --force to overwrite.",
                path.display()
            );
            continue;
        }
        std::fs::write(&path, &script_body)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms)?;
        }
        wrote += 1;
    }

    println!(
        "{}",
        format!(
            "Installed {} hook(s) into {}.",
            wrote,
            git_dir.display()
        )
        .green()
    );
    if !skipped.is_empty() {
        println!(
            "{}",
            format!(
                "Already installed (skipped): {}",
                skipped.join(", ")
            )
            .dimmed()
        );
    }
    println!("Each hook runs `ast-index update` in the background after the git operation.");
    Ok(())
}

const GIT_HOOK_MARKER: &str = "# ast-index git hook";

fn git_hook_script() -> String {
    format!(
        "#!/usr/bin/env bash\n\
         {}\n\
         # Auto-installed by `ast-index install-git-hooks`.\n\
         # Re-run that command with --force to update this script.\n\
         set -u\n\
         if ! command -v ast-index >/dev/null 2>&1; then\n\
           exit 0\n\
         fi\n\
         # Skip if a watcher is already running for this tree.\n\
         if pgrep -f \"ast-index watch\" >/dev/null 2>&1; then\n\
           exit 0\n\
         fi\n\
         (ast-index update >/dev/null 2>&1 &) >/dev/null 2>&1\n\
         exit 0\n",
        GIT_HOOK_MARKER
    )
}

fn resolve_git_hooks_dir(root: &Path) -> Result<PathBuf> {
    let git_path = root.join(".git");
    if git_path.is_dir() {
        return Ok(git_path.join("hooks"));
    }
    if git_path.is_file() {
        // Worktree / submodule: .git is a file containing `gitdir: <path>`.
        let content = std::fs::read_to_string(&git_path)
            .with_context(|| format!("could not read {}", git_path.display()))?;
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("gitdir:") {
                let raw = rest.trim();
                let candidate = Path::new(raw);
                let real = if candidate.is_absolute() {
                    candidate.to_path_buf()
                } else {
                    root.join(candidate)
                };
                return Ok(real.join("hooks"));
            }
        }
    }
    Err(anyhow::anyhow!(
        "{} is not a git repository — `.git` not found.",
        root.display()
    ))
}

fn resolve_ast_index_bin_from(
    invoked_as: Option<&OsStr>,
    cwd: &Path,
    path_env: Option<&OsString>,
) -> Option<PathBuf> {
    let invoked_as = invoked_as?;
    if invoked_as.is_empty() {
        return None;
    }

    let invoked_path = Path::new(invoked_as);
    if invoked_path.is_absolute() {
        return Some(normalize_dot_components(invoked_path));
    }
    if invoked_path.components().count() > 1 {
        return Some(normalize_dot_components(&cwd.join(invoked_path)));
    }

    let invoked_name = invoked_as.to_string_lossy();
    find_on_path(&invoked_name, path_env).or_else(|| find_on_path(ast_index_exe_name(), path_env))
}

fn normalize_dot_components(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        if matches!(component, std::path::Component::CurDir) {
            continue;
        }
        out.push(component.as_os_str());
    }
    out
}

fn resolve_ast_index_mcp_bin_from(
    current_exe: &Path,
    path_env: Option<&OsString>,
) -> Result<PathBuf> {
    let exe_name = ast_index_mcp_exe_name();

    if let Some(dir) = current_exe.parent() {
        let sibling = dir.join(exe_name);
        if sibling.is_file() {
            return Ok(sibling);
        }
    }

    if let Some(found) = find_on_path(exe_name, path_env) {
        return Ok(found);
    }

    Err(anyhow::anyhow!(
        "could not find `{}` next to `{}` or on PATH; build it with `cargo build --release -p ast-index-mcp` and copy it next to ast-index or onto PATH",
        exe_name,
        current_exe.display()
    ))
}

fn ast_index_exe_name() -> &'static str {
    if cfg!(windows) {
        "ast-index.exe"
    } else {
        "ast-index"
    }
}

fn ast_index_mcp_exe_name() -> &'static str {
    if cfg!(windows) {
        "ast-index-mcp.exe"
    } else {
        "ast-index-mcp"
    }
}

fn find_on_path(exe_name: &str, path_env: Option<&OsString>) -> Option<PathBuf> {
    let path_env = path_env?;
    for dir in std::env::split_paths(path_env) {
        let candidate = dir.join(exe_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '=' | ':'))
    {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', "'\\''"))
}

fn find_project_root_for_write() -> Result<PathBuf> {
    Ok(std::env::current_dir()?)
}

fn find_project_root_for_read(walk_up: bool) -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let home = dirs::home_dir();
    find_project_root_for_read_at_with_db(&cwd, home.as_deref(), walk_up, db::db_exists)
}

/// Test-friendly wrapper for `find_project_root_for_read` that uses the
/// real `db::db_exists` and defaults to the legacy (walk-up-off) behaviour.
#[cfg(test)]
fn find_project_root_for_read_at(cwd: &Path, home: Option<&Path>) -> Result<PathBuf> {
    // Default behaviour for pre-existing tests: walk-up disabled.
    // db_exists closure returns false so only marker-based detection runs,
    // which is what the pre-existing tests exercise.
    find_project_root_for_read_at_with_db(cwd, home, false, |_| false)
}

/// Pure helper for `find_project_root_for_read`: walks ancestors of `cwd`
/// looking for the nearest directory that either has an existing index DB
/// or carries a project/VCS marker (`.git`, `Cargo.toml`, `settings.gradle`,
/// etc.). Stops at `home` to avoid escaping into the user's home directory.
/// Returns `cwd` itself if nothing is found.
///
/// `walk_up == true` changes the precedence: a FIRST pass walks all
/// ancestors (up to `home`) looking only for `db_exists`. Only if no
/// existing DB is found does the legacy per-ancestor (db-or-marker) walk
/// run. This is opt-in for monorepo scenarios where nested `.git` /
/// `Cargo.toml` markers would otherwise short-circuit the walk before
/// reaching a pre-built root index.
///
/// `db_exists` is injected so tests can simulate DBs without touching the
/// real cache directory.
fn find_project_root_for_read_at_with_db(
    cwd: &Path,
    home: Option<&Path>,
    walk_up: bool,
    db_exists: impl Fn(&Path) -> bool,
) -> Result<PathBuf> {
    // Opt-in first pass: DB-only across ALL ancestors, winning over any
    // marker we'd otherwise stop at. Bounded by $HOME.
    if walk_up {
        for ancestor in cwd.ancestors() {
            if let Some(h) = home {
                if ancestor == h {
                    break;
                }
            }
            if db_exists(ancestor) {
                return Ok(ancestor.to_path_buf());
            }
        }
    }
    for ancestor in cwd.ancestors() {
        // Never go above $HOME — prevents indexing entire user directory
        if let Some(h) = home {
            if ancestor == h {
                break;
            }
        }
        // Check if an index DB already exists for this ancestor
        if db_exists(ancestor) {
            return Ok(ancestor.to_path_buf());
        }
        // VCS markers
        if ancestor.join(".git").exists() || ancestor.join(".arc").join("HEAD").exists() {
            return Ok(ancestor.to_path_buf());
        }
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
                if entry
                    .path()
                    .extension()
                    .map(|e| e == "xcodeproj")
                    .unwrap_or(false)
                {
                    return Ok(ancestor.to_path_buf());
                }
            }
        }
        // Dart/Flutter markers
        if ancestor.join("pubspec.yaml").exists() {
            return Ok(ancestor.to_path_buf());
        }
        // Rust markers
        if ancestor.join("Cargo.toml").exists() {
            return Ok(ancestor.to_path_buf());
        }
        // Node.js markers
        if ancestor.join("package.json").exists() {
            return Ok(ancestor.to_path_buf());
        }
        // Go markers
        if ancestor.join("go.mod").exists() {
            return Ok(ancestor.to_path_buf());
        }
        // Python markers
        if ancestor.join("pyproject.toml").exists() || ancestor.join("setup.py").exists() {
            return Ok(ancestor.to_path_buf());
        }
        // C#/.NET markers
        if let Ok(entries) = std::fs::read_dir(ancestor) {
            let has_sln = entries.flatten().any(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext == "sln")
                    .unwrap_or(false)
            });
            if has_sln {
                return Ok(ancestor.to_path_buf());
            }
        }
        // Bazel markers
        if ancestor.join("WORKSPACE").exists()
            || ancestor.join("WORKSPACE.bazel").exists()
            || ancestor.join("MODULE.bazel").exists()
        {
            return Ok(ancestor.to_path_buf());
        }
    }
    Ok(cwd.to_path_buf())
}

#[cfg(test)]
mod root_lookup_tests {
    //! Unit tests for `find_project_root_for_read_at`.
    //!
    //! These exercise the marker-precedence logic (VCS and project-type
    //! markers) without touching the real index cache directory — we never
    //! materialise a DB, so the `db::db_exists` branch is not covered here
    //! (that's exercised implicitly by `tests/update_extra_roots_tests.rs`
    //! and `tests/path_resolver_tests.rs`).
    //!
    //! These tests exist primarily to pin current behaviour before the
    //! #30 `--walk-up` opt-in flag lands, so any intentional change shows
    //! up as a red test.
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn touch(path: &std::path::Path) {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(path, b"").unwrap();
    }

    #[test]
    fn returns_cwd_when_no_markers_anywhere() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("nested/deep");
        fs::create_dir_all(&sub).unwrap();

        // home above the tmp so the walk can run freely; no markers anywhere
        let home = tmp.path().parent().unwrap();
        let got = find_project_root_for_read_at(&sub, Some(home)).unwrap();
        // Walk exits without a match → fallback is `cwd`.
        assert_eq!(got, sub);
    }

    #[test]
    fn stops_at_git_marker_in_cwd() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        touch(&repo.join(".git/HEAD"));

        let home = tmp.path().parent().unwrap();
        let got = find_project_root_for_read_at(&repo, Some(home)).unwrap();
        assert_eq!(got, repo);
    }

    #[test]
    fn walks_up_to_git_when_subdir_has_no_marker() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        let sub = repo.join("crates/ast-index-mcp/src");
        touch(&repo.join(".git/HEAD"));
        fs::create_dir_all(&sub).unwrap();

        let home = tmp.path().parent().unwrap();
        let got = find_project_root_for_read_at(&sub, Some(home)).unwrap();
        assert_eq!(got, repo, "should walk up to `repo` which has .git");
    }

    #[test]
    fn nearest_marker_wins_over_further_ancestor() {
        // This documents the current (#30-reported) behaviour: if a nested
        // subdir carries its own VCS marker, the walk stops there and does
        // NOT reach the outer parent that might already own an index.
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path().join("outer");
        let inner = outer.join("submodule");
        touch(&outer.join(".git/HEAD"));
        touch(&inner.join(".git/HEAD")); // submodule marker
        fs::create_dir_all(inner.join("src")).unwrap();

        let home = tmp.path().parent().unwrap();
        let cwd = inner.join("src");
        let got = find_project_root_for_read_at(&cwd, Some(home)).unwrap();
        assert_eq!(got, inner, "nested `.git` wins over outer `.git`");
    }

    #[test]
    fn arc_vcs_marker_detected_via_arc_head_file() {
        // Arc (Yandex VCS) detection needs `.arc/HEAD` specifically —
        // not just a `.arc/` directory (which exists on every arc mount).
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        touch(&repo.join(".arc/HEAD"));

        let home = tmp.path().parent().unwrap();
        let got = find_project_root_for_read_at(&repo, Some(home)).unwrap();
        assert_eq!(got, repo);
    }

    #[test]
    fn gradle_kts_marker_triggers_match() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("gradle-proj");
        touch(&root.join("settings.gradle.kts"));
        fs::create_dir_all(root.join("app/src")).unwrap();

        let home = tmp.path().parent().unwrap();
        let got = find_project_root_for_read_at(&root.join("app/src"), Some(home)).unwrap();
        assert_eq!(got, root);
    }

    #[test]
    fn cargo_toml_marker_triggers_match() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("cargo-proj");
        touch(&root.join("Cargo.toml"));
        fs::create_dir_all(root.join("src")).unwrap();

        let home = tmp.path().parent().unwrap();
        let got = find_project_root_for_read_at(&root.join("src"), Some(home)).unwrap();
        assert_eq!(got, root);
    }

    #[test]
    fn xcodeproj_extension_triggers_match() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("ios-proj");
        fs::create_dir_all(root.join("MyApp.xcodeproj")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();

        let home = tmp.path().parent().unwrap();
        let got = find_project_root_for_read_at(&root.join("src"), Some(home)).unwrap();
        assert_eq!(got, root);
    }

    #[test]
    fn walk_stops_at_home_boundary() {
        // A marker ABOVE $HOME must not be returned — prevents
        // accidentally treating the user's home directory (or higher)
        // as the project root.
        let tmp = TempDir::new().unwrap();
        let fake_home = tmp.path().join("home");
        let above_home = tmp.path(); // tmp is parent of fake_home
        touch(&above_home.join(".git/HEAD")); // marker ABOVE home — should NOT match

        let proj = fake_home.join("user/proj");
        fs::create_dir_all(&proj).unwrap();

        let got = find_project_root_for_read_at(&proj, Some(&fake_home)).unwrap();
        // No marker at or below $HOME → falls back to cwd
        assert_eq!(
            got, proj,
            "walk must stop at $HOME, marker above is ignored"
        );
    }

    #[test]
    fn git_wins_over_deeper_cargo_toml() {
        // Test of ancestor-order: we walk FROM cwd UPWARDS, so the FIRST
        // matching ancestor wins. If Cargo.toml is in `repo/crate/` and
        // .git is in `repo/`, cwd=`repo/crate/src` should return
        // `repo/crate` (Cargo.toml in closer ancestor).
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        let kid = repo.join("crate");
        touch(&repo.join(".git/HEAD"));
        touch(&kid.join("Cargo.toml"));
        fs::create_dir_all(kid.join("src")).unwrap();

        let home = tmp.path().parent().unwrap();
        let got = find_project_root_for_read_at(&kid.join("src"), Some(home)).unwrap();
        assert_eq!(got, kid, "closer Cargo.toml wins over further .git");
    }

    // --- walk-up opt-in (#30) ---

    /// Helper: construct a `db_exists` closure that returns true only for
    /// the given path (and whatever canonical variant Rust produces).
    fn db_at(path: &std::path::Path) -> impl Fn(&std::path::Path) -> bool + '_ {
        move |p| p == path
    }

    #[test]
    fn walk_up_off_stops_at_nested_marker_even_when_parent_has_db() {
        // Default behaviour: nested .git wins, ignores parent DB.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("repo");
        let sub = root.join("packages/module");
        touch(&sub.join(".git/HEAD"));
        fs::create_dir_all(&sub).unwrap();

        let home = tmp.path().parent().unwrap();
        let got = find_project_root_for_read_at_with_db(
            &sub,
            Some(home),
            false, // walk_up disabled
            db_at(&root),
        )
        .unwrap();
        assert_eq!(
            got, sub,
            "with walk_up=false, nested marker wins over parent DB"
        );
    }

    #[test]
    fn walk_up_on_prefers_parent_db_over_nested_marker() {
        // With walk-up enabled: parent DB wins.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("repo");
        let sub = root.join("packages/module");
        touch(&sub.join(".git/HEAD"));
        fs::create_dir_all(&sub).unwrap();

        let home = tmp.path().parent().unwrap();
        let got = find_project_root_for_read_at_with_db(
            &sub,
            Some(home),
            true, // walk_up enabled
            db_at(&root),
        )
        .unwrap();
        assert_eq!(
            got, root,
            "with walk_up=true, parent DB wins over nested marker"
        );
    }

    #[test]
    fn walk_up_on_still_stops_at_home_boundary() {
        // walk_up must not escape $HOME.
        let tmp = TempDir::new().unwrap();
        let fake_home = tmp.path().join("home");
        let above_home = tmp.path();
        let proj = fake_home.join("user/proj");
        fs::create_dir_all(&proj).unwrap();

        // DB is ABOVE $HOME — should be ignored.
        let got =
            find_project_root_for_read_at_with_db(&proj, Some(&fake_home), true, db_at(above_home))
                .unwrap();
        assert_eq!(
            got, proj,
            "walk_up must not escape $HOME even to find an existing DB"
        );
    }

    #[test]
    fn walk_up_off_still_finds_same_level_db() {
        // Regression: when walk_up is off, a DB in the nearest ancestor
        // is still preferred over a marker in the SAME ancestor. This is
        // the pre-existing behaviour and must not regress.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("repo");
        touch(&root.join(".git/HEAD"));
        fs::create_dir_all(root.join("src")).unwrap();

        let home = tmp.path().parent().unwrap();
        let got = find_project_root_for_read_at_with_db(
            &root.join("src"),
            Some(home),
            false,
            db_at(&root), // DB at root
        )
        .unwrap();
        // Both db and marker are at `root` — either is a valid answer,
        // but current code checks db_exists first in the per-ancestor
        // loop, so DB wins.
        assert_eq!(got, root);
    }

    #[test]
    fn walk_up_on_falls_back_to_markers_when_no_parent_db() {
        // With walk_up=true but no DB anywhere, behaviour falls back to
        // the marker-based walk — identical to default.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("repo");
        let sub = root.join("packages/module");
        touch(&root.join(".git/HEAD"));
        fs::create_dir_all(&sub).unwrap();

        let home = tmp.path().parent().unwrap();
        let got = find_project_root_for_read_at_with_db(
            &sub,
            Some(home),
            true,
            |_| false, // no DB anywhere
        )
        .unwrap();
        assert_eq!(
            got, root,
            "walk_up=true without any DB falls back to marker walk"
        );
    }
}

#[cfg(test)]
mod codex_mcp_install_tests {
    use super::*;
    use std::ffi::OsString;
    use std::fs;
    use tempfile::TempDir;

    fn fake_exe(dir: &std::path::Path, stem: &str) -> PathBuf {
        let name = if cfg!(windows) {
            format!("{stem}.exe")
        } else {
            stem.to_string()
        };
        let path = dir.join(name);
        fs::write(&path, b"").unwrap();
        path
    }

    #[test]
    fn codex_mcp_add_args_include_root_bin_and_server() {
        let install = CodexMcpInstall {
            project_root: PathBuf::from("/repo"),
            ast_index_bin: PathBuf::from("/bin/ast-index"),
            ast_index_mcp_bin: PathBuf::from("/bin/ast-index-mcp"),
        };

        assert_eq!(
            install.codex_args(),
            vec![
                "mcp",
                "add",
                "--env",
                "AST_INDEX_ROOT=/repo",
                "--env",
                "AST_INDEX_BIN=/bin/ast-index",
                "ast-index",
                "/bin/ast-index-mcp",
            ]
        );
    }

    #[test]
    fn fallback_config_escapes_toml_strings() {
        let install = CodexMcpInstall {
            project_root: PathBuf::from("/repo/with \"quotes\""),
            ast_index_bin: PathBuf::from("/bin/ast\\index"),
            ast_index_mcp_bin: PathBuf::from("/bin/ast-index-mcp"),
        };

        assert_eq!(
            install.fallback_config_toml(),
            "[mcp_servers.ast-index]\n\
             command = \"/bin/ast-index-mcp\"\n\
             env = { AST_INDEX_ROOT = \"/repo/with \\\"quotes\\\"\", AST_INDEX_BIN = \"/bin/ast\\\\index\" }\n"
        );
    }

    #[test]
    fn dry_run_output_contains_command_and_fallback_config() {
        let install = CodexMcpInstall {
            project_root: PathBuf::from("/repo"),
            ast_index_bin: PathBuf::from("/bin/ast-index"),
            ast_index_mcp_bin: PathBuf::from("/bin/ast-index-mcp"),
        };

        let out = install.dry_run_output();
        assert!(out.contains("codex mcp add --env AST_INDEX_ROOT=/repo"));
        assert!(out.contains("AST_INDEX_BIN=/bin/ast-index"));
        assert!(out.contains("[mcp_servers.ast-index]"));
        assert!(out.contains("command = \"/bin/ast-index-mcp\""));
    }

    #[test]
    fn resolve_ast_index_bin_uses_path_for_bare_invocation() {
        let tmp = TempDir::new().unwrap();
        let path_dir = tmp.path().join("path-bin");
        fs::create_dir_all(&path_dir).unwrap();
        let path_bin = fake_exe(&path_dir, "ast-index");

        let got = resolve_ast_index_bin_from(
            Some(OsStr::new("ast-index")),
            tmp.path(),
            Some(&OsString::from(&path_dir)),
        )
        .unwrap();

        assert_eq!(got, path_bin);
    }

    #[test]
    fn resolve_ast_index_bin_keeps_relative_invocation_without_canonicalizing() {
        let tmp = TempDir::new().unwrap();
        let invoked = Path::new(".")
            .join("target")
            .join("release")
            .join(ast_index_exe_name());

        let got = resolve_ast_index_bin_from(
            Some(invoked.as_os_str()),
            tmp.path(),
            Some(&OsString::new()),
        )
        .unwrap();

        assert_eq!(
            got,
            tmp.path()
                .join("target")
                .join("release")
                .join(ast_index_exe_name())
        );
    }

    #[test]
    fn resolve_mcp_bin_prefers_current_exe_directory() {
        let tmp = TempDir::new().unwrap();
        let current_exe = fake_exe(tmp.path(), "ast-index");
        let sibling_mcp = fake_exe(tmp.path(), "ast-index-mcp");
        let path_dir = tmp.path().join("path-bin");
        fs::create_dir_all(&path_dir).unwrap();
        fake_exe(&path_dir, "ast-index-mcp");

        let got =
            resolve_ast_index_mcp_bin_from(&current_exe, Some(&OsString::from(&path_dir))).unwrap();

        assert_eq!(got, sibling_mcp);
    }

    #[test]
    fn resolve_mcp_bin_falls_back_to_path() {
        let tmp = TempDir::new().unwrap();
        let current_exe = fake_exe(tmp.path(), "ast-index");
        let path_dir = tmp.path().join("path-bin");
        fs::create_dir_all(&path_dir).unwrap();
        let path_mcp = fake_exe(&path_dir, "ast-index-mcp");

        let got =
            resolve_ast_index_mcp_bin_from(&current_exe, Some(&OsString::from(&path_dir))).unwrap();

        assert_eq!(got, path_mcp);
    }
}
