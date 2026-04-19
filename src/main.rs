use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use ast_index::{db, commands, indexer};

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

    /// Output format: text or json
    #[arg(long, global = true, default_value = "text")]
    format: String,
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
        /// Filter by file path
        #[arg(long)]
        in_file: Option<String>,
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
        /// Filter by file path
        #[arg(long)]
        in_file: Option<String>,
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
        /// Force project type (e.g., dart, android, ios, python, go, rust, csharp, cpp, php, ruby, scala, bsl, frontend, perl, bazel)
        #[arg(long)]
        project_type: Option<String>,
        /// Verbose logging with timing for each step
        #[arg(long, short)]
        verbose: bool,
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
    /// Show cross-references: definitions, imports, usages
    Refs {
        /// Symbol name
        symbol: String,
        /// Max results per section
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Filter by file path
        #[arg(long)]
        in_file: Option<String>,
        /// Filter by module path
        #[arg(long)]
        module: Option<String>,
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
    let root = match &cli.command {
        Commands::Rebuild { .. } | Commands::Clear => find_project_root_for_write()?,
        _ => find_project_root_for_read()?,
    };
    let format = cli.format.as_str();

    // Migrate project DB from old kotlin-index to ast-index
    db::migrate_legacy_project(&root);

    // Compute directory scope: if cwd is inside project root, limit search to cwd subtree
    let cwd = std::env::current_dir().unwrap_or_default();
    let dir_prefix = if cwd != root {
        cwd.strip_prefix(&root)
            .ok()
            .map(|rel| {
                let mut s = rel.to_string_lossy().to_string();
                if !s.ends_with('/') { s.push('/'); }
                s
            })
    } else {
        None
    };
    let dir_prefix_ref = dir_prefix.as_deref();

    match cli.command {
        // Grep commands
        Commands::Todo { pattern, limit } => commands::grep::cmd_todo(&root, &pattern, limit),
        Commands::Callers { function_name, limit, in_file } => commands::grep::cmd_callers(&root, &function_name, limit, in_file.as_deref()),
        Commands::CallTree { function_name, depth, limit, in_file } => commands::grep::cmd_call_tree(&root, &function_name, depth, limit, in_file.as_deref()),
        Commands::Provides { type_name, limit } => commands::grep::cmd_provides(&root, &type_name, limit),
        Commands::Suspend { query, limit } => commands::grep::cmd_suspend(&root, query.as_deref(), limit),
        Commands::Composables { query, limit } => commands::grep::cmd_composables(&root, query.as_deref(), limit),
        Commands::Deprecated { query, limit } => commands::grep::cmd_deprecated(&root, query.as_deref(), limit),
        Commands::Suppress { query, limit } => commands::grep::cmd_suppress(&root, query.as_deref(), limit),
        Commands::Inject { type_name, limit } => commands::grep::cmd_inject(&root, &type_name, limit),
        Commands::Annotations { annotation, limit } => commands::grep::cmd_annotations(&root, &annotation, limit),
        Commands::Deeplinks { query, limit } => commands::grep::cmd_deeplinks(&root, query.as_deref(), limit),
        Commands::Extensions { receiver_type, limit } => commands::grep::cmd_extensions(&root, &receiver_type, limit),
        Commands::Flows { query, limit } => commands::grep::cmd_flows(&root, query.as_deref(), limit),
        Commands::Previews { query, limit } => commands::grep::cmd_previews(&root, query.as_deref(), limit),
        // Management commands
        Commands::Rebuild { r#type, no_deps, no_ignore, sub_projects, project_type, verbose, threads, include, exclude, paths } => {
            if let Some(t) = threads {
                std::env::set_var("AST_INDEX_THREADS", t.to_string());
            }
            let pt_override = project_type.as_ref().and_then(|s| indexer::ProjectType::from_str(s));
            commands::management::cmd_rebuild(&root, &r#type, !no_deps, no_ignore, sub_projects, pt_override, verbose, &include, &exclude, &paths)
        }
        Commands::Update { verbose } => commands::management::cmd_update(&root, verbose),
        Commands::Restore { path } => commands::management::cmd_restore(&root, &path),
        Commands::Stats => commands::management::cmd_stats(&root, format),
        // Index commands
        Commands::Search { query, r#type, limit, in_file, module, fuzzy } => {
            let scope = db::SearchScope { in_file: in_file.as_deref(), module: module.as_deref(), dir_prefix: dir_prefix_ref };
            commands::index::cmd_search(&root, &query, r#type.as_deref(), limit, format, &scope, fuzzy)
        }
        Commands::Symbol { name, pattern, r#type, limit, in_file, module, fuzzy } => {
            let scope = db::SearchScope { in_file: in_file.as_deref(), module: module.as_deref(), dir_prefix: dir_prefix_ref };
            commands::index::cmd_symbol(&root, name.as_deref(), pattern.as_deref(), r#type.as_deref(), limit, format, &scope, fuzzy)
        }
        Commands::Class { name, pattern, limit, in_file, module, fuzzy } => {
            let scope = db::SearchScope { in_file: in_file.as_deref(), module: module.as_deref(), dir_prefix: dir_prefix_ref };
            commands::index::cmd_class(&root, name.as_deref(), pattern.as_deref(), limit, format, &scope, fuzzy)
        }
        Commands::Implementations { parent, limit, in_file, module } => {
            let scope = db::SearchScope { in_file: in_file.as_deref(), module: module.as_deref(), dir_prefix: dir_prefix_ref };
            commands::index::cmd_implementations(&root, &parent, limit, format, &scope)
        }
        Commands::Refs { symbol, limit, in_file, module } => {
            let scope = db::SearchScope { in_file: in_file.as_deref(), module: module.as_deref(), dir_prefix: dir_prefix_ref };
            commands::index::cmd_refs(&root, &symbol, limit, format, &scope)
        }
        Commands::Hierarchy { name, in_file, module } => {
            let scope = db::SearchScope { in_file: in_file.as_deref(), module: module.as_deref(), dir_prefix: dir_prefix_ref };
            commands::index::cmd_hierarchy(&root, &name, &scope)
        }
        Commands::Usages { symbol, limit, in_file, module } => {
            let scope = db::SearchScope { in_file: in_file.as_deref(), module: module.as_deref(), dir_prefix: dir_prefix_ref };
            commands::index::cmd_usages(&root, &symbol, limit, format, &scope)
        }
        // Module commands
        Commands::Module { pattern, limit } => commands::modules::cmd_module(&root, &pattern, limit),
        Commands::Deps { module } => commands::modules::cmd_deps(&root, &module),
        Commands::Dependents { module } => commands::modules::cmd_dependents(&root, &module),
        Commands::UnusedDeps { module, verbose, no_transitive, no_xml, no_resources, strict } => {
            let check_transitive = !no_transitive && !strict;
            let check_xml = !no_xml && !strict;
            let check_resources = !no_resources && !strict;
            commands::modules::cmd_unused_deps(&root, &module, verbose, check_transitive, check_xml, check_resources)
        }
        // File commands
        Commands::File { pattern, exact, limit } => commands::files::cmd_file(&root, &pattern, exact, limit),
        Commands::Outline { file } => commands::files::cmd_outline(&root, &file),
        Commands::Imports { file } => commands::files::cmd_imports(&root, &file),
        Commands::Api { module_path, limit } => commands::files::cmd_api(&root, &module_path, limit),
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
        Commands::XmlUsages { class_name, module } => commands::android::cmd_xml_usages(&root, &class_name, module.as_deref()),
        Commands::ResourceUsages { resource, module, r#type, unused } => {
            commands::android::cmd_resource_usages(&root, &resource, module.as_deref(), r#type.as_deref(), unused)
        }
        // iOS commands
        Commands::StoryboardUsages { class_name, module } => commands::ios::cmd_storyboard_usages(&root, &class_name, module.as_deref()),
        Commands::AssetUsages { asset, module, r#type, unused } => commands::ios::cmd_asset_usages(&root, &asset, module.as_deref(), r#type.as_deref(), unused),
        Commands::Swiftui { query, limit } => commands::ios::cmd_swiftui(&root, query.as_deref(), limit),
        Commands::AsyncFuncs { query, limit } => commands::ios::cmd_async_funcs(&root, query.as_deref(), limit),
        Commands::Publishers { query, limit } => commands::ios::cmd_publishers(&root, query.as_deref(), limit),
        Commands::MainActor { query, limit } => commands::ios::cmd_main_actor(&root, query.as_deref(), limit),
        // Perl commands
        Commands::PerlExports { query, limit } => commands::perl::cmd_perl_exports(&root, query.as_deref(), limit),
        Commands::PerlSubs { query, limit } => commands::perl::cmd_perl_subs(&root, query.as_deref(), limit),
        Commands::PerlPod { query, limit } => commands::perl::cmd_perl_pod(&root, query.as_deref(), limit),
        Commands::PerlTests { query, limit } => commands::perl::cmd_perl_tests(&root, query.as_deref(), limit),
        Commands::PerlImports { query, limit } => commands::perl::cmd_perl_imports(&root, query.as_deref(), limit),
        // Project insights
        Commands::Map { module, per_dir, limit } => commands::project_info::cmd_map(&root, module.as_deref(), per_dir, limit, format),
        Commands::Conventions => commands::project_info::cmd_conventions(&root, format),
        Commands::UnusedSymbols { module, export_only, limit } => {
            commands::analysis::cmd_unused_symbols(&root, module.as_deref(), export_only, limit, format)
        }
        Commands::AddRoot { path, force } => commands::management::cmd_add_root(&root, &path, force),
        Commands::RemoveRoot { path } => commands::management::cmd_remove_root(&root, &path),
        Commands::ListRoots => commands::management::cmd_list_roots(&root),
        Commands::Watch => commands::watch::cmd_watch(&root),
        Commands::Clear => commands::management::cmd_clear(&root),
        Commands::Version => {
            println!("ast-index v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Commands::InstallClaudePlugin => cmd_install_claude_plugin(),
        // Programmatic access
        Commands::Agrep { pattern, lang, json } => commands::grep::cmd_ast_grep(&root, &pattern, lang.as_deref(), json),
        Commands::Query { sql, limit } => commands::management::cmd_query(&root, &sql, limit),
        Commands::DbPath => commands::management::cmd_db_path(&root),
        Commands::Schema => commands::management::cmd_schema(&root),
    }
}

fn cmd_install_claude_plugin() -> Result<()> {
    use std::process::Command;

    println!("Adding ast-index marketplace...");
    let status = Command::new("claude")
        .args(["plugin", "marketplace", "add", "defendend/Claude-ast-index-search"])
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
            return Err(anyhow::anyhow!("Failed to run claude plugin install: {}", e));
        }
    }

    Ok(())
}

fn find_project_root_for_write() -> Result<PathBuf> {
    Ok(std::env::current_dir()?)
}

fn find_project_root_for_read() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let home = dirs::home_dir();
    for ancestor in cwd.ancestors() {
        // Never go above $HOME — prevents indexing entire user directory
        if let Some(ref h) = home {
            if ancestor == h.as_path() {
                break;
            }
        }
        // Check if an index DB already exists for this ancestor
        if db::db_exists(ancestor) {
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
                if entry.path().extension().map(|e| e == "xcodeproj").unwrap_or(false) {
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
        if ancestor.join("pyproject.toml").exists()
            || ancestor.join("setup.py").exists()
        {
            return Ok(ancestor.to_path_buf());
        }
        // C#/.NET markers
        if let Ok(entries) = std::fs::read_dir(ancestor) {
            let has_sln = entries
                .flatten()
                .any(|e| e.path().extension().map(|ext| ext == "sln").unwrap_or(false));
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
    Ok(cwd)
}
