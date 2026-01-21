mod db;
mod indexer;
mod parsers;
mod commands;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
        /// Include gitignored files (e.g., build/ directories)
        #[arg(long)]
        no_ignore: bool,
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
    /// Show version
    Version,
    /// Install Claude Code plugin to ~/.claude/plugins/
    InstallClaudePlugin,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = find_project_root()?;

    match cli.command {
        // Grep commands
        Commands::Todo { pattern, limit } => commands::grep::cmd_todo(&root, &pattern, limit),
        Commands::Callers { function_name, limit } => commands::grep::cmd_callers(&root, &function_name, limit),
        Commands::CallTree { function_name, depth, limit } => commands::grep::cmd_call_tree(&root, &function_name, depth, limit),
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
        Commands::Init => commands::management::cmd_init(&root),
        Commands::Rebuild { r#type, no_deps, no_ignore } => commands::management::cmd_rebuild(&root, &r#type, !no_deps, no_ignore),
        Commands::Update => commands::management::cmd_update(&root),
        Commands::Stats => commands::management::cmd_stats(&root),
        // Index commands
        Commands::Search { query, limit } => commands::index::cmd_search(&root, &query, limit),
        Commands::Symbol { name, r#type, limit } => commands::index::cmd_symbol(&root, &name, r#type.as_deref(), limit),
        Commands::Class { name, limit } => commands::index::cmd_class(&root, &name, limit),
        Commands::Implementations { parent, limit } => commands::index::cmd_implementations(&root, &parent, limit),
        Commands::Hierarchy { name } => commands::index::cmd_hierarchy(&root, &name),
        Commands::Usages { symbol, limit } => commands::index::cmd_usages(&root, &symbol, limit),
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
        Commands::Changed { base } => commands::files::cmd_changed(&root, &base),
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
        Commands::Version => {
            println!("ast-index v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Commands::InstallClaudePlugin => cmd_install_claude_plugin(),
    }
}

fn cmd_install_claude_plugin() -> Result<()> {
    use std::fs;
    use std::io::Write;

    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;
    let plugin_dir = home.join(".claude").join("plugins").join("ast-index");

    // Create plugin directory
    fs::create_dir_all(&plugin_dir)?;
    fs::create_dir_all(plugin_dir.join(".claude-plugin"))?;
    fs::create_dir_all(plugin_dir.join("skills").join("ast-index").join("references"))?;
    fs::create_dir_all(plugin_dir.join("commands"))?;

    // Write plugin.json
    let plugin_json = include_str!("../plugin/.claude-plugin/plugin.json");
    fs::write(plugin_dir.join(".claude-plugin").join("plugin.json"), plugin_json)?;

    // Write skill files
    let skill_md = include_str!("../plugin/skills/ast-index/SKILL.md");
    fs::write(plugin_dir.join("skills").join("ast-index").join("SKILL.md"), skill_md)?;

    // Write reference files
    let refs = [
        ("android-commands.md", include_str!("../plugin/skills/ast-index/references/android-commands.md")),
        ("go-commands.md", include_str!("../plugin/skills/ast-index/references/go-commands.md")),
        ("ios-commands.md", include_str!("../plugin/skills/ast-index/references/ios-commands.md")),
        ("module-commands.md", include_str!("../plugin/skills/ast-index/references/module-commands.md")),
        ("perl-commands.md", include_str!("../plugin/skills/ast-index/references/perl-commands.md")),
        ("python-commands.md", include_str!("../plugin/skills/ast-index/references/python-commands.md")),
    ];
    for (name, content) in refs {
        fs::write(plugin_dir.join("skills").join("ast-index").join("references").join(name), content)?;
    }

    // Write command files
    let commands = [
        ("initialize-android.md", include_str!("../plugin/commands/initialize-android.md")),
        ("initialize-ios.md", include_str!("../plugin/commands/initialize-ios.md")),
    ];
    for (name, content) in commands {
        fs::write(plugin_dir.join("commands").join(name), content)?;
    }

    println!("Claude Code plugin installed to: {}", plugin_dir.display());
    println!("\nRestart Claude Code to activate the plugin.");
    Ok(())
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
