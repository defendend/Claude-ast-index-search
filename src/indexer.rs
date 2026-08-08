use anyhow::{Context, Result};
use rayon::prelude::*;
use regex::Regex;
use rusqlite::Connection;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::LazyLock;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::db;
use crate::parsers::{self, ParsedRef, ParsedSymbol};

/// File-size cap for parsing. Larger files are recorded in the `files`
/// table (so `update` still tracks their mtime) but never parsed — their
/// symbol contribution is 0. This prevents pathological RAM peaks on
/// minified bundles / generated proto bundles / vendor blobs.
///
/// Configurable via `AST_INDEX_MAX_FILE_SIZE` (bytes). Default: 1 MB.
fn max_file_size_bytes() -> u64 {
    std::env::var("AST_INDEX_MAX_FILE_SIZE")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(1_000_000)
}

/// Soft cap on the number of candidate files the walker accepts before
/// aborting. Protects against accidental `rebuild` on a VCS / monorepo
/// root that would index hundreds of millions of files.
///
/// Configurable via `AST_INDEX_MAX_FILES`. Default: 2_000_000. Set to 0
/// to disable entirely.
fn max_files_cap() -> usize {
    std::env::var("AST_INDEX_MAX_FILES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(2_000_000)
}

/// Soft threshold at which the walker prints a one-shot warning to stderr
/// but keeps going. Lets the user notice "I'm rebuilding something huge"
/// without aborting projects that legitimately have ~1M files.
///
/// Configurable via `AST_INDEX_WARN_FILES`. Default: 500_000. Set to 0 to
/// silence entirely.
fn warn_files_threshold() -> usize {
    std::env::var("AST_INDEX_WARN_FILES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(500_000)
}

/// Read the per-project `bypass_size_check` opt-in flag.
///
/// Set by `ast-index rebuild --force --remember` for projects where the
/// user has explicitly accepted the cost of indexing a very large root.
/// Future `rebuild`/`update` runs on that project skip the cap silently.
fn cap_disabled_for_root(conn: &Connection) -> bool {
    let row: rusqlite::Result<String> = conn.query_row(
        "SELECT value FROM metadata WHERE key = 'bypass_size_check'",
        [],
        |row| row.get(0),
    );
    matches!(row.as_deref(), Ok("1") | Ok("true") | Ok("yes"))
}

/// Per-worker stack size for the rayon parsing pool.
///
/// Tree-sitter parsers recurse on each node of the AST; pathological inputs
/// (Dart SDK test corpus, deeply nested generics, long expression chains) can
/// overflow the Rust default (≈ 2 MB on most platforms). 32 MB gives plenty
/// of headroom without committing the pages eagerly.
const RAYON_WORKER_STACK_SIZE: usize = 32 * 1024 * 1024;
const DEFAULT_PARALLELISM_CAP: usize = 8;

fn effective_num_threads() -> usize {
    std::env::var("AST_INDEX_THREADS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get().min(DEFAULT_PARALLELISM_CAP))
                .unwrap_or(4)
        })
}

fn effective_chunk_size(total_files: usize) -> usize {
    if total_files >= 20_000 {
        1_000
    } else {
        500
    }
}

/// Sorted module lookup for efficient longest-prefix matching.
/// Entries sorted by path length descending so the longest (most specific) match is found first.
#[derive(Clone)]
struct ModuleLookup {
    sorted: Vec<(String, i64)>, // (path, module_id) sorted by path length desc
}

impl ModuleLookup {
    fn from_db(conn: &Connection) -> Result<Self> {
        let mut stmt = conn.prepare("SELECT id, path FROM modules")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(0)?))
        })?;
        let mut sorted: Vec<(String, i64)> = Vec::new();
        for row in rows {
            let (path, id) = row?;
            sorted.push((path, id));
        }
        sorted.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        Ok(ModuleLookup { sorted })
    }

    fn find(&self, file_path: &str) -> Option<i64> {
        self.sorted
            .iter()
            .find(|(path, _)| file_path.starts_with(path.as_str()))
            .map(|(_, id)| *id)
    }
}

/// Project type detected by markers
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProjectType {
    Android,  // Kotlin/Java - build.gradle.kts, settings.gradle.kts
    IOS,      // Swift/ObjC - Package.swift, *.xcodeproj
    Perl,     // Perl - .pm files, Makefile.PL, Build.PL
    Frontend, // JS/TS - package.json
    Python,   // Python - pyproject.toml, setup.py, setup.cfg
    Go,       // Go - go.mod
    Rust,     // Rust - Cargo.toml
    Bazel,    // Bazel - BUILD, WORKSPACE
    Bsl,      // 1C:Enterprise - Configuration.mdo, Configuration.xml, .bsl files
    CSharp,   // C# - *.csproj, *.sln
    Cpp,      // C++ - CMakeLists.txt with .cpp/.h files
    Dart,     // Dart/Flutter - pubspec.yaml
    PHP,      // PHP - composer.json
    Ruby,     // Ruby - Gemfile, *.gemspec
    Scala,    // Scala - build.sbt
    Matlab,   // Matlab - .m files with classdef/function
    Zig,      // Zig - build.zig, build.zig.zon
    Sql,      // SQL - .sql files (no build-system marker, extension-only)
    Mixed,    // Multiple platforms present
    Unknown,
}

impl ProjectType {
    pub fn as_str(&self) -> &str {
        match self {
            ProjectType::Android => "Android (Kotlin/Java)",
            ProjectType::IOS => "iOS (Swift/ObjC)",
            ProjectType::Perl => "Perl",
            ProjectType::Frontend => "Frontend (JS/TS)",
            ProjectType::Python => "Python",
            ProjectType::Go => "Go",
            ProjectType::Rust => "Rust",
            ProjectType::Bazel => "Bazel",
            ProjectType::Bsl => "1C:Enterprise (BSL)",
            ProjectType::CSharp => "C# (.NET)",
            ProjectType::Cpp => "C/C++",
            ProjectType::Dart => "Dart/Flutter",
            ProjectType::PHP => "PHP",
            ProjectType::Ruby => "Ruby",
            ProjectType::Scala => "Scala",
            ProjectType::Matlab => "Matlab",
            ProjectType::Zig => "Zig",
            ProjectType::Sql => "SQL",
            ProjectType::Mixed => "Mixed",
            ProjectType::Unknown => "Unknown",
        }
    }
}

impl ProjectType {
    pub fn from_str(s: &str) -> Option<ProjectType> {
        match s.to_lowercase().as_str() {
            "android" | "kotlin" | "java" => Some(ProjectType::Android),
            "ios" | "swift" | "objc" => Some(ProjectType::IOS),
            "perl" => Some(ProjectType::Perl),
            "frontend" | "js" | "ts" | "typescript" | "javascript" => Some(ProjectType::Frontend),
            "python" | "py" => Some(ProjectType::Python),
            "go" | "golang" => Some(ProjectType::Go),
            "rust" | "rs" => Some(ProjectType::Rust),
            "bazel" => Some(ProjectType::Bazel),
            "bsl" | "1c" | "onescript" => Some(ProjectType::Bsl),
            "csharp" | "c#" | "cs" | "dotnet" | ".net" => Some(ProjectType::CSharp),
            "cpp" | "c++" | "c" => Some(ProjectType::Cpp),
            "dart" | "flutter" => Some(ProjectType::Dart),
            "php" | "laravel" => Some(ProjectType::PHP),
            "ruby" | "rb" | "rails" => Some(ProjectType::Ruby),
            "scala" | "sbt" => Some(ProjectType::Scala),
            "matlab" | "m" => Some(ProjectType::Matlab),
            "zig" => Some(ProjectType::Zig),
            "sql" => Some(ProjectType::Sql),
            _ => None,
        }
    }
}

/// Project configuration loaded from `.ast-index.yaml`
#[derive(serde::Deserialize, Default, Debug)]
pub struct ProjectConfig {
    pub roots: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
    /// Allow-list: only index these directories (relative to root).
    /// When set, only matching top-level directories are indexed; everything else is skipped.
    pub include: Option<Vec<String>>,
    pub no_ignore: Option<bool>,
}

/// Load project config from `.ast-index.yaml` or `.ast-index.yml` in the given root.
/// Returns `None` if no config file found or on parse error (with warning).
pub fn load_config(root: &Path) -> Option<ProjectConfig> {
    match load_config_strict(root) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("Warning: {error:#}");
            None
        }
    }
}

/// Load project config while preserving parse and read errors for commands
/// that cannot safely fall back to scanning the whole project root.
pub fn load_config_strict(root: &Path) -> Result<Option<ProjectConfig>> {
    let yaml_path = root.join(".ast-index.yaml");
    let yml_path = root.join(".ast-index.yml");
    let config_path = if yaml_path.exists() {
        yaml_path
    } else if yml_path.exists() {
        yml_path
    } else {
        return Ok(None);
    };

    let content = fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let config = serde_yaml::from_str::<ProjectConfig>(&content)
        .with_context(|| format!("failed to parse {}", config_path.display()))?;
    eprintln!("Loaded config from {}", config_path.display());
    Ok(Some(config))
}

/// Check if project has build system markers (Gradle/Maven build files)
pub fn has_android_markers(root: &Path) -> bool {
    root.join("settings.gradle.kts").exists()
        || root.join("settings.gradle").exists()
        || root.join("build.gradle.kts").exists()
        || root.join("build.gradle").exists()
        || root.join("pom.xml").exists()
}

/// Check if project has iOS markers (Xcode/SPM)
pub fn has_ios_markers(root: &Path) -> bool {
    if root.join("Package.swift").exists() {
        return true;
    }
    // Check for .xcodeproj
    fs::read_dir(root)
        .map(|entries| {
            entries.filter_map(|e| e.ok()).any(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext == "xcodeproj")
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Find immediate subdirectories that are project roots.
/// Returns list of (path, project_type) for dirs with recognized project markers.
/// If 2+ subdirs have markers, treats root as monorepo and includes ALL subdirs.
/// `exclude` — optional gitignore-style matcher anchored to `root`; matching dirs are skipped.
/// `include` — optional allow-list. When set, include entries are treated as explicit
/// scoped roots (relative to `root`), and can point to arbitrarily nested directories —
/// not just immediate subdirs of `root`. Each include entry becomes a separate sub-project.
pub fn find_sub_projects(
    root: &Path,
    exclude: Option<&ignore::gitignore::Gitignore>,
    include: Option<&[String]>,
) -> Vec<(PathBuf, ProjectType)> {
    // When include is explicitly set, honor it literally: each entry is a scoped root.
    // This allows deep paths like "smart_devices/tools/burn_data" instead of being forced
    // to top-level subdirs only.
    if let Some(inc) = include {
        let mut result: Vec<(PathBuf, ProjectType)> = Vec::new();
        for entry in inc {
            let path = root.join(entry);
            if !path.is_dir() {
                continue;
            }
            let pt = detect_project_type(&path);
            result.push((path, pt));
        }
        result.sort_by(|a, b| a.0.cmp(&b.0));
        return result;
    }

    let mut marked = Vec::new();
    let mut all_dirs = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return marked,
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Skip hidden and hard-coded excluded dirs
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with('.') || EXCLUDED_DIRS.contains(&name) {
                continue;
            }
        }
        // Skip dirs matching config exclude patterns
        if let Some(m) = exclude {
            if m.matched(&path, true).is_ignore() {
                continue;
            }
        }
        let pt = detect_project_type(&path);
        let has_marker = pt != ProjectType::Unknown || has_build_marker(&path);
        if has_marker {
            marked.push((path.clone(), pt));
        }
        all_dirs.push((path, pt));
    }
    // If 2+ subdirs have markers → monorepo, index ALL subdirs
    let mut result = if marked.len() >= 2 { all_dirs } else { marked };
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

/// Check if directory has any build system marker (for monorepo sub-project detection)
fn has_build_marker(path: &Path) -> bool {
    path.join("ya.make").exists()
        || path.join("Makefile").exists()
        || path.join("BUILD").exists()
        || path.join("BUILD.bazel").exists()
        || path.join("CMakeLists.txt").exists()
}

/// Detect project type by looking for marker files
pub fn detect_project_type(root: &Path) -> ProjectType {
    let has_gradle = root.join("settings.gradle.kts").exists()
        || root.join("settings.gradle").exists()
        || root.join("build.gradle.kts").exists()
        || root.join("build.gradle").exists()
        || root.join("pom.xml").exists();

    let has_swift = root.join("Package.swift").exists()
        || fs::read_dir(root)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext == "xcodeproj")
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);

    // Also check subdirectories for Package.swift (SPM structure)
    let has_swift = has_swift || {
        fs::read_dir(root)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    let path = e.path();
                    path.is_dir() && path.join("Package.swift").exists()
                })
            })
            .unwrap_or(false)
    };

    // Perl project detection: Makefile.PL, Build.PL, or .pm files in root
    let has_perl = root.join("Makefile.PL").exists()
        || root.join("Build.PL").exists()
        || root.join("cpanfile").exists()
        || fs::read_dir(root)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .any(|e| e.path().extension().map(|ext| ext == "pm").unwrap_or(false))
            })
            .unwrap_or(false);

    // Frontend (JS/TS) project detection
    let has_frontend = root.join("package.json").exists();

    // Python project detection
    let has_python = root.join("pyproject.toml").exists()
        || root.join("setup.py").exists()
        || root.join("setup.cfg").exists();

    // Go project detection
    let has_go = root.join("go.mod").exists();

    // Rust project detection
    let has_rust = root.join("Cargo.toml").exists();

    // Bazel project detection
    let has_bazel = root.join("WORKSPACE").exists()
        || root.join("WORKSPACE.bazel").exists()
        || root.join("MODULE.bazel").exists();

    // 1C:Enterprise (BSL) project detection
    let has_bsl = root.join("src/Configuration/Configuration.mdo").exists()
        || root.join("Configuration/Configuration.mdo").exists()
        || root.join("Configuration.xml").exists()
        || root.join("ConfigDumpInfo.xml").exists()
        || root.join("packagedef").exists()
        || fs::read_dir(root)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext == "bsl" || ext == "os")
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);

    // C# project detection
    let has_csharp = root.join("Directory.Build.props").exists()
        || fs::read_dir(root)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext == "sln" || ext == "csproj")
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);

    // C++ project detection (CMakeLists.txt without other markers, or ya.make with C/C++ files)
    let has_cpp = root.join("CMakeLists.txt").exists()
        || (root.join("Makefile").exists() && !has_perl)
        || (root.join("ya.make").exists() && !has_gradle && !has_python && !has_go && !has_rust);

    // Dart/Flutter project detection
    let has_dart = root.join("pubspec.yaml").exists();

    // PHP project detection
    let has_php = root.join("composer.json").exists();

    // Ruby project detection
    let has_ruby = root.join("Gemfile").exists()
        || fs::read_dir(root)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext == "gemspec")
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);

    // Scala project detection
    let has_scala = root.join("build.sbt").exists();

    // Matlab project detection: look for startup.m, pathdef.m, + package dirs,
    // or .m files containing classdef/function keywords (not ObjC markers)
    let has_matlab = root.join("startup.m").exists()
        || root.join("pathdef.m").exists()
        || fs::read_dir(root)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    let name = e.file_name();
                    let name = name.to_string_lossy();
                    // + prefix directories are Matlab package directories
                    name.starts_with('+') && e.path().is_dir()
                })
            })
            .unwrap_or(false)
        || {
            // Sample a .m file to check for Matlab keywords
            fs::read_dir(root)
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok())
                        .filter(|e| e.path().extension().map(|ext| ext == "m").unwrap_or(false))
                        .take(3)
                        .any(|e| {
                            fs::read_to_string(e.path())
                                .map(|content| {
                                    let trimmed = content.trim_start();
                                    trimmed.starts_with("classdef")
                                        || trimmed.starts_with("function")
                                        || trimmed.starts_with('%')
                                })
                                .unwrap_or(false)
                        })
                })
                .unwrap_or(false)
        };

    // Zig project detection: build.zig (primary) or build.zig.zon (package manifest)
    let has_zig = root.join("build.zig").exists() || root.join("build.zig.zon").exists();

    // SQL project detection: no canonical build-system marker, so fall back to a
    // .sql file in root (migrations/, schemas/, query dumps).
    let has_sql = fs::read_dir(root)
        .map(|entries| {
            entries.filter_map(|e| e.ok()).any(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext == "sql")
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);

    // Count how many platforms are detected
    let count = [
        has_gradle,
        has_swift,
        has_perl,
        has_frontend,
        has_python,
        has_go,
        has_rust,
        has_bazel,
        has_bsl,
        has_csharp,
        has_cpp,
        has_dart,
        has_php,
        has_ruby,
        has_scala,
        has_matlab,
        has_zig,
        has_sql,
    ]
    .iter()
    .filter(|&&x| x)
    .count();

    if count > 1 {
        ProjectType::Mixed
    } else if has_gradle {
        ProjectType::Android
    } else if has_swift {
        ProjectType::IOS
    } else if has_perl {
        ProjectType::Perl
    } else if has_frontend {
        ProjectType::Frontend
    } else if has_python {
        ProjectType::Python
    } else if has_go {
        ProjectType::Go
    } else if has_rust {
        ProjectType::Rust
    } else if has_bazel {
        ProjectType::Bazel
    } else if has_bsl {
        ProjectType::Bsl
    } else if has_csharp {
        ProjectType::CSharp
    } else if has_dart {
        ProjectType::Dart
    } else if has_cpp {
        ProjectType::Cpp
    } else if has_php {
        ProjectType::PHP
    } else if has_ruby {
        ProjectType::Ruby
    } else if has_scala {
        ProjectType::Scala
    } else if has_matlab {
        ProjectType::Matlab
    } else if has_zig {
        ProjectType::Zig
    } else if has_sql {
        ProjectType::Sql
    } else {
        ProjectType::Unknown
    }
}

/// One stack detected in the project root by marker files.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DetectedStack {
    /// Short id: "android", "ios", "kmp", "web", "rust", ...
    pub kind: String,
    /// Human-readable label suitable for CLI output.
    pub label: String,
    /// Specific marker files that triggered the detection (relative to root).
    pub markers: Vec<String>,
}

/// Result of `detect_stacks()`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StackDetection {
    pub stacks: Vec<DetectedStack>,
    /// Kotlin Multiplatform: Kotlin plugin + commonMain/<platform>Main source sets.
    pub is_kmp: bool,
    /// True when more than one independent stack is present and it is not a KMP repo.
    pub is_polyglot: bool,
}

fn collect_markers(root: &Path, candidates: &[&str]) -> Vec<String> {
    candidates
        .iter()
        .filter(|name| root.join(name).exists())
        .map(|s| s.to_string())
        .collect()
}

fn any_ext_in_root(root: &Path, ext: &str) -> bool {
    fs::read_dir(root)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .any(|e| e.path().extension().map(|x| x == ext).unwrap_or(false))
        })
        .unwrap_or(false)
}

fn collect_ext_markers(root: &Path, ext: &str, limit: usize) -> Vec<String> {
    fs::read_dir(root)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().map(|x| x == ext).unwrap_or(false))
                .take(limit)
                .filter_map(|e| {
                    e.path()
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn has_kmp_markers(root: &Path) -> (bool, Vec<String>) {
    // KMP requires two independent signals:
    //   * a Kotlin multiplatform source set directory (commonMain / *Main),
    //   * a Gradle plugin reference (`kotlin("multiplatform")` or
    //     `org.jetbrains.kotlin.multiplatform`).
    // Either alone is too weak — non-KMP Android repos sometimes have stray
    // `commonMain` folders, and some Gradle catalogs declare the plugin without
    // applying it.
    let mut markers = Vec::new();

    let kmp_source_sets = [
        "commonMain",
        "commonTest",
        "androidMain",
        "iosMain",
        "iosArm64Main",
        "iosSimulatorArm64Main",
        "iosX64Main",
        "jsMain",
        "wasmJsMain",
        "jvmMain",
        "nativeMain",
    ];
    'sources: for top in [
        "src",
        "shared",
        "common",
        "core",
        "kmp",
        "composeApp",
        "androidApp",
    ] {
        let top_path = root.join(top);
        if !top_path.is_dir() {
            continue;
        }
        if let Ok(entries) = fs::read_dir(&top_path) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if kmp_source_sets.contains(&name.as_str()) {
                    markers.push(format!("{}/{}", top, name));
                    break 'sources;
                }
            }
        }
    }
    let has_source_set = !markers.is_empty();

    let plugin_files = [
        "build.gradle.kts",
        "build.gradle",
        "settings.gradle.kts",
        "settings.gradle",
    ];
    let mut has_plugin = false;
    'gradle: for file in plugin_files {
        let subdirs: Vec<PathBuf> = fs::read_dir(root)
            .map(|it| {
                it.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect()
            })
            .unwrap_or_default();
        for base in std::iter::once(root.to_path_buf()).chain(subdirs) {
            let path = base.join(file);
            if !path.is_file() {
                continue;
            }
            if let Ok(content) = fs::read_to_string(&path) {
                if content.contains("kotlin(\"multiplatform\")")
                    || content.contains("org.jetbrains.kotlin.multiplatform")
                {
                    let rel = path
                        .strip_prefix(root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .into_owned();
                    markers.push(rel);
                    has_plugin = true;
                    break 'gradle;
                }
            }
        }
    }

    (has_source_set && has_plugin, markers)
}

/// Detect every stack present in `root` by inspecting marker files.
///
/// Unlike `detect_project_type` (which collapses multiple stacks to
/// `ProjectType::Mixed`), this returns the full list with the specific
/// marker files that triggered each detection. The smart `initialize`
/// command consumes this to compose Android+iOS rules for KMP repos and
/// per-stack rules for polyglot monorepos.
pub fn detect_stacks(root: &Path) -> StackDetection {
    let mut stacks: Vec<DetectedStack> = Vec::new();

    let android_markers = collect_markers(
        root,
        &[
            "settings.gradle.kts",
            "settings.gradle",
            "build.gradle.kts",
            "build.gradle",
            "libs.versions.toml",
            "pom.xml",
        ],
    );
    if !android_markers.is_empty() {
        stacks.push(DetectedStack {
            kind: "android".to_string(),
            label: "Android (Kotlin/Java/JVM)".to_string(),
            markers: android_markers,
        });
    }

    let mut ios_markers = collect_markers(root, &["Package.swift", "Podfile"]);
    ios_markers.extend(collect_ext_markers(root, "xcodeproj", 3));
    ios_markers.extend(collect_ext_markers(root, "xcworkspace", 3));
    if !ios_markers.is_empty() {
        stacks.push(DetectedStack {
            kind: "ios".to_string(),
            label: "iOS (Swift/ObjC)".to_string(),
            markers: ios_markers,
        });
    }

    let (kmp_found, kmp_markers) = has_kmp_markers(root);
    if kmp_found {
        stacks.push(DetectedStack {
            kind: "kmp".to_string(),
            label: "Kotlin Multiplatform".to_string(),
            markers: kmp_markers,
        });
    }

    let web_markers = collect_markers(
        root,
        &[
            "package.json",
            "tsconfig.json",
            "vite.config.ts",
            "vite.config.js",
            "next.config.js",
            "next.config.mjs",
            "nuxt.config.ts",
            "angular.json",
        ],
    );
    if !web_markers.is_empty() {
        stacks.push(DetectedStack {
            kind: "web".to_string(),
            label: "Web (TypeScript/JavaScript)".to_string(),
            markers: web_markers,
        });
    }

    let rust_markers = collect_markers(root, &["Cargo.toml"]);
    if !rust_markers.is_empty() {
        stacks.push(DetectedStack {
            kind: "rust".to_string(),
            label: "Rust".to_string(),
            markers: rust_markers,
        });
    }

    let mut csharp_markers = collect_markers(root, &["Directory.Build.props"]);
    csharp_markers.extend(collect_ext_markers(root, "sln", 3));
    csharp_markers.extend(collect_ext_markers(root, "csproj", 3));
    if !csharp_markers.is_empty() {
        stacks.push(DetectedStack {
            kind: "csharp".to_string(),
            label: "C# / .NET".to_string(),
            markers: csharp_markers,
        });
    }

    let mut ruby_markers = collect_markers(root, &["Gemfile"]);
    ruby_markers.extend(collect_ext_markers(root, "gemspec", 3));
    if !ruby_markers.is_empty() {
        stacks.push(DetectedStack {
            kind: "ruby".to_string(),
            label: "Ruby".to_string(),
            markers: ruby_markers,
        });
    }

    let python_markers = collect_markers(root, &["pyproject.toml", "setup.py", "setup.cfg"]);
    if !python_markers.is_empty() {
        stacks.push(DetectedStack {
            kind: "python".to_string(),
            label: "Python".to_string(),
            markers: python_markers,
        });
    }

    let go_markers = collect_markers(root, &["go.mod"]);
    if !go_markers.is_empty() {
        stacks.push(DetectedStack {
            kind: "go".to_string(),
            label: "Go".to_string(),
            markers: go_markers,
        });
    }

    let dart_markers = collect_markers(root, &["pubspec.yaml"]);
    if !dart_markers.is_empty() {
        stacks.push(DetectedStack {
            kind: "dart".to_string(),
            label: "Dart / Flutter".to_string(),
            markers: dart_markers,
        });
    }

    let php_markers = collect_markers(root, &["composer.json"]);
    if !php_markers.is_empty() {
        stacks.push(DetectedStack {
            kind: "php".to_string(),
            label: "PHP".to_string(),
            markers: php_markers,
        });
    }

    let scala_markers = collect_markers(root, &["build.sbt"]);
    if !scala_markers.is_empty() {
        stacks.push(DetectedStack {
            kind: "scala".to_string(),
            label: "Scala".to_string(),
            markers: scala_markers,
        });
    }

    let zig_markers = collect_markers(root, &["build.zig", "build.zig.zon"]);
    if !zig_markers.is_empty() {
        stacks.push(DetectedStack {
            kind: "zig".to_string(),
            label: "Zig".to_string(),
            markers: zig_markers,
        });
    }

    let cpp_markers = collect_markers(root, &["CMakeLists.txt"]);
    if !cpp_markers.is_empty() {
        stacks.push(DetectedStack {
            kind: "cpp".to_string(),
            label: "C / C++".to_string(),
            markers: cpp_markers,
        });
    }

    let mut perl_markers = collect_markers(root, &["Makefile.PL", "Build.PL", "cpanfile"]);
    if any_ext_in_root(root, "pm") {
        perl_markers.push("*.pm".to_string());
    }
    if !perl_markers.is_empty() {
        stacks.push(DetectedStack {
            kind: "perl".to_string(),
            label: "Perl".to_string(),
            markers: perl_markers,
        });
    }

    let is_kmp = stacks.iter().any(|s| s.kind == "kmp");
    // Distinct top-level stacks excluding KMP itself: android+ios pair under KMP
    // does NOT count as polyglot.
    let distinct_primary = stacks
        .iter()
        .filter(|s| {
            if is_kmp {
                // KMP repos legitimately have android+ios+kmp markers.
                !matches!(s.kind.as_str(), "android" | "ios" | "kmp")
            } else {
                true
            }
        })
        .count();
    let is_polyglot = if is_kmp {
        distinct_primary >= 1 // KMP + at least one extra stack
    } else {
        stacks.len() >= 2
    };

    StackDetection {
        stacks,
        is_kmp,
        is_polyglot,
    }
}

/// Parsed file data for parallel processing
struct ParsedFile {
    rel_path: String,
    root_path: String,
    mtime: i64,
    size: i64,
    symbols: Vec<ParsedSymbol>,
    qualified_names: HashMap<(String, usize, String), String>,
    refs: Vec<ParsedRef>,
}

/// File scheduled by incremental update.
enum PendingUpdateFile {
    Regular {
        root: PathBuf,
        path: PathBuf,
    },
    NodeModulesDts {
        path: PathBuf,
        rel_path: String,
        root_path: String,
    },
}

/// Parse a single file without DB access (thread-safe)
fn parse_file(root: &Path, file_path: &Path) -> Result<ParsedFile> {
    let metadata = fs::metadata(file_path)?;
    let mtime = metadata
        .modified()?
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_secs() as i64;
    let size = metadata.len() as i64;
    let root_path = db::normalize_root_for_storage(root);

    let rel_path = file_path
        .strip_prefix(root)
        .unwrap_or(file_path)
        .to_string_lossy()
        .to_string();

    // Skip files larger than the configured cap (likely generated/minified).
    // Recorded in `files` so `update` still notices on-disk changes, but
    // never read into memory or parsed — that's how a single 200 MB vendor
    // bundle used to push rebuild to 20+ GB RSS.
    if (size as u64) > max_file_size_bytes() {
        return Ok(ParsedFile {
            rel_path,
            root_path,
            mtime,
            size,
            symbols: vec![],
            qualified_names: HashMap::new(),
            refs: vec![],
        });
    }

    let content = fs::read_to_string(file_path)?;

    // Detect file type by extension, with content-based sniffing for .m files
    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let file_type = match if ext == "m" {
        Some(parsers::FileType::detect_m_file_type(&content))
    } else {
        parsers::FileType::from_extension(ext)
    } {
        Some(ft) => ft,
        None => {
            return Ok(ParsedFile {
                rel_path,
                root_path,
                mtime,
                size,
                symbols: vec![],
                qualified_names: HashMap::new(),
                refs: vec![],
            });
        }
    };

    let (mut symbols, refs) = parsers::parse_file_symbols(&content, file_type)?;
    let mut qualified_names = HashMap::new();

    if file_type == parsers::FileType::Cpp {
        qualified_names = parsers::treesitter::cpp::collect_qualified_names(&content)?;
    }

    // BSL (1C:Enterprise) — module names are encoded in directory structure,
    // not in file content. Extract module name from path and emit synthetic symbol.
    if file_type == parsers::FileType::Bsl {
        if let Some(module_name) = parsers::treesitter::bsl::extract_bsl_module_name(&rel_path) {
            symbols.push(parsers::ParsedSymbol {
                name: module_name,
                kind: crate::db::SymbolKind::Package,
                line: 1,
                signature: format!("module {}", rel_path),
                parents: vec![],
            });
        }
    }

    // Vue/Svelte single-file components export an anonymous `export default {}`,
    // so the component itself has no named symbol — its identity is the file
    // name (e.g. NavBar.vue → component `NavBar`). Emit a synthetic symbol so
    // the component is discoverable by name (search/explore/go-to), in addition
    // to the script-block symbols the parser already extracts.
    if matches!(
        file_type,
        parsers::FileType::Vue | parsers::FileType::Svelte
    ) {
        if let Some(stem) = Path::new(&rel_path).file_stem().and_then(|s| s.to_str()) {
            if !symbols.iter().any(|s| s.name == stem && s.line == 1) {
                symbols.push(parsers::ParsedSymbol {
                    name: stem.to_string(),
                    kind: crate::db::SymbolKind::Class,
                    line: 1,
                    signature: format!("component {}", stem),
                    parents: vec![],
                });
            }
        }
    }

    Ok(ParsedFile {
        rel_path,
        root_path,
        mtime,
        size,
        symbols,
        qualified_names,
        refs,
    })
}

/// Directories to always exclude from indexing (regardless of .gitignore).
/// Keep this list to generated caches/build outputs only; ordinary dependency
/// or source directories can be excluded via .gitignore or .ast-index.yaml.
const EXCLUDED_DIRS: &[&str] = &[
    "node_modules",
    "__pycache__",
    ".build",
    "build",
    "dist",
    "target",
    ".gradle",
    ".idea",
    "Pods",
    "DerivedData",
    ".next",
    ".nuxt",
    ".venv",
    "venv",
    ".tox",
    "coverage",
    ".cache",
    // Build system outputs
    "out",
    "bazel-out",
    "bazel-bin",
    "bazel-genfiles",
    "bazel-testlogs",
    "buck-out",
    "_build",
    // IDE / tooling
    ".metals",
    ".bsp",
    ".dart_tool",
    // Temp / generated
    "tmp",
    "temp",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    // Other
    "_site",
    ".turbo",
    ".parcel-cache",
];

/// Check if root has a .git directory/file (false for arc/FUSE mounts)
pub fn has_git_repo(root: &Path) -> bool {
    root.join(".git").exists()
}

/// Find Arc repository root (Yandex Arcadia monorepo).
/// Searches up from root looking for .arc/HEAD, stops at $HOME.
/// Returns the arc repo root path if found.
pub fn find_arc_root(root: &Path) -> Option<PathBuf> {
    let home = dirs::home_dir();
    let mut current = Some(root.to_path_buf());
    while let Some(dir) = current {
        if dir.join(".arc").join("HEAD").exists() {
            return Some(dir);
        }
        // Stop at $HOME to avoid confusing ~/.arc (client storage) with repo marker
        if home.as_ref().map(|h| h == &dir).unwrap_or(false) {
            break;
        }
        current = dir.parent().map(|p| p.to_path_buf());
    }
    None
}

/// Check if root is inside an Arc repository
pub fn has_arc_repo(root: &Path) -> bool {
    find_arc_root(root).is_some()
}

/// Quickly count source files in a directory, stopping at `limit`.
/// Returns the count (capped at `limit`) — avoids full traversal for large dirs.
/// Quick file count for auto-detection threshold.
/// Intentionally skips arc/gitignore checks — this is just a rough estimate,
/// and stat-ing .gitignore on every dir is too slow on FUSE mounts.
pub fn quick_file_count(root: &Path, no_ignore: bool, limit: usize) -> usize {
    use ignore::WalkBuilder;

    let use_git = has_git_repo(root) && !no_ignore;
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(true)
        .follow_links(false)
        .max_depth(Some(50))
        .git_ignore(use_git)
        .git_exclude(use_git)
        .filter_entry(|entry| !is_excluded_dir(entry));
    // No arc ignore here — quick_file_count is just a rough estimate,
    // and add_custom_ignore_filename causes stat per directory (slow on FUSE)

    let mut count = 0;
    for entry in builder.build().filter_map(|e| e.ok()) {
        if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
            if parsers::is_supported_extension(ext) {
                count += 1;
                if count >= limit {
                    return count;
                }
            }
        }
    }
    count
}

/// Check if a path component matches an excluded directory
pub fn is_excluded_dir(entry: &ignore::DirEntry) -> bool {
    if !entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
        return false;
    }
    if let Some(name) = entry.path().file_name().and_then(|n| n.to_str()) {
        EXCLUDED_DIRS.contains(&name)
    } else {
        false
    }
}

/// Module-related file names to collect during directory walk
fn is_module_file(name: &str) -> bool {
    name == "build.gradle"
        || name == "build.gradle.kts"
        || name == "Package.swift"
        || name.ends_with(".pm")
        || name == "pom.xml"
        || name == "pyproject.toml"
        || name == "setup.py"
        || name == "setup.cfg"
        || name == "ya.make"
}

fn sample_parseable_files_without_ignore(walk_dir: &Path, limit: usize) -> Vec<PathBuf> {
    use ignore::WalkBuilder;

    let mut builder = WalkBuilder::new(walk_dir);
    builder
        .hidden(true)
        .follow_links(false)
        .max_depth(Some(50))
        .git_ignore(false)
        .git_exclude(false)
        .filter_entry(|entry| !is_excluded_dir(entry));

    let mut files = Vec::new();
    for entry in builder.build().filter_map(|e| e.ok()) {
        let path = entry.path();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if parsers::is_supported_extension(ext) {
                files.push(path.to_path_buf());
                if files.len() >= limit {
                    break;
                }
            }
        }
    }
    files
}

/// Result of the filesystem walk in index_directory.
/// Collects all interesting paths in a single walk to avoid redundant traversals.
pub struct WalkResult {
    pub file_count: usize,
    pub module_files: Vec<PathBuf>,
    // iOS
    pub storyboard_files: Vec<PathBuf>, // .storyboard, .xib
    pub xcassets_dirs: Vec<PathBuf>,    // .xcassets directories
    // Android
    pub xml_layout_files: Vec<PathBuf>, // .xml in /res/(layout|menu|navigation)
    pub res_files: Vec<PathBuf>,        // all files under /res/
    /// True when the walker aborted because `AST_INDEX_MAX_FILES` was hit.
    /// The caller should surface a clear error with bypass instructions
    /// instead of pretending the partial result is complete.
    pub aborted_by_cap: bool,
}

#[derive(Clone, Copy)]
enum WriteMode {
    FreshRebuild,
    ReplaceExisting,
}

#[derive(Debug, Default)]
struct WalkErrorSummary {
    count: usize,
    samples: Vec<String>,
}

impl WalkErrorSummary {
    const MAX_SAMPLES: usize = 5;

    fn record(&mut self, err: ignore::Error) {
        self.record_message(err.to_string());
    }

    fn record_message(&mut self, message: String) {
        self.count += 1;
        if self.samples.len() < Self::MAX_SAMPLES {
            self.samples.push(message);
        }
    }

    fn finish(&self, walk_dir: &Path, source_files: usize, progress: bool, verbose: bool) {
        if self.count == 0 {
            return;
        }

        let should_log = progress || source_files == 0;
        if !should_log {
            return;
        }

        if source_files == 0 {
            eprintln!(
                "Warning: filesystem walk under {} hit {} error(s) and found 0 parseable files. The index may be incomplete.",
                walk_dir.display(),
                self.count
            );
        } else {
            eprintln!(
                "Warning: skipped {} filesystem entr{} due to walk errors while indexing {}",
                self.count,
                if self.count == 1 { "y" } else { "ies" },
                walk_dir.display()
            );
        }

        if verbose {
            for sample in &self.samples {
                eprintln!("[verbose] walk error: {}", sample);
            }
        } else if let Some(sample) = self.samples.first() {
            eprintln!("First walk error: {}", sample);
            eprintln!("Run with --verbose to show more walk errors.");
        }
    }

    fn merge_from(&mut self, other: Self) {
        self.count += other.count;
        let remaining = Self::MAX_SAMPLES.saturating_sub(self.samples.len());
        self.samples
            .extend(other.samples.into_iter().take(remaining));
    }
}

#[derive(Default)]
struct CollectedWalkData {
    files: Vec<PathBuf>,
    module_files: Vec<PathBuf>,
    storyboard_files: Vec<PathBuf>,
    xcassets_dirs: Vec<PathBuf>,
    xml_layout_files: Vec<PathBuf>,
    res_files: Vec<PathBuf>,
    walk_errors: WalkErrorSummary,
}

/// Canonical Android resource directory prefixes under `/res/`.
///
/// Used to (1) keep `res_files` Android-specific instead of grabbing any
/// `/res/` substring (a Python project's `assets/res/` would otherwise leak
/// in and falsely mark the tree as Android), and (2) narrow `xml_layout_files`
/// to actual layout/menu/navigation directories without matching `values-pl/
/// layout_attrs.xml`.
const ANDROID_RES_SUBDIRS: &[&str] = &[
    "values",
    "layout",
    "drawable",
    "menu",
    "navigation",
    "mipmap",
    "anim",
    "animator",
    "color",
    "font",
    "interpolator",
    "raw",
    "transition",
    "xml",
];

/// `path_str` is under one of `<name>` or `<name>-<qualifier>` subdirs of `/res/`.
fn android_res_subdir_match(path_str: &str, name: &str) -> bool {
    let needle_slash = format!("/res/{}/", name);
    let needle_dash = format!("/res/{}-", name);
    path_str.contains(&needle_slash) || path_str.contains(&needle_dash)
}

fn is_android_res_path(path_str: &str) -> bool {
    ANDROID_RES_SUBDIRS
        .iter()
        .any(|name| android_res_subdir_match(path_str, name))
}

fn is_android_layout_path(path_str: &str) -> bool {
    ["layout", "menu", "navigation"]
        .iter()
        .any(|name| android_res_subdir_match(path_str, name))
}

fn collect_walk_entry(data: &mut CollectedWalkData, entry: &ignore::DirEntry) {
    let path = entry.path();
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if is_module_file(name) {
            data.module_files.push(path.to_path_buf());
        }
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if parsers::is_supported_extension(ext) {
            data.files.push(path.to_path_buf());
        }
        if ext == "storyboard" || ext == "xib" {
            data.storyboard_files.push(path.to_path_buf());
        }
        if ext == "xcassets" && path.is_dir() {
            data.xcassets_dirs.push(path.to_path_buf());
        }
        let path_str = path.to_string_lossy();
        if is_android_res_path(&path_str) {
            data.res_files.push(path.to_path_buf());
            if ext == "xml" && is_android_layout_path(&path_str) {
                data.xml_layout_files.push(path.to_path_buf());
            }
        }
    }
}

struct ParallelWalkCollectorBuilder {
    shared: Arc<Mutex<CollectedWalkData>>,
    entries_seen: Arc<AtomicUsize>,
    verbose: bool,
    walk_start: std::time::Instant,
    max_files: usize,
    warn_threshold: usize,
    aborted: Arc<std::sync::atomic::AtomicBool>,
    warned: Arc<std::sync::atomic::AtomicBool>,
}

struct ParallelWalkCollector {
    shared: Arc<Mutex<CollectedWalkData>>,
    entries_seen: Arc<AtomicUsize>,
    verbose: bool,
    walk_start: std::time::Instant,
    max_files: usize,
    warn_threshold: usize,
    aborted: Arc<std::sync::atomic::AtomicBool>,
    warned: Arc<std::sync::atomic::AtomicBool>,
    local: CollectedWalkData,
}

impl<'s> ignore::ParallelVisitorBuilder<'s> for ParallelWalkCollectorBuilder {
    fn build(&mut self) -> Box<dyn ignore::ParallelVisitor + 's> {
        Box::new(ParallelWalkCollector {
            shared: self.shared.clone(),
            entries_seen: self.entries_seen.clone(),
            verbose: self.verbose,
            walk_start: self.walk_start,
            max_files: self.max_files,
            warn_threshold: self.warn_threshold,
            aborted: self.aborted.clone(),
            warned: self.warned.clone(),
            local: CollectedWalkData::default(),
        })
    }
}

fn emit_soft_warning(seen: usize, walk_start: std::time::Instant) {
    eprintln!(
        "[ast-index] warning: scanning a very large root ({}+ candidate files in {:?}). \
         Indexing will continue, but rebuild may take a while and consume substantial \
         memory. Consider scoping via .ast-index.yaml `include` or running from a deeper \
         subdirectory. The hard abort cap is AST_INDEX_MAX_FILES (default 2,000,000).",
        seen,
        walk_start.elapsed()
    );
}

impl ignore::ParallelVisitor for ParallelWalkCollector {
    fn visit(&mut self, entry: Result<ignore::DirEntry, ignore::Error>) -> ignore::WalkState {
        // Cooperative early-stop when another worker has tripped the cap.
        if self.max_files > 0 && self.aborted.load(std::sync::atomic::Ordering::Relaxed) {
            return ignore::WalkState::Quit;
        }
        match entry {
            Ok(entry) => {
                let seen = self.entries_seen.fetch_add(1, Ordering::Relaxed) + 1;
                if self.verbose && seen % 10000 == 0 {
                    eprintln!(
                        "[verbose] walk: {} entries scanned in {:?}...",
                        seen,
                        self.walk_start.elapsed()
                    );
                }
                if self.warn_threshold > 0
                    && seen > self.warn_threshold
                    && !self.warned.swap(true, std::sync::atomic::Ordering::Relaxed)
                {
                    emit_soft_warning(seen, self.walk_start);
                }
                if self.max_files > 0 && seen > self.max_files {
                    self.aborted
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    return ignore::WalkState::Quit;
                }
                collect_walk_entry(&mut self.local, &entry);
            }
            Err(err) => self.local.walk_errors.record(err),
        }
        ignore::WalkState::Continue
    }
}

impl Drop for ParallelWalkCollector {
    fn drop(&mut self) {
        if self.local.files.is_empty()
            && self.local.module_files.is_empty()
            && self.local.storyboard_files.is_empty()
            && self.local.xcassets_dirs.is_empty()
            && self.local.xml_layout_files.is_empty()
            && self.local.res_files.is_empty()
            && self.local.walk_errors.count == 0
        {
            return;
        }

        let mut shared = self.shared.lock().unwrap();
        shared.files.append(&mut self.local.files);
        shared.module_files.append(&mut self.local.module_files);
        shared
            .storyboard_files
            .append(&mut self.local.storyboard_files);
        shared.xcassets_dirs.append(&mut self.local.xcassets_dirs);
        shared
            .xml_layout_files
            .append(&mut self.local.xml_layout_files);
        shared.res_files.append(&mut self.local.res_files);
        let local_errors = std::mem::take(&mut self.local.walk_errors);
        shared.walk_errors.merge_from(local_errors);
    }
}

pub fn index_directory(
    conn: &mut Connection,
    root: &Path,
    progress: bool,
    no_ignore: bool,
) -> Result<WalkResult> {
    index_directory_scoped(conn, root, root, progress, no_ignore, None)
}

pub fn index_directory_with_config(
    conn: &mut Connection,
    root: &Path,
    progress: bool,
    no_ignore: bool,
    extra_exclude: Option<&[String]>,
) -> Result<WalkResult> {
    index_directory_scoped(conn, root, root, progress, no_ignore, extra_exclude)
}

/// Index only direct entries under `root`.
///
/// Sub-project rebuild mode walks each child project separately. This helper
/// preserves root-level files and module markers without recursively walking
/// the same child trees again.
pub fn index_directory_direct_entries(
    conn: &mut Connection,
    root: &Path,
    progress: bool,
    no_ignore: bool,
    extra_exclude: Option<&[String]>,
) -> Result<WalkResult> {
    index_directory_scoped_with_max_depth(
        conn,
        root,
        root,
        progress,
        no_ignore,
        extra_exclude,
        Some(1),
    )
}

/// Index a directory, walking `walk_dir` but storing paths relative to `root`.
/// When walk_dir == root, behaves identically to index_directory.
/// When walk_dir is a subdirectory of root, only indexes that subdirectory.
/// `extra_exclude` — additional directory names to skip (from .ast-index.yaml config).
pub fn index_directory_scoped(
    conn: &mut Connection,
    root: &Path,
    walk_dir: &Path,
    progress: bool,
    no_ignore: bool,
    extra_exclude: Option<&[String]>,
) -> Result<WalkResult> {
    index_directory_scoped_with_max_depth(
        conn,
        root,
        walk_dir,
        progress,
        no_ignore,
        extra_exclude,
        Some(50),
    )
}

fn index_directory_scoped_with_max_depth(
    conn: &mut Connection,
    root: &Path,
    walk_dir: &Path,
    progress: bool,
    no_ignore: bool,
    extra_exclude: Option<&[String]>,
    max_depth: Option<usize>,
) -> Result<WalkResult> {
    use ignore::WalkBuilder;
    use std::time::Instant;

    let verbose = std::env::var("AST_INDEX_VERBOSE").is_ok();
    let experimental_parallel_walk = std::env::var("AST_INDEX_EXPERIMENTAL_PARALLEL_WALK").is_ok();

    // Collect all file paths (paths are lightweight, OK to keep in memory)
    if verbose {
        eprintln!(
            "[verbose] checking git repo: walk_dir={}",
            walk_dir.display()
        );
    }
    let t = Instant::now();
    let use_git = has_git_repo(walk_dir) || has_git_repo(root);
    let use_git = use_git && !no_ignore;
    if verbose {
        eprintln!("[verbose] has_git_repo: {} in {:?}", use_git, t.elapsed());
    }

    let t = Instant::now();
    let arc_root = if no_ignore {
        None
    } else {
        find_arc_root(walk_dir).or_else(|| find_arc_root(root))
    };
    if verbose {
        eprintln!(
            "[verbose] find_arc_root: {:?} in {:?}",
            arc_root.as_ref().map(|p| p.display().to_string()),
            t.elapsed()
        );
    }

    // Build gitignore-style exclude matcher from config patterns.
    // Full gitignore semantics: *, **, ?, [abc], leading / anchors to walk_dir, trailing / = dirs only.
    let exclude_matcher: Option<ignore::gitignore::Gitignore> = {
        let patterns = extra_exclude.unwrap_or(&[]);
        if patterns.is_empty() {
            None
        } else {
            let mut gb = ignore::gitignore::GitignoreBuilder::new(walk_dir);
            for p in patterns {
                gb.add_line(None, p).ok();
            }
            gb.build().ok()
        }
    };

    let mut builder = WalkBuilder::new(walk_dir);
    builder
        .hidden(true)
        .follow_links(false) // Never follow symlinks — prevents loops in monorepos
        .max_depth(max_depth) // Prevent runaway traversal in deeply nested structures
        .git_ignore(use_git) // Respect .gitignore only if .git exists
        .git_exclude(use_git)
        .filter_entry(move |entry| {
            if is_excluded_dir(entry) {
                return false;
            }
            if let Some(ref matcher) = exclude_matcher {
                let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                if matcher.matched(entry.path(), is_dir).is_ignore() {
                    return false;
                }
            }
            true
        });
    // Arc repos: respect .gitignore and .arcignore without .git directory
    if let Some(ref arc) = arc_root {
        if verbose {
            eprintln!("[verbose] arc mode: adding .gitignore + .arcignore custom ignore filenames");
        }
        builder.add_custom_ignore_filename(".gitignore");
        builder.add_custom_ignore_filename(".arcignore");
        // Add root .gitignore from arc repo root (may be above walk root)
        let root_gitignore = arc.join(".gitignore");
        if root_gitignore.exists() {
            if verbose {
                eprintln!(
                    "[verbose] adding root .gitignore: {}",
                    root_gitignore.display()
                );
            }
            builder.add_ignore(root_gitignore);
        }
    }

    // Thread count: --threads flag > AST_INDEX_THREADS env > CPU cores (max 8 for local, higher for network FS)
    let num_threads = effective_num_threads();

    if verbose {
        eprintln!("[verbose] starting file walk...");
    }
    let walk_start = Instant::now();
    let mut collected = CollectedWalkData::default();

    let max_files = if cap_disabled_for_root(conn) {
        0
    } else {
        max_files_cap()
    };
    // The soft warning fires regardless of the hard cap bypass — even users
    // who opt into indexing the whole monorepo deserve a heads-up that the
    // run is going to be heavy.
    let warn_threshold = warn_files_threshold();
    let aborted_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let warned_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let walk_entries = if experimental_parallel_walk {
        builder.threads(num_threads);
        let shared = Arc::new(Mutex::new(CollectedWalkData::default()));
        let entries_seen = Arc::new(AtomicUsize::new(0));
        let mut collector = ParallelWalkCollectorBuilder {
            shared: shared.clone(),
            entries_seen: entries_seen.clone(),
            verbose,
            walk_start,
            max_files,
            warn_threshold,
            aborted: aborted_flag.clone(),
            warned: warned_flag.clone(),
        };
        builder.build_parallel().visit(&mut collector);
        let mut shared = shared.lock().unwrap();
        collected = std::mem::take(&mut *shared);
        entries_seen.load(Ordering::Relaxed)
    } else {
        let walker = builder.build();
        let mut walk_entries = 0usize;
        for entry in walker {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    collected.walk_errors.record(err);
                    continue;
                }
            };
            walk_entries += 1;
            if verbose && walk_entries % 10000 == 0 {
                eprintln!(
                    "[verbose] walk: {} entries scanned in {:?}...",
                    walk_entries,
                    walk_start.elapsed()
                );
            }
            if warn_threshold > 0
                && walk_entries > warn_threshold
                && !warned_flag.swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                emit_soft_warning(walk_entries, walk_start);
            }
            if max_files > 0 && walk_entries > max_files {
                aborted_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                break;
            }
            collect_walk_entry(&mut collected, &entry);
        }
        walk_entries
    };
    let aborted_by_cap = aborted_flag.load(std::sync::atomic::Ordering::Relaxed);

    if aborted_by_cap {
        return Err(anyhow::anyhow!(
            "walker stopped after {} candidate files (configurable cap).\n\
             \n\
             ast-index is tuned for a project subtree, not for a monorepo /\n\
             VCS root. Re-run from a narrower subdirectory, or override:\n\
             \n  ast-index rebuild --force\n\
                 index this root anyway for one run (slow, may use a lot of memory)\n\
             \n  ast-index rebuild --force --remember\n\
                 same, but persist the opt-in for this project — subsequent\n\
                 `ast-index rebuild` runs no longer hit the cap\n\
             \n  ast-index rebuild --max-files 5000000\n\
                 raise the cap explicitly for one run\n\
             \n\
             The cap also respects AST_INDEX_MAX_FILES (set to 0 to disable).",
            max_files
        ));
    }

    let files = collected.files;
    let module_files = collected.module_files;
    let storyboard_files = collected.storyboard_files;
    let xcassets_dirs = collected.xcassets_dirs;
    let xml_layout_files = collected.xml_layout_files;
    let res_files = collected.res_files;
    let walk_errors = collected.walk_errors;

    if verbose {
        eprintln!(
            "[verbose] walk complete: {} total entries, {} source files, {} module files in {:?}",
            walk_entries,
            files.len(),
            module_files.len(),
            walk_start.elapsed()
        );
    }

    walk_errors.finish(walk_dir, files.len(), progress, verbose);

    if progress && files.is_empty() && !no_ignore {
        let visible_without_ignore = sample_parseable_files_without_ignore(walk_dir, 5);
        if !visible_without_ignore.is_empty() {
            eprintln!(
                "Warning: ignore rules filtered out all parseable source files under {}.",
                walk_dir.display()
            );
            eprintln!("Try `ast-index rebuild --no-ignore` to confirm.");
            if arc_root.is_some() {
                eprintln!(
                    "Note: in Arc mode ast-index also loads `.gitignore` from the repo root."
                );
            }
            eprintln!("Example files visible without ignore rules:");
            for path in &visible_without_ignore {
                let display = path.strip_prefix(root).unwrap_or(path);
                eprintln!("  - {}", display.display());
            }
        }
    }

    let total_files = files.len();
    let chunk_size = effective_chunk_size(total_files);
    if progress {
        eprintln!("Found {} files to parse...", total_files);
    }

    let mut total_count = 0;
    let parsed_global = Arc::new(AtomicUsize::new(0));
    if verbose {
        eprintln!("[verbose] using {} threads for parsing", num_threads);
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .stack_size(RAYON_WORKER_STACK_SIZE)
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build thread pool: {}", e))?;

    let root_buf = root.to_path_buf();
    let total_chunks = (files.len() + chunk_size - 1) / chunk_size;
    for (chunk_idx, chunk) in files.chunks(chunk_size).enumerate() {
        let root_clone = root_buf.clone();
        let counter = parsed_global.clone();
        let total = total_files;

        if verbose {
            eprintln!(
                "[verbose] chunk {}/{}: parsing {} files...",
                chunk_idx + 1,
                total_chunks,
                chunk.len()
            );
        }
        let chunk_start = Instant::now();

        // Parse chunk in parallel — at most `chunk_size` ParsedFiles in memory
        let parsed_files: Vec<ParsedFile> = pool.install(|| {
            chunk
                .par_iter()
                .filter_map(|path| {
                    let result = parse_file(&root_clone, path).ok();
                    let c = counter.fetch_add(1, Ordering::Relaxed) + 1;
                    if progress && c % 2000 == 0 {
                        eprintln!("Parsed {} / {} files...", c, total);
                    }
                    result
                })
                .collect()
        });

        if verbose {
            eprintln!(
                "[verbose] chunk {}/{}: parsed in {:?}, writing {} to DB...",
                chunk_idx + 1,
                total_chunks,
                chunk_start.elapsed(),
                parsed_files.len()
            );
        }
        let write_start = Instant::now();

        // Write to DB and free parsed_files
        write_batch_to_db(
            conn,
            parsed_files,
            &mut total_count,
            WriteMode::FreshRebuild,
        )?;

        if verbose {
            eprintln!(
                "[verbose] chunk {}/{}: written in {:?}",
                chunk_idx + 1,
                total_chunks,
                write_start.elapsed()
            );
        }

        if progress {
            eprintln!("Written {} / {} files to DB", total_count, total_files);
        }
    }

    Ok(WalkResult {
        file_count: total_count,
        module_files,
        storyboard_files,
        xcassets_dirs,
        xml_layout_files,
        res_files,
        aborted_by_cap: false,
    })
}

/// Write a batch of parsed files to DB in a single transaction
fn write_batch_to_db(
    conn: &mut Connection,
    batch: Vec<ParsedFile>,
    total_count: &mut usize,
    mode: WriteMode,
) -> Result<()> {
    let tx = conn.transaction()?;

    {
        let file_sql = match mode {
            WriteMode::FreshRebuild => {
                "INSERT INTO files (path, root_path, mtime, size) VALUES (?1, ?2, ?3, ?4)"
            }
            WriteMode::ReplaceExisting => {
                "INSERT OR REPLACE INTO files (path, root_path, mtime, size) VALUES (?1, ?2, ?3, ?4)"
            }
        };
        let mut file_stmt = tx.prepare_cached(file_sql)?;
        let mut sym_stmt = tx.prepare_cached(
            "INSERT INTO symbols (file_id, name, qualified_name, kind, line, signature) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        )?;
        let mut inh_stmt = tx.prepare_cached(
            "INSERT INTO inheritance (child_id, parent_name, kind) VALUES (?1, ?2, ?3)",
        )?;
        let mut ref_stmt = tx.prepare_cached(
            "INSERT INTO refs (file_id, name, line, context) VALUES (?1, ?2, ?3, ?4)",
        )?;

        for pf in batch {
            let ParsedFile {
                rel_path,
                root_path,
                mtime,
                size,
                symbols,
                qualified_names,
                refs,
            } = pf;

            file_stmt.execute(rusqlite::params![rel_path, root_path, mtime, size])?;
            let file_id = tx.last_insert_rowid();
            // `INSERT OR REPLACE` on `files.path` drops the previous file row first, and
            // `ON DELETE CASCADE` clears old symbols/refs automatically. Explicit deletes
            // here only add extra work, especially during full rebuilds on a fresh DB.

            for sym in symbols {
                let qualified_name = qualified_names.get(&(
                    sym.kind.as_str().to_string(),
                    sym.line,
                    sym.name.clone(),
                ));
                sym_stmt.execute(rusqlite::params![
                    file_id,
                    sym.name,
                    qualified_name,
                    sym.kind.as_str(),
                    sym.line as i64,
                    parsers::truncate_signature(&sym.signature)
                ])?;
                let symbol_id = tx.last_insert_rowid();

                for (parent_name, inherit_kind) in sym.parents {
                    inh_stmt.execute(rusqlite::params![symbol_id, parent_name, inherit_kind])?;
                }
            }

            for r in refs {
                ref_stmt.execute(rusqlite::params![file_id, r.name, r.line as i64, r.context])?;
            }

            *total_count += 1;
        }
    }

    tx.commit()?;
    Ok(())
}

/// Incremental update: only re-index changed/new files, delete removed files.
///
/// Walks the primary root AND every extra_root registered in metadata. Each
/// root's files are stored with paths relative to that root (matching how
/// `rebuild` indexed them), so reconciliation against the DB works correctly
/// for extra_roots — without this, extra-root files were seen as "missing"
/// during the primary walk and deleted on every `update`.
///
/// `include` — optional allow-list (as in `.ast-index.yaml`). When set, the
/// primary root is replaced with the listed sub-paths; everything else under
/// `root` is skipped. Paths in the DB stay relative to the outer `root` so
/// they match what `rebuild` wrote. extra_roots are walked unconditionally.
///
/// `exclude_matcher` — optional gitignore-style matcher applied to every
/// walked entry, mirroring the rebuild path so update doesn't re-index dirs
/// that rebuild deliberately skipped.
pub fn update_directory_incremental(
    conn: &mut Connection,
    root: &Path,
    progress: bool,
    include: Option<&[String]>,
    exclude_matcher: Option<&ignore::gitignore::Gitignore>,
) -> Result<(usize, usize, usize)> {
    use ignore::WalkBuilder;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // 1. Load existing files from DB with their mtime and size.
    let mut existing_files: HashMap<(String, String), (i64, i64, i64)> = HashMap::new(); // (root_path, path) -> (file_id, mtime, size)
    {
        let mut stmt = conn.prepare("SELECT id, root_path, path, mtime, size FROM files")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
        for row in rows {
            let (id, root_path, path, mtime, size) = row?;
            existing_files.insert((root_path, path), (id, mtime, size));
        }
    }

    if progress {
        eprintln!("Loaded {} files from index", existing_files.len());
    }

    // 2. Build the list of (walk_dir, path_anchor) pairs. `path_anchor` is the
    //    base used for `strip_prefix` when computing rel_path — keeping it equal
    //    to the outer root for include sub-paths means the DB stays consistent
    //    with what `rebuild` wrote (paths are relative to the project root, not
    //    the sub-include). extra_roots are anchored to themselves.
    let mut walk_specs: Vec<(PathBuf, PathBuf)> = Vec::new();
    match include {
        Some(inc) if !inc.is_empty() => {
            for entry in inc {
                let walk_dir = root.join(entry);
                if walk_dir.is_dir() {
                    walk_specs.push((walk_dir, root.to_path_buf()));
                } else if progress {
                    eprintln!("Skipping missing include path: {}", walk_dir.display());
                }
            }
        }
        _ => {
            walk_specs.push((root.to_path_buf(), root.to_path_buf()));
        }
    }
    for e in db::get_extra_roots(conn)? {
        let p = PathBuf::from(&e);
        if p.exists() {
            walk_specs.push((p.clone(), p));
        } else if progress {
            eprintln!("Skipping missing extra root: {}", e);
        }
    }

    // 3. Walk each (walk_dir, anchor) pair and categorize its files. Paths are
    //    stored relative to `anchor`, matching `index_directory_scoped`'s scheme.
    let mut files_to_parse: Vec<PendingUpdateFile> = Vec::new();
    let mut current_paths: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();

    for (walk_dir, anchor) in &walk_specs {
        let is_git = has_git_repo(walk_dir) || has_git_repo(anchor);
        let arc_root = find_arc_root(walk_dir).or_else(|| find_arc_root(anchor));
        let mut builder = WalkBuilder::new(walk_dir);
        let exclude_matcher_owned = exclude_matcher.cloned();
        builder
            .hidden(true)
            .git_ignore(is_git)
            .filter_entry(move |entry| {
                if is_excluded_dir(entry) {
                    return false;
                }
                if let Some(ref m) = exclude_matcher_owned {
                    let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                    if m.matched(entry.path(), is_dir).is_ignore() {
                        return false;
                    }
                }
                true
            });
        if let Some(ref arc) = arc_root {
            builder.add_custom_ignore_filename(".gitignore");
            builder.add_custom_ignore_filename(".arcignore");
            let root_gitignore = arc.join(".gitignore");
            if root_gitignore.exists() {
                builder.add_ignore(root_gitignore);
            }
        }
        let walker = builder.build();

        for entry in walker.filter_map(|e| e.ok()) {
            let is_supported = entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(parsers::is_supported_extension)
                .unwrap_or(false);
            if !is_supported {
                continue;
            }

            let file_path = entry.path().to_path_buf();
            let rel_path = file_path
                .strip_prefix(anchor)
                .unwrap_or(&file_path)
                .to_string_lossy()
                .to_string();
            let root_key = db::normalize_root_for_storage(anchor);

            let (file_mtime, file_size) = fs::metadata(&file_path)
                .ok()
                .map(|metadata| {
                    let mtime = metadata
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    (mtime, metadata.len() as i64)
                })
                .unwrap_or((0, 0));

            let need_parse = match existing_files.get(&(root_key.clone(), rel_path.clone())) {
                Some((_, db_mtime, db_size)) => file_mtime != *db_mtime || file_size != *db_size,
                None => true,
            };

            if need_parse {
                files_to_parse.push(PendingUpdateFile::Regular {
                    root: anchor.clone(),
                    path: file_path,
                });
            }
            current_paths.insert((root_key, rel_path));
        }
    }

    let root_key = db::normalize_root_for_storage(root);
    for (file_path, rel_path) in collect_node_modules_dts_files(root) {
        let (file_mtime, file_size) = fs::metadata(&file_path)
            .ok()
            .map(|metadata| {
                let mtime = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                (mtime, metadata.len() as i64)
            })
            .unwrap_or((0, 0));

        let need_parse = match existing_files.get(&(root_key.clone(), rel_path.clone())) {
            Some((_, db_mtime, db_size)) => file_mtime != *db_mtime || file_size != *db_size,
            None => true,
        };

        if need_parse {
            files_to_parse.push(PendingUpdateFile::NodeModulesDts {
                path: file_path,
                rel_path: rel_path.clone(),
                root_path: root_key.clone(),
            });
        }
        current_paths.insert((root_key.clone(), rel_path));
    }

    // 4. Find deleted files
    let deleted_paths: Vec<(String, String)> = existing_files
        .keys()
        .filter(|p| !current_paths.contains(*p))
        .cloned()
        .collect();

    if progress {
        eprintln!(
            "Found {} new/changed files, {} deleted files",
            files_to_parse.len(),
            deleted_paths.len()
        );
    }

    let was_dirty = db::has_index_update_dirty(conn)?;
    let has_planned_mutations = !files_to_parse.is_empty() || !deleted_paths.is_empty();
    if has_planned_mutations {
        db::mark_index_update_dirty(conn)?;
    }

    // 5. Delete removed files from DB
    if !deleted_paths.is_empty() {
        let tx = conn.transaction()?;
        {
            let mut del_file_stmt =
                tx.prepare_cached("DELETE FROM files WHERE root_path = ?1 AND path = ?2")?;
            for (root_path, path) in &deleted_paths {
                del_file_stmt.execute(rusqlite::params![root_path, path])?;
            }
        }
        tx.commit()?;
    }

    // 6. Parse and update changed/new files
    //    Thread count: AST_INDEX_THREADS env > 32 (high default — update on
    //    monorepos benefits from higher parallelism than the cautious rebuild
    //    default; per-file parsing is CPU-bound and the I/O is mostly cached
    //    after the walker has already touched the inodes).
    let updated_count = if !files_to_parse.is_empty() {
        let total_files = files_to_parse.len();
        let parsed_count = Arc::new(AtomicUsize::new(0));
        let parsed_count_clone = parsed_count.clone();

        let num_threads = std::env::var("AST_INDEX_THREADS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or_else(|| effective_num_threads().max(16));
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .stack_size(RAYON_WORKER_STACK_SIZE)
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build thread pool: {}", e))?;

        let parsed_files: Vec<ParsedFile> = pool.install(|| {
            files_to_parse
                .par_iter()
                .filter_map(|pending| {
                    let result = match pending {
                        PendingUpdateFile::Regular { root, path } => parse_file(root, path),
                        PendingUpdateFile::NodeModulesDts {
                            path,
                            rel_path,
                            root_path,
                        } => parse_dts_file(path, rel_path, root_path),
                    }
                    .ok();
                    let c = parsed_count_clone.fetch_add(1, Ordering::Relaxed) + 1;
                    if progress && c % 500 == 0 {
                        eprintln!("Parsed {} / {} changed files...", c, total_files);
                    }
                    result
                })
                .collect()
        });

        let count = parsed_files.len();
        let mut dummy_total = 0;
        write_batch_to_db(
            conn,
            parsed_files,
            &mut dummy_total,
            WriteMode::ReplaceExisting,
        )?;
        count
    } else {
        0
    };

    let all_planned_files_written = updated_count == files_to_parse.len();
    if all_planned_files_written && (has_planned_mutations || was_dirty) {
        db::complete_index_update(conn)?;
    }

    Ok((updated_count, files_to_parse.len(), deleted_paths.len()))
}

/// Index modules from build.gradle files (Android) and Package.swift (iOS)
pub fn index_modules(conn: &Connection, root: &Path) -> Result<usize> {
    use ignore::WalkBuilder;

    let is_git = has_git_repo(root);
    let arc_root = find_arc_root(root);
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(true)
        .git_ignore(is_git)
        .filter_entry(|entry| !is_excluded_dir(entry));
    if let Some(ref arc) = arc_root {
        builder.add_custom_ignore_filename(".gitignore");
        builder.add_custom_ignore_filename(".arcignore");
        let root_gitignore = arc.join(".gitignore");
        if root_gitignore.exists() {
            builder.add_ignore(root_gitignore);
        }
    }
    let walker = builder.build();

    let files: Vec<PathBuf> = walker
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .map(is_module_file)
                .unwrap_or(false)
        })
        .map(|e| e.path().to_path_buf())
        .collect();

    index_modules_from_files(conn, root, &files)
}

/// Index modules from a pre-collected list of module files (avoids re-walking the filesystem)
pub fn index_modules_from_files(
    conn: &Connection,
    root: &Path,
    files: &[PathBuf],
) -> Result<usize> {
    let mut count = 0;

    // Regex to extract SPM targets from Package.swift
    static SPM_TARGET_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"\.(?:target|testTarget|binaryTarget)\s*\(\s*name:\s*["']([^"']+)["']"#)
            .unwrap()
    });

    let spm_target_re = &*SPM_TARGET_RE;

    // Outer repository root — used to normalize ya.make module paths so they match PEERDIR
    // entries, which are written relative to the outer repo root, not the rebuild root.
    let mono_root = find_arc_root(root);

    for path in files {
        if let Some(name) = path.file_name() {
            let name_str = name.to_string_lossy();

            // Android/Gradle modules
            if name_str == "build.gradle" || name_str == "build.gradle.kts" {
                if let Some(parent) = path.parent() {
                    let module_path = parent
                        .strip_prefix(root)
                        .unwrap_or(parent)
                        .to_string_lossy()
                        .to_string();

                    // Convert path to module name (e.g., features/payments/api -> features.payments.api)
                    let module_name = module_path.replace('/', ".");

                    if !module_name.is_empty() {
                        conn.execute(
                            "INSERT OR IGNORE INTO modules (name, path) VALUES (?1, ?2)",
                            rusqlite::params![module_name, module_path],
                        )?;
                        count += 1;
                    }
                }
            }

            // iOS/SPM modules (Package.swift)
            if name_str == "Package.swift" {
                if let Some(parent) = path.parent() {
                    let package_path = parent
                        .strip_prefix(root)
                        .unwrap_or(parent)
                        .to_string_lossy()
                        .to_string();

                    // Read Package.swift and extract targets
                    if let Ok(content) = fs::read_to_string(path) {
                        for caps in spm_target_re.captures_iter(&content) {
                            let target_name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                            if !target_name.is_empty() {
                                let module_name = if package_path.is_empty() {
                                    target_name.to_string()
                                } else {
                                    format!("{}.{}", package_path.replace('/', "."), target_name)
                                };
                                let module_path = if package_path.is_empty() {
                                    target_name.to_string()
                                } else {
                                    format!("{}/{}", package_path, target_name)
                                };

                                conn.execute(
                                    "INSERT OR IGNORE INTO modules (name, path) VALUES (?1, ?2)",
                                    rusqlite::params![module_name, module_path],
                                )?;
                                count += 1;
                            }
                        }
                    }
                }
            }

            // Perl modules (.pm files with package declarations)
            if name_str.ends_with(".pm") {
                if let Ok(content) = fs::read_to_string(path) {
                    static PERL_PACKAGE_RE: LazyLock<Regex> = LazyLock::new(|| {
                        Regex::new(r"^\s*package\s+([A-Za-z_][A-Za-z0-9_:]*)\s*;").unwrap()
                    });
                    let re = &*PERL_PACKAGE_RE;
                    {
                        for caps in re.captures_iter(&content) {
                            let package_name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                            if !package_name.is_empty() {
                                let module_path = path
                                    .strip_prefix(root)
                                    .unwrap_or(path)
                                    .to_string_lossy()
                                    .to_string();

                                conn.execute(
                                    "INSERT OR IGNORE INTO modules (name, path) VALUES (?1, ?2)",
                                    rusqlite::params![package_name, module_path],
                                )?;
                                count += 1;
                            }
                        }
                    }
                }
            }

            // Maven modules (pom.xml)
            if name_str == "pom.xml" {
                if let Some(parent) = path.parent() {
                    let module_path = parent
                        .strip_prefix(root)
                        .unwrap_or(parent)
                        .to_string_lossy()
                        .to_string();

                    if let Ok(content) = fs::read_to_string(path) {
                        static ARTIFACT_RE: LazyLock<Regex> = LazyLock::new(|| {
                            Regex::new(r"<artifactId>\s*([^<]+?)\s*</artifactId>").unwrap()
                        });
                        let artifact_re = &*ARTIFACT_RE;
                        if let Some(caps) = artifact_re.captures(&content) {
                            let artifact_id = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                            if !artifact_id.is_empty() {
                                let module_name = if module_path.is_empty() {
                                    artifact_id.to_string()
                                } else {
                                    module_path.replace('/', ".")
                                };
                                conn.execute(
                                    "INSERT OR IGNORE INTO modules (name, path) VALUES (?1, ?2)",
                                    rusqlite::params![module_name, module_path],
                                )?;
                                count += 1;
                            }
                        }
                    }
                }
            }

            // ya.make build files — each directory with ya.make is a module, keyed by
            // its path relative to the outer repo root so that PEERDIR entries (which
            // use repo-root-relative paths) can be matched by literal lookup.
            if name_str == "ya.make" {
                if let Some(parent) = path.parent() {
                    // Prefer monorepo-root-relative; fall back to rebuild-root-relative if not in a monorepo
                    let rel = if let Some(ref mono) = mono_root {
                        parent.strip_prefix(mono).ok()
                    } else {
                        None
                    }
                    .or_else(|| parent.strip_prefix(root).ok())
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| parent.to_path_buf());

                    let module_name = rel.to_string_lossy().replace('\\', "/");
                    let module_path = parent
                        .strip_prefix(root)
                        .unwrap_or(parent)
                        .to_string_lossy()
                        .to_string();

                    if !module_name.is_empty() {
                        conn.execute(
                            "INSERT OR IGNORE INTO modules (name, path, kind) VALUES (?1, ?2, ?3)",
                            rusqlite::params![module_name, module_path, "ya.make"],
                        )?;
                        count += 1;
                    }
                }
            }

            // Python modules (pyproject.toml, setup.py, setup.cfg)
            if name_str == "pyproject.toml" || name_str == "setup.py" || name_str == "setup.cfg" {
                if let Some(parent) = path.parent() {
                    let module_path = parent
                        .strip_prefix(root)
                        .unwrap_or(parent)
                        .to_string_lossy()
                        .to_string();

                    // Use directory name as module name
                    let module_name = if module_path.is_empty() {
                        // Root project — try to extract name from pyproject.toml
                        if name_str == "pyproject.toml" {
                            if let Ok(content) = fs::read_to_string(path) {
                                extract_python_module_name(&content).unwrap_or_else(|| {
                                    root.file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or("root")
                                        .to_string()
                                })
                            } else {
                                root.file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("root")
                                    .to_string()
                            }
                        } else {
                            root.file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("root")
                                .to_string()
                        }
                    } else {
                        module_path.replace('/', ".")
                    };

                    if !module_name.is_empty() {
                        conn.execute(
                            "INSERT OR IGNORE INTO modules (name, path) VALUES (?1, ?2)",
                            rusqlite::params![module_name, module_path],
                        )?;
                        count += 1;
                    }
                }
            }
        }
    }

    Ok(count)
}

/// Extract quoted strings from a Python/TOML list body (the text inside [...]).
/// Handles both single and double quotes and ignores comments.
fn extract_py_list_strings(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' || c == b'\'' {
            let quote = c;
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != quote {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                    continue;
                }
                j += 1;
            }
            if j < bytes.len() {
                if let Ok(s) = std::str::from_utf8(&bytes[start..j]) {
                    out.push(s.to_string());
                }
                i = j + 1;
                continue;
            }
        }
        if c == b'#' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        }
        i += 1;
    }
    out
}

/// Strip PEP 508 version specifiers / extras / markers from a dependency string,
/// returning just the package name. e.g. "foo[extra]>=1.0; python_version>='3.8'" -> "foo"
fn strip_py_version(dep: &str) -> String {
    let dep = dep.trim();
    let end = dep
        .find(|c: char| {
            c == '['
                || c == '<'
                || c == '>'
                || c == '='
                || c == '!'
                || c == '~'
                || c == ';'
                || c == ' '
        })
        .unwrap_or(dep.len());
    dep[..end].to_string()
}

/// Extract project name from pyproject.toml content
fn extract_python_module_name(content: &str) -> Option<String> {
    static PYPROJECT_NAME_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?m)^\s*name\s*=\s*["']([^"']+)["']"#).unwrap());
    let re = &*PYPROJECT_NAME_RE;
    re.captures(content)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
}

/// Collect build files (Gradle, Maven, ya.make, Python) from module paths in DB (for standalone rebuild modules/deps)
pub fn collect_build_files_from_db(conn: &Connection, root: &Path) -> Result<Vec<PathBuf>> {
    let mut stmt = conn.prepare("SELECT path FROM modules")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut files = Vec::new();
    for row in rows {
        let module_path = row?;
        let dir = root.join(&module_path);
        for name in &[
            "build.gradle.kts",
            "build.gradle",
            "pom.xml",
            "ya.make",
            "pyproject.toml",
            "setup.py",
            "setup.cfg",
        ] {
            let p = dir.join(name);
            if p.exists() {
                files.push(p);
                break;
            }
        }
    }
    Ok(files)
}

/// Locate Forma-style `<name>dependencies = wrapper(...) [+ wrapper(...)]*` blocks in a
/// Gradle file. Returns the byte ranges (start of the assignment, end of the last
/// chained wrapper call). Used to scope the unanchored `project(...)` fallback so it
/// does not match comments, string literals, or unrelated code elsewhere in the file.
fn find_forma_deps_blocks(content: &str) -> Vec<(usize, usize)> {
    static START_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)\b\w*[Dd]ependencies\s*=\s*\w+\s*\(").unwrap());

    let bytes = content.as_bytes();
    let mut blocks = Vec::new();

    for m in START_RE.find_iter(content) {
        let span_start = m.start();
        let mut i = m.end();
        let mut depth = 1usize;
        while i < bytes.len() && depth > 0 {
            match bytes[i] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            i += 1;
            if depth == 0 {
                break;
            }
        }

        loop {
            let ws_end = bytes[i..]
                .iter()
                .position(|b| !b.is_ascii_whitespace())
                .map(|p| i + p)
                .unwrap_or(bytes.len());
            if ws_end >= bytes.len() || bytes[ws_end] != b'+' {
                break;
            }
            let mut j = ws_end + 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            let ident_start = j;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if j == ident_start {
                break;
            }
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= bytes.len() || bytes[j] != b'(' {
                break;
            }
            j += 1;
            let mut d2 = 1usize;
            while j < bytes.len() && d2 > 0 {
                match bytes[j] {
                    b'(' => d2 += 1,
                    b')' => d2 -= 1,
                    _ => {}
                }
                j += 1;
                if d2 == 0 {
                    break;
                }
            }
            i = j;
        }

        blocks.push((span_start, i));
    }

    blocks
}

/// Strip `//` line comments from a Kotlin/Gradle slice. Naive — does not understand
/// string literals — but the only consumer is regex capture of a quoted path, where
/// a `//` inside a string would already be malformed Kotlin.
fn strip_kt_line_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, line) in s.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match line.find("//") {
            Some(idx) => out.push_str(&line[..idx]),
            None => out.push_str(line),
        }
    }
    out
}

/// Parse module dependencies from collected build files (Gradle, Maven, ya.make, Python)
pub fn index_module_dependencies(
    conn: &mut Connection,
    root: &Path,
    gradle_files: &[PathBuf],
    progress: bool,
) -> Result<usize> {
    // Regex patterns for dependency declarations
    // Gradle projects DSL style: modules { api(projects.features.payments.api) }
    static PROJECTS_DEP_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^\s*(api|implementation|compileOnly|testImplementation)\s*\(\s*projects\.([a-zA-Z_][a-zA-Z0-9_.]*)\s*\)").unwrap()
    });

    let projects_dep_re = &*PROJECTS_DEP_RE;

    // Gradle project(...) deps: implementation(project(":features:payments:api"))
    // Matches patterns like: implementation(project(":path")) or deps(project(":path"))
    // Capture group 1 is the configuration/wrapper identifier; the leading `:` on the path is optional.
    static GRADLE_PROJECT_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?m)\b(\w+)\s*\(\s*project\s*\(\s*["']:?([^"']+)["']\s*\)"#).unwrap()
    });

    let gradle_project_re = &*GRADLE_PROJECT_RE;

    // Fallback: match any project(":path") inside a Forma-style `dependencies = wrapper(...)`
    // block. The wrapper-anchored regex above only fires once per `wrapper(`, missing 2nd+
    // project() declarations in a single block. Scoping to the assignment block (via
    // `find_forma_deps_blocks`) prevents matches in top-level comments, string literals,
    // or unrelated code that happens to contain `project("...")`.
    // See https://github.com/formatools/forma for the Forma DSL.
    static PROJECT_ONLY_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?m)project\s*\(\s*["']:?([^"']+)["']\s*\)"#).unwrap());

    let project_only_re = &*PROJECT_ONLY_RE;

    // ya.make PEERDIR(...) — accepts one or more whitespace-separated paths
    static PEERDIR_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)PEERDIR\s*\(\s*([^)]*)\s*\)").unwrap());
    let peerdir_re = &*PEERDIR_RE;

    // Python pyproject.toml: [project] dependencies = ["foo>=1.0", ...]
    static PY_PROJECT_DEPS_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?ms)^\s*dependencies\s*=\s*\[([^\]]*)\]").unwrap());
    let py_project_deps_re = &*PY_PROJECT_DEPS_RE;

    // Python pyproject.toml poetry section: [tool.poetry.dependencies]
    static PY_POETRY_SECTION_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?ms)^\s*\[\s*tool\.poetry\.dependencies\s*\]\s*$(.*?)(?:^\s*\[|\z)").unwrap()
    });
    let py_poetry_section_re = &*PY_POETRY_SECTION_RE;

    // Python setup.py install_requires=[...]
    static PY_SETUP_DEPS_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?ms)install_requires\s*=\s*\[([^\]]*)\]"#).unwrap());
    let py_setup_deps_re = &*PY_SETUP_DEPS_RE;

    let mono_root = find_arc_root(root);

    // First, ensure all modules are indexed and get their IDs
    let module_ids: std::collections::HashMap<String, i64> = {
        let mut stmt = conn.prepare("SELECT id, name FROM modules")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(0)?))
        })?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (name, id) = row?;
            map.insert(name, id);
        }
        map
    };

    if progress {
        eprintln!("Found {} modules in index", module_ids.len());
    }

    let mut dep_count = 0;
    let tx = conn.transaction()?;

    // Clear existing dependencies
    tx.execute("DELETE FROM module_deps", [])?;

    {
        let mut dep_stmt = tx.prepare_cached(
            "INSERT OR IGNORE INTO module_deps (module_id, dep_module_id, dep_kind) VALUES (?1, ?2, ?3)"
        )?;

        // Maven dependency regex: <dependency>...<artifactId>name</artifactId>...</dependency>
        static MAVEN_DEP_RE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(
                r"(?s)<dependency>.*?<artifactId>\s*([^<]+?)\s*</artifactId>.*?</dependency>",
            )
            .unwrap()
        });
        let maven_dep_re = &*MAVEN_DEP_RE;

        let mut edges: Vec<(i64, i64, String)> = {
            let num_threads = effective_num_threads();
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(num_threads)
                .stack_size(RAYON_WORKER_STACK_SIZE)
                .build()
                .map_err(|e| anyhow::anyhow!("Failed to build thread pool: {}", e))?;
            let root_buf = root.to_path_buf();
            let mono_root = mono_root.clone();
            let module_ids = Arc::new(module_ids.clone());

            pool.install(|| {
                gradle_files
                    .par_iter()
                    .flat_map_iter(|path| {
                        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        let parent = match path.parent() {
                            Some(p) => p,
                            None => return Vec::new(),
                        };

                        let source_module_name: String = match file_name {
                            "ya.make" => {
                                let rel = if let Some(ref mono) = mono_root {
                                    parent.strip_prefix(mono).ok()
                                } else {
                                    None
                                }
                                .or_else(|| parent.strip_prefix(&root_buf).ok())
                                .map(|p| p.to_path_buf())
                                .unwrap_or_else(|| parent.to_path_buf());
                                rel.to_string_lossy().replace('\\', "/")
                            }
                            "pyproject.toml" | "setup.py" | "setup.cfg" => {
                                let module_path = parent
                                    .strip_prefix(&root_buf)
                                    .unwrap_or(parent)
                                    .to_string_lossy()
                                    .to_string();
                                if module_path.is_empty() {
                                    if file_name == "pyproject.toml" {
                                        fs::read_to_string(path)
                                            .ok()
                                            .as_deref()
                                            .and_then(extract_python_module_name)
                                            .unwrap_or_else(|| {
                                                root_buf
                                                    .file_name()
                                                    .and_then(|n| n.to_str())
                                                    .unwrap_or("root")
                                                    .to_string()
                                            })
                                    } else {
                                        root_buf
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .unwrap_or("root")
                                            .to_string()
                                    }
                                } else {
                                    module_path.replace('/', ".")
                                }
                            }
                            _ => parent
                                .strip_prefix(&root_buf)
                                .unwrap_or(parent)
                                .to_string_lossy()
                                .replace('/', "."),
                        };

                        let module_id = match module_ids.get(&source_module_name) {
                            Some(&id) => id,
                            None => return Vec::new(),
                        };

                        let content = match fs::read_to_string(path) {
                            Ok(c) => c,
                            Err(_) => return Vec::new(),
                        };

                        let mut edges = Vec::new();
                        match file_name {
                            "pom.xml" => {
                                for caps in maven_dep_re.captures_iter(&content) {
                                    let artifact_id = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                                    for (mod_name, &mod_id) in module_ids.iter() {
                                        let last_segment =
                                            mod_name.rsplit('.').next().unwrap_or(mod_name);
                                        if last_segment == artifact_id {
                                            edges.push((module_id, mod_id, "compile".to_string()));
                                        }
                                    }
                                }
                            }
                            "ya.make" => {
                                for caps in peerdir_re.captures_iter(&content) {
                                    let raw = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                                    for token in raw.split_ascii_whitespace() {
                                        let dep_name = token.trim_end_matches(',').trim();
                                        if dep_name.is_empty() || dep_name.starts_with('#') {
                                            continue;
                                        }
                                        let dep_name = dep_name.replace('\\', "/");
                                        if let Some(&dep_id) = module_ids.get(&dep_name) {
                                            edges.push((module_id, dep_id, "peerdir".to_string()));
                                        }
                                    }
                                }
                            }
                            "pyproject.toml" => {
                                for caps in py_project_deps_re.captures_iter(&content) {
                                    let body = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                                    for raw in extract_py_list_strings(body) {
                                        let dep_name = strip_py_version(&raw);
                                        if let Some(&dep_id) = module_ids.get(&dep_name) {
                                            edges.push((module_id, dep_id, "compile".to_string()));
                                        }
                                    }
                                }
                                if let Some(caps) = py_poetry_section_re.captures(&content) {
                                    let section = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                                    for line in section.lines() {
                                        let line = line.trim();
                                        if line.is_empty()
                                            || line.starts_with('#')
                                            || line.starts_with('[')
                                        {
                                            continue;
                                        }
                                        if let Some(eq_pos) = line.find('=') {
                                            let dep_name = line[..eq_pos]
                                                .trim()
                                                .trim_matches('"')
                                                .trim_matches('\'');
                                            if dep_name == "python" || dep_name.is_empty() {
                                                continue;
                                            }
                                            if let Some(&dep_id) = module_ids.get(dep_name) {
                                                edges.push((
                                                    module_id,
                                                    dep_id,
                                                    "compile".to_string(),
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                            "setup.py" | "setup.cfg" => {
                                for caps in py_setup_deps_re.captures_iter(&content) {
                                    let body = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                                    for raw in extract_py_list_strings(body) {
                                        let dep_name = strip_py_version(&raw);
                                        if let Some(&dep_id) = module_ids.get(&dep_name) {
                                            edges.push((module_id, dep_id, "compile".to_string()));
                                        }
                                    }
                                }
                            }
                            _ => {
                                let mut inserted: std::collections::HashSet<(i64, i64)> =
                                    std::collections::HashSet::new();
                                for caps in projects_dep_re.captures_iter(&content) {
                                    let dep_kind =
                                        caps.get(1).map(|m| m.as_str()).unwrap_or("implementation");
                                    let dep_name = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                                    if let Some(&dep_id) = module_ids.get(dep_name) {
                                        if inserted.insert((module_id, dep_id)) {
                                            edges.push((module_id, dep_id, dep_kind.to_string()));
                                        }
                                    }
                                }
                                for caps in gradle_project_re.captures_iter(&content) {
                                    let dep_kind =
                                        caps.get(1).map(|m| m.as_str()).unwrap_or("implementation");
                                    let dep_path = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                                    let dep_name =
                                        dep_path.trim_start_matches(':').replace(':', ".");
                                    if let Some(&dep_id) = module_ids.get(&dep_name) {
                                        if inserted.insert((module_id, dep_id)) {
                                            edges.push((module_id, dep_id, dep_kind.to_string()));
                                        }
                                    }
                                }
                                for (b_start, b_end) in find_forma_deps_blocks(&content) {
                                    let block = strip_kt_line_comments(&content[b_start..b_end]);
                                    for caps in project_only_re.captures_iter(&block) {
                                        let dep_path =
                                            caps.get(1).map(|m| m.as_str()).unwrap_or("");
                                        let dep_name =
                                            dep_path.trim_start_matches(':').replace(':', ".");
                                        if let Some(&dep_id) = module_ids.get(&dep_name) {
                                            if inserted.insert((module_id, dep_id)) {
                                                edges.push((
                                                    module_id,
                                                    dep_id,
                                                    "implementation".to_string(),
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        edges
                    })
                    .collect()
            })
        };

        edges.sort_unstable();
        edges.dedup();

        for (module_id, dep_id, dep_kind) in edges {
            dep_stmt.execute(rusqlite::params![module_id, dep_id, dep_kind])?;
            dep_count += 1;
        }
    }

    db::mark_modules_indexed(&tx)?;
    tx.commit()?;

    Ok(dep_count)
}

/// Get dependencies of a module
pub fn get_module_deps(
    conn: &Connection,
    module_name: &str,
) -> Result<Vec<(String, String, String)>> {
    // Returns (dep_module_name, dep_module_path, dep_kind)
    let mut stmt = conn.prepare(
        r#"
        SELECT m2.name, m2.path, md.dep_kind
        FROM module_deps md
        JOIN modules m1 ON md.module_id = m1.id
        JOIN modules m2 ON md.dep_module_id = m2.id
        WHERE m1.name = ?1 OR m1.path = ?1
        ORDER BY md.dep_kind, m2.name
        "#,
    )?;

    let results = stmt
        .query_map(rusqlite::params![module_name], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Get modules that depend on this module
pub fn get_module_dependents(
    conn: &Connection,
    module_name: &str,
) -> Result<Vec<(String, String, String)>> {
    // Returns (dependent_module_name, dependent_module_path, dep_kind)
    let mut stmt = conn.prepare(
        r#"
        SELECT m1.name, m1.path, md.dep_kind
        FROM module_deps md
        JOIN modules m1 ON md.module_id = m1.id
        JOIN modules m2 ON md.dep_module_id = m2.id
        WHERE m2.name = ?1 OR m2.path = ?1
        ORDER BY md.dep_kind, m1.name
        "#,
    )?;

    let results = stmt
        .query_map(rusqlite::params![module_name], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Parsed XML usage
#[derive(Debug)]
pub struct XmlUsage {
    pub file_path: String,
    pub line: usize,
    pub class_name: String,
    pub usage_type: String,
    pub element_id: Option<String>,
}

/// Index XML layouts for class usages
pub fn index_xml_usages(
    conn: &mut Connection,
    root: &Path,
    xml_layout_files: &[PathBuf],
    progress: bool,
) -> Result<usize> {
    let module_lookup = ModuleLookup::from_db(conn)?;

    // Regex for class names in XML
    // Full class name: <com.example.MyView ...>
    static FULL_CLASS_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"<([a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*\.[A-Z][a-zA-Z0-9_]*)").unwrap()
    });

    let full_class_re = &*FULL_CLASS_RE;
    // view class="..." or fragment android:name="..."
    static CLASS_ATTR_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?:class|android:name)\s*=\s*["']([a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*\.[A-Z][a-zA-Z0-9_]*)["']"#).unwrap()
    });

    let class_attr_re = &*CLASS_ATTR_RE;
    // android:id="@+id/xxx"
    static ID_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"android:id\s*=\s*["']@\+?id/([^"']+)["']"#).unwrap());

    let id_re = &*ID_RE;

    if progress {
        eprintln!(
            "Found {} XML layout files to index...",
            xml_layout_files.len()
        );
    }

    let tx = conn.transaction()?;

    // Clear existing XML usages
    tx.execute("DELETE FROM xml_usages", [])?;

    let mut count = 0;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO xml_usages (module_id, file_path, line, class_name, usage_type, element_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        )?;

        let usage_rows: Vec<(
            Option<i64>,
            String,
            i64,
            String,
            &'static str,
            Option<String>,
        )> = {
            let num_threads = effective_num_threads();
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(num_threads)
                .stack_size(RAYON_WORKER_STACK_SIZE)
                .build()
                .map_err(|e| anyhow::anyhow!("Failed to build thread pool: {}", e))?;
            let root_buf = root.to_path_buf();
            let module_lookup = module_lookup.clone();

            pool.install(|| {
                xml_layout_files
                    .par_iter()
                    .flat_map_iter(|xml_path| {
                        let rel_path = xml_path
                            .strip_prefix(&root_buf)
                            .unwrap_or(xml_path)
                            .to_string_lossy()
                            .to_string();
                        let module_id = module_lookup.find(&rel_path);
                        let content = match fs::read_to_string(xml_path) {
                            Ok(content) => content,
                            Err(_) => return Vec::new(),
                        };

                        let mut rows = Vec::new();
                        for (line_idx, line) in content.lines().enumerate() {
                            if !line.contains('.')
                                && !line.contains("class")
                                && !line.contains("android:name")
                            {
                                continue;
                            }

                            let line_num = line_idx as i64 + 1;
                            let element_id = id_re
                                .captures(line)
                                .map(|c| c.get(1).unwrap().as_str().to_string());

                            if line.contains('<') && line.contains('.') {
                                for caps in full_class_re.captures_iter(line) {
                                    rows.push((
                                        module_id,
                                        rel_path.clone(),
                                        line_num,
                                        caps.get(1).unwrap().as_str().to_string(),
                                        "view_tag",
                                        element_id.clone(),
                                    ));
                                }
                            }

                            if line.contains("class") || line.contains("android:name") {
                                let usage_type = if line.contains("<fragment")
                                    || line.contains("android:name")
                                {
                                    "fragment"
                                } else {
                                    "view_class_attr"
                                };
                                for caps in class_attr_re.captures_iter(line) {
                                    rows.push((
                                        module_id,
                                        rel_path.clone(),
                                        line_num,
                                        caps.get(1).unwrap().as_str().to_string(),
                                        usage_type,
                                        element_id.clone(),
                                    ));
                                }
                            }
                        }
                        rows
                    })
                    .collect()
            })
        };

        for (module_id, rel_path, line_num, class_name, usage_type, element_id) in usage_rows {
            stmt.execute(rusqlite::params![
                module_id, rel_path, line_num, class_name, usage_type, element_id
            ])?;
            count += 1;
        }
    }

    tx.commit()?;

    Ok(count)
}

/// Resource type
#[derive(Debug, Clone, PartialEq)]
pub enum ResourceType {
    Drawable,
    String,
    Color,
    Dimen,
    Style,
    Layout,
    Id,
    Mipmap,
    Other(String),
}

impl ResourceType {
    pub fn as_str(&self) -> &str {
        match self {
            ResourceType::Drawable => "drawable",
            ResourceType::String => "string",
            ResourceType::Color => "color",
            ResourceType::Dimen => "dimen",
            ResourceType::Style => "style",
            ResourceType::Layout => "layout",
            ResourceType::Id => "id",
            ResourceType::Mipmap => "mipmap",
            ResourceType::Other(s) => s,
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "drawable" => ResourceType::Drawable,
            "string" => ResourceType::String,
            "color" => ResourceType::Color,
            "dimen" => ResourceType::Dimen,
            "style" => ResourceType::Style,
            "layout" => ResourceType::Layout,
            "id" => ResourceType::Id,
            "mipmap" => ResourceType::Mipmap,
            other => ResourceType::Other(other.to_string()),
        }
    }
}

/// Index Android resources (drawable, string, color, etc.)
pub fn index_resources(
    conn: &mut Connection,
    root: &Path,
    res_files: &[PathBuf],
    progress: bool,
) -> Result<(usize, usize)> {
    let module_lookup = ModuleLookup::from_db(conn)?;

    if progress {
        eprintln!("Found {} resource files to analyze...", res_files.len());
    }

    let tx = conn.transaction()?;

    // Clear existing resources
    tx.execute("DELETE FROM resource_usages", [])?;
    tx.execute("DELETE FROM resources", [])?;

    let mut resource_count = 0;
    let mut usage_count = 0;

    // Regex for resource references
    static R_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"R\.(drawable|string|color|dimen|style|layout|id|mipmap)\.([a-zA-Z_][a-zA-Z0-9_]*)",
        )
        .unwrap()
    });

    let r_ref_re = &*R_REF_RE;
    static XML_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"@(drawable|string|color|dimen|style|layout|id|mipmap)/([a-zA-Z_][a-zA-Z0-9_]*)"#,
        )
        .unwrap()
    });

    let xml_ref_re = &*XML_REF_RE;

    // Resource definitions regex for values/*.xml
    static STRING_DEF_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<string\s+name="([^"]+)""#).unwrap());

    let string_def_re = &*STRING_DEF_RE;
    static COLOR_DEF_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<color\s+name="([^"]+)""#).unwrap());

    let color_def_re = &*COLOR_DEF_RE;
    static DIMEN_DEF_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<dimen\s+name="([^"]+)""#).unwrap());

    let dimen_def_re = &*DIMEN_DEF_RE;
    static STYLE_DEF_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<style\s+name="([^"]+)""#).unwrap());

    let style_def_re = &*STYLE_DEF_RE;

    {
        let mut res_stmt = tx.prepare_cached(
            "INSERT INTO resources (module_id, type, name, file_path, line) VALUES (?1, ?2, ?3, ?4, ?5)"
        )?;

        // First pass: index resource definitions
        for res_path in res_files {
            let rel_path = res_path
                .strip_prefix(root)
                .unwrap_or(res_path)
                .to_string_lossy()
                .to_string();

            let module_id = module_lookup.find(&rel_path);

            // Drawable files
            if rel_path.contains("/drawable") || rel_path.contains("/mipmap") {
                if let Some(name) = res_path.file_stem().and_then(|n| n.to_str()) {
                    let res_type = if rel_path.contains("/mipmap") {
                        "mipmap"
                    } else {
                        "drawable"
                    };
                    res_stmt.execute(rusqlite::params![module_id, res_type, name, rel_path, 1])?;
                    resource_count += 1;
                }
            }

            // Layout files
            if rel_path.contains("/layout") && rel_path.ends_with(".xml") {
                if let Some(name) = res_path.file_stem().and_then(|n| n.to_str()) {
                    res_stmt.execute(rusqlite::params![module_id, "layout", name, rel_path, 1])?;
                    resource_count += 1;
                }
            }

            // Values files (strings, colors, dimens, styles)
            if rel_path.contains("/values") && rel_path.ends_with(".xml") {
                if let Ok(content) = fs::read_to_string(res_path) {
                    for (line_num, line) in content.lines().enumerate() {
                        let line_num = line_num + 1;

                        if let Some(caps) = string_def_re.captures(line) {
                            let name = caps.get(1).unwrap().as_str();
                            res_stmt.execute(rusqlite::params![
                                module_id,
                                "string",
                                name,
                                rel_path,
                                line_num as i64
                            ])?;
                            resource_count += 1;
                        }
                        if let Some(caps) = color_def_re.captures(line) {
                            let name = caps.get(1).unwrap().as_str();
                            res_stmt.execute(rusqlite::params![
                                module_id,
                                "color",
                                name,
                                rel_path,
                                line_num as i64
                            ])?;
                            resource_count += 1;
                        }
                        if let Some(caps) = dimen_def_re.captures(line) {
                            let name = caps.get(1).unwrap().as_str();
                            res_stmt.execute(rusqlite::params![
                                module_id,
                                "dimen",
                                name,
                                rel_path,
                                line_num as i64
                            ])?;
                            resource_count += 1;
                        }
                        if let Some(caps) = style_def_re.captures(line) {
                            let name = caps.get(1).unwrap().as_str();
                            res_stmt.execute(rusqlite::params![
                                module_id,
                                "style",
                                name,
                                rel_path,
                                line_num as i64
                            ])?;
                            resource_count += 1;
                        }
                    }
                }
            }
        }
    }

    // Build resource ID map: type -> name -> id (two-level for allocation-free lookup)
    let resource_ids: std::collections::HashMap<String, std::collections::HashMap<String, i64>> = {
        let mut stmt = tx.prepare("SELECT id, type, name FROM resources")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut map: std::collections::HashMap<String, std::collections::HashMap<String, i64>> =
            std::collections::HashMap::new();
        for row in rows {
            let (id, res_type, name) = row?;
            map.entry(res_type).or_default().insert(name, id);
        }
        map
    };

    // Second pass: index resource usages
    {
        let mut usage_stmt = tx.prepare_cached(
            "INSERT INTO resource_usages (resource_id, usage_file, usage_line, usage_type) VALUES (?1, ?2, ?3, ?4)"
        )?;

        // Query code files from DB instead of walking filesystem again
        let code_rel_paths: Vec<String> = {
            let mut stmt = tx.prepare("SELECT path FROM files WHERE path LIKE '%.kt' OR path LIKE '%.java' OR path LIKE '%.xml'")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.filter_map(|r| r.ok()).collect()
        };
        if progress {
            eprintln!("Scanning resource usages in parallel...");
        }

        let num_threads = effective_num_threads();

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .stack_size(RAYON_WORKER_STACK_SIZE)
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build thread pool: {}", e))?;

        let root_buf = root.to_path_buf();
        let resource_ids = Arc::new(resource_ids);
        let usage_batches: Vec<Vec<(i64, String, i64, &'static str)>> = pool.install(|| {
            code_rel_paths
                .par_iter()
                .map(|rel_path| {
                    let file_path = root_buf.join(rel_path);
                    let content = match fs::read_to_string(file_path) {
                        Ok(content) => content,
                        Err(_) => return Vec::new(),
                    };

                    let is_xml = rel_path.ends_with(".xml");
                    let mut usages = Vec::new();

                    for (line_idx, line) in content.lines().enumerate() {
                        let line_num = line_idx as i64 + 1;

                        if !is_xml && line.contains("R.") {
                            for caps in r_ref_re.captures_iter(line) {
                                let res_type = caps.get(1).unwrap().as_str();
                                let res_name = caps.get(2).unwrap().as_str();

                                if let Some(&resource_id) =
                                    resource_ids.get(res_type).and_then(|m| m.get(res_name))
                                {
                                    usages.push((resource_id, rel_path.clone(), line_num, "code"));
                                }
                            }
                        }

                        if line.contains('@') {
                            for caps in xml_ref_re.captures_iter(line) {
                                let res_type = caps.get(1).unwrap().as_str();
                                let res_name = caps.get(2).unwrap().as_str();

                                if let Some(&resource_id) =
                                    resource_ids.get(res_type).and_then(|m| m.get(res_name))
                                {
                                    usages.push((resource_id, rel_path.clone(), line_num, "xml"));
                                }
                            }
                        }
                    }

                    usages
                })
                .collect()
        });

        for batch in usage_batches {
            for (resource_id, rel_path, line_num, usage_type) in batch {
                usage_stmt.execute(rusqlite::params![
                    resource_id,
                    rel_path,
                    line_num,
                    usage_type
                ])?;
                usage_count += 1;
            }
        }
    }

    tx.commit()?;

    Ok((resource_count, usage_count))
}

/// Build transitive dependencies cache
pub fn build_transitive_deps(conn: &mut Connection, progress: bool) -> Result<usize> {
    // Get all direct dependencies
    let direct_deps: Vec<(i64, i64, String)> = {
        let mut stmt =
            conn.prepare("SELECT module_id, dep_module_id, dep_kind FROM module_deps")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    // Get module names
    let module_names: std::collections::HashMap<i64, String> = {
        let mut stmt = conn.prepare("SELECT id, name FROM modules")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (id, name) = row?;
            map.insert(id, name);
        }
        map
    };

    // Build adjacency list (only api dependencies create transitive access)
    let mut api_deps: std::collections::HashMap<i64, Vec<i64>> = std::collections::HashMap::new();
    for (module_id, dep_id, dep_kind) in &direct_deps {
        if dep_kind == "api" {
            api_deps.entry(*module_id).or_default().push(*dep_id);
        }
    }

    let tx = conn.transaction()?;

    // Clear existing
    tx.execute("DELETE FROM transitive_deps", [])?;

    let mut count = 0;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO transitive_deps (module_id, dependency_id, depth, path) VALUES (?1, ?2, ?3, ?4)"
        )?;

        let unknown = "?";

        // For each module, BFS to find all transitive dependencies
        for (module_id, dep_id, _) in &direct_deps {
            let mod_name = module_names
                .get(module_id)
                .map(|s| s.as_str())
                .unwrap_or(unknown);
            let dep_name = module_names
                .get(dep_id)
                .map(|s| s.as_str())
                .unwrap_or(unknown);

            // Direct dependency
            let path = format!("{} -> {}", mod_name, dep_name);
            stmt.execute(rusqlite::params![module_id, dep_id, 1, path])?;
            count += 1;

            // BFS for transitive (only through api deps)
            let mut visited: std::collections::HashSet<i64> = std::collections::HashSet::new();
            visited.insert(*dep_id);
            let mut queue: std::collections::VecDeque<(i64, usize, String)> =
                std::collections::VecDeque::new();

            // Add api dependencies of dep_id
            if let Some(next_deps) = api_deps.get(dep_id) {
                for &next_dep in next_deps {
                    let next_name = module_names
                        .get(&next_dep)
                        .map(|s| s.as_str())
                        .unwrap_or(unknown);
                    let next_path = format!("{} -> {} -> {}", mod_name, dep_name, next_name);
                    queue.push_back((next_dep, 2, next_path));
                }
            }

            while let Some((trans_dep, depth, path)) = queue.pop_front() {
                if visited.contains(&trans_dep) || depth > 5 {
                    continue;
                }
                visited.insert(trans_dep);

                stmt.execute(rusqlite::params![module_id, trans_dep, depth as i64, path])?;
                count += 1;

                // Continue BFS
                if let Some(next_deps) = api_deps.get(&trans_dep) {
                    for &next_dep in next_deps {
                        if !visited.contains(&next_dep) {
                            let next_name = module_names
                                .get(&next_dep)
                                .map(|s| s.as_str())
                                .unwrap_or(unknown);
                            let next_path = format!("{} -> {}", path, next_name);
                            queue.push_back((next_dep, depth + 1, next_path));
                        }
                    }
                }
            }
        }
    }

    tx.commit()?;

    if progress {
        eprintln!("Built {} transitive dependency entries", count);
    }

    Ok(count)
}

/// Parsed iOS Storyboard/XIB usage
#[derive(Debug)]
pub struct StoryboardUsage {
    pub file_path: String,
    pub line: usize,
    pub class_name: String,
    pub usage_type: String, // "viewController", "view", "cell", "segue"
    pub storyboard_id: Option<String>,
}

/// Index iOS storyboard and XIB files for class usages
pub fn index_storyboard_usages(
    conn: &mut Connection,
    root: &Path,
    storyboard_files: &[PathBuf],
    progress: bool,
) -> Result<usize> {
    let module_lookup = ModuleLookup::from_db(conn)?;

    // Regex for customClass in storyboards/xibs
    // <viewController customClass="MyViewController" ...>
    static CUSTOM_CLASS_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"customClass\s*=\s*["']([A-Z][a-zA-Z0-9_]+)["']"#).unwrap());

    let custom_class_re = &*CUSTOM_CLASS_RE;
    // storyboardIdentifier="..."
    static STORYBOARD_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?:storyboardIdentifier|identifier)\s*=\s*["']([^"']+)["']"#).unwrap()
    });

    let storyboard_id_re = &*STORYBOARD_ID_RE;

    if progress {
        eprintln!(
            "Found {} storyboard/xib files to index...",
            storyboard_files.len()
        );
    }

    let tx = conn.transaction()?;

    // Clear existing storyboard usages
    tx.execute("DELETE FROM storyboard_usages", [])?;

    let mut count = 0;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO storyboard_usages (module_id, file_path, line, class_name, usage_type, storyboard_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        )?;

        for sb_path in storyboard_files {
            let rel_path = sb_path
                .strip_prefix(root)
                .unwrap_or(sb_path)
                .to_string_lossy()
                .to_string();

            // Find module for this file
            let module_id = module_lookup.find(&rel_path);

            if let Ok(content) = fs::read_to_string(sb_path) {
                for (line_num, line) in content.lines().enumerate() {
                    let line_num = line_num + 1;

                    // Extract storyboard identifier if present
                    let sb_id = storyboard_id_re
                        .captures(line)
                        .map(|c| c.get(1).unwrap().as_str().to_string());

                    // Extract custom classes
                    if let Some(caps) = custom_class_re.captures(line) {
                        let class_name = caps.get(1).unwrap().as_str();

                        // Determine usage type based on element
                        let usage_type = if line.contains("<viewController")
                            || line.contains("<tableViewController")
                            || line.contains("<collectionViewController")
                            || line.contains("<navigationController")
                            || line.contains("<tabBarController")
                        {
                            "viewController"
                        } else if line.contains("<tableViewCell")
                            || line.contains("<collectionViewCell")
                        {
                            "cell"
                        } else if line.contains("<view") || line.contains("<View") {
                            "view"
                        } else {
                            "other"
                        };

                        stmt.execute(rusqlite::params![
                            module_id,
                            rel_path,
                            line_num as i64,
                            class_name,
                            usage_type,
                            sb_id
                        ])?;
                        count += 1;
                    }
                }
            }
        }
    }

    tx.commit()?;

    if progress {
        eprintln!("Indexed {} storyboard/xib class usages", count);
    }

    Ok(count)
}

/// iOS Asset type
#[derive(Debug, Clone, PartialEq)]
pub enum IosAssetType {
    ImageSet,
    ColorSet,
    AppIcon,
    LaunchImage,
    DataSet,
    Other(String),
}

impl IosAssetType {
    pub fn as_str(&self) -> &str {
        match self {
            IosAssetType::ImageSet => "imageset",
            IosAssetType::ColorSet => "colorset",
            IosAssetType::AppIcon => "appiconset",
            IosAssetType::LaunchImage => "launchimage",
            IosAssetType::DataSet => "dataset",
            IosAssetType::Other(s) => s,
        }
    }

    pub fn from_extension(ext: &str) -> Self {
        match ext {
            "imageset" => IosAssetType::ImageSet,
            "colorset" => IosAssetType::ColorSet,
            "appiconset" => IosAssetType::AppIcon,
            "launchimage" => IosAssetType::LaunchImage,
            "dataset" => IosAssetType::DataSet,
            other => IosAssetType::Other(other.to_string()),
        }
    }
}

/// Index iOS Assets.xcassets
pub fn index_ios_assets(
    conn: &mut Connection,
    root: &Path,
    xcassets_dirs: &[PathBuf],
    progress: bool,
) -> Result<(usize, usize)> {
    use ignore::WalkBuilder;

    let module_lookup = ModuleLookup::from_db(conn)?;

    if progress {
        eprintln!("Found {} .xcassets directories...", xcassets_dirs.len());
    }

    let tx = conn.transaction()?;

    // Clear existing iOS assets
    tx.execute("DELETE FROM ios_asset_usages", [])?;
    tx.execute("DELETE FROM ios_assets", [])?;

    let mut asset_count = 0;
    let mut usage_count = 0;

    {
        let mut asset_stmt = tx.prepare_cached(
            "INSERT INTO ios_assets (module_id, type, name, file_path) VALUES (?1, ?2, ?3, ?4)",
        )?;

        // Index assets from .xcassets directories
        for xcassets_dir in xcassets_dirs {
            let rel_xcassets = xcassets_dir
                .strip_prefix(root)
                .unwrap_or(xcassets_dir)
                .to_string_lossy()
                .to_string();

            let module_id = module_lookup.find(&rel_xcassets);

            // Walk inside xcassets to find imagesets, colorsets, etc.
            let inner_walker = WalkBuilder::new(xcassets_dir).hidden(false).build();

            for entry in inner_walker {
                if let Ok(entry) = entry {
                    let path = entry.path();
                    if path.is_dir() {
                        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                            if matches!(
                                ext,
                                "imageset" | "colorset" | "appiconset" | "launchimage" | "dataset"
                            ) {
                                if let Some(name) = path.file_stem().and_then(|n| n.to_str()) {
                                    let rel_path = path
                                        .strip_prefix(root)
                                        .unwrap_or(path)
                                        .to_string_lossy()
                                        .to_string();

                                    let asset_type = IosAssetType::from_extension(ext);
                                    asset_stmt.execute(rusqlite::params![
                                        module_id,
                                        asset_type.as_str(),
                                        name,
                                        rel_path
                                    ])?;
                                    asset_count += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Build asset ID map
    let asset_ids: std::collections::HashMap<String, i64> = {
        let mut stmt = tx.prepare("SELECT id, name FROM ios_assets")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(0)?))
        })?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (name, id) = row?;
            map.insert(name, id);
        }
        map
    };

    // Index asset usages in Swift code
    // UIImage(named: "assetName") or Image("assetName") or Color("colorName")
    static SWIFT_IMAGE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?:UIImage\s*\(\s*named:\s*["']|Image\s*\(\s*["']|\.image\s*\(\s*named:\s*["'])([^"']+)["']"#).unwrap()
    });

    let swift_image_re = &*SWIFT_IMAGE_RE;
    static SWIFT_COLOR_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?:UIColor\s*\(\s*named:\s*["']|Color\s*\(\s*["'])([^"']+)["']"#).unwrap()
    });

    let swift_color_re = &*SWIFT_COLOR_RE;

    {
        let mut usage_stmt = tx.prepare_cached(
            "INSERT INTO ios_asset_usages (asset_id, usage_file, usage_line, usage_type) VALUES (?1, ?2, ?3, ?4)"
        )?;

        // Query swift files from DB instead of walking filesystem again
        let swift_rel_paths: Vec<String> = {
            let mut stmt = tx.prepare("SELECT path FROM files WHERE path LIKE '%.swift'")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.filter_map(|r| r.ok()).collect()
        };

        for rel_path in &swift_rel_paths {
            let file_path = root.join(rel_path);

            if let Ok(content) = fs::read_to_string(file_path) {
                for (line_num, line) in content.lines().enumerate() {
                    let line_num = line_num + 1;

                    // Image references
                    for caps in swift_image_re.captures_iter(line) {
                        let asset_name = caps.get(1).unwrap().as_str();
                        if let Some(&asset_id) = asset_ids.get(asset_name) {
                            usage_stmt.execute(rusqlite::params![
                                asset_id,
                                rel_path,
                                line_num as i64,
                                "code"
                            ])?;
                            usage_count += 1;
                        }
                    }

                    // Color references
                    for caps in swift_color_re.captures_iter(line) {
                        let asset_name = caps.get(1).unwrap().as_str();
                        if let Some(&asset_id) = asset_ids.get(asset_name) {
                            usage_stmt.execute(rusqlite::params![
                                asset_id,
                                rel_path,
                                line_num as i64,
                                "code"
                            ])?;
                            usage_count += 1;
                        }
                    }
                }
            }
        }
    }

    tx.commit()?;

    if progress {
        eprintln!("Indexed {} iOS assets, {} usages", asset_count, usage_count);
    }

    Ok((asset_count, usage_count))
}

/// Index CocoaPods and Carthage dependencies
pub fn index_ios_package_managers(conn: &Connection, root: &Path, progress: bool) -> Result<usize> {
    let mut count = 0;

    // CocoaPods: Podfile
    let podfile = root.join("Podfile");
    if podfile.exists() {
        if let Ok(content) = fs::read_to_string(&podfile) {
            // pod 'PodName', '~> 1.0'
            static POD_RE: LazyLock<Regex> =
                LazyLock::new(|| Regex::new(r#"pod\s+['"]([^'"]+)['"]"#).unwrap());

            let pod_re = &*POD_RE;

            for caps in pod_re.captures_iter(&content) {
                let pod_name = caps.get(1).unwrap().as_str();
                conn.execute(
                    "INSERT OR IGNORE INTO modules (name, path, kind) VALUES (?1, ?2, ?3)",
                    rusqlite::params![format!("pod.{}", pod_name), "Pods", "cocoapods"],
                )?;
                count += 1;
            }
        }
    }

    // Podfile.lock for exact versions
    let podfile_lock = root.join("Podfile.lock");
    if podfile_lock.exists() {
        if let Ok(content) = fs::read_to_string(&podfile_lock) {
            // PODS:
            //   - PodName (1.0.0)
            static POD_LOCK_RE: LazyLock<Regex> =
                LazyLock::new(|| Regex::new(r#"^\s+-\s+([A-Za-z0-9_-]+)\s+\("#).unwrap());

            let pod_lock_re = &*POD_LOCK_RE;

            for line in content.lines() {
                if let Some(caps) = pod_lock_re.captures(line) {
                    let pod_name = caps.get(1).unwrap().as_str();
                    conn.execute(
                        "INSERT OR IGNORE INTO modules (name, path, kind) VALUES (?1, ?2, ?3)",
                        rusqlite::params![format!("pod.{}", pod_name), "Pods", "cocoapods"],
                    )?;
                    count += 1;
                }
            }
        }
    }

    // Carthage: Cartfile
    let cartfile = root.join("Cartfile");
    if cartfile.exists() {
        if let Ok(content) = fs::read_to_string(&cartfile) {
            // github "owner/repo" ~> 1.0
            static CARTHAGE_RE: LazyLock<Regex> =
                LazyLock::new(|| Regex::new(r#"github\s+["']([^"']+)["']"#).unwrap());

            let carthage_re = &*CARTHAGE_RE;

            for caps in carthage_re.captures_iter(&content) {
                let repo = caps.get(1).unwrap().as_str();
                let name = repo.split('/').last().unwrap_or(repo);
                conn.execute(
                    "INSERT OR IGNORE INTO modules (name, path, kind) VALUES (?1, ?2, ?3)",
                    rusqlite::params![format!("carthage.{}", name), "Carthage/Build", "carthage"],
                )?;
                count += 1;
            }
        }
    }

    // Carthage.resolved for exact versions
    let cartfile_resolved = root.join("Cartfile.resolved");
    if cartfile_resolved.exists() {
        if let Ok(content) = fs::read_to_string(&cartfile_resolved) {
            static CARTHAGE_RE: LazyLock<Regex> =
                LazyLock::new(|| Regex::new(r#"github\s+["']([^"']+)["']"#).unwrap());

            let carthage_re = &*CARTHAGE_RE;

            for caps in carthage_re.captures_iter(&content) {
                let repo = caps.get(1).unwrap().as_str();
                let name = repo.split('/').last().unwrap_or(repo);
                conn.execute(
                    "INSERT OR IGNORE INTO modules (name, path, kind) VALUES (?1, ?2, ?3)",
                    rusqlite::params![format!("carthage.{}", name), "Carthage/Build", "carthage"],
                )?;
                count += 1;
            }
        }
    }

    if progress {
        eprintln!("Indexed {} CocoaPods/Carthage dependencies", count);
    }

    Ok(count)
}

fn collect_node_modules_dts_files(root: &Path) -> Vec<(PathBuf, String)> {
    use ignore::WalkBuilder;

    let node_modules = root.join("node_modules");
    if !node_modules.exists() || !node_modules.is_dir() {
        return Vec::new();
    }

    let verbose = std::env::var("AST_INDEX_VERBOSE").is_ok();

    // Collect (resolved_dir, node_modules_prefix) pairs.
    // Resolves symlinks only at the package level (safe for pnpm).
    // E.g.: (resolved_path, "node_modules/@types/react")
    let mut pkg_map: Vec<(PathBuf, String)> = Vec::new();

    if let Ok(entries) = fs::read_dir(&node_modules) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let name_str = entry.file_name().to_string_lossy().to_string();

            if name_str.starts_with('.') {
                continue;
            }

            if name_str.starts_with('@') {
                // Scoped packages: enumerate @scope/pkg
                let scope_dir = fs::canonicalize(&path).unwrap_or(path);
                if let Ok(scoped) = fs::read_dir(&scope_dir) {
                    for sub in scoped.filter_map(|e| e.ok()) {
                        let sub_name = sub.file_name().to_string_lossy().to_string();
                        let sub_resolved =
                            fs::canonicalize(sub.path()).unwrap_or_else(|_| sub.path());
                        if sub_resolved.is_dir() {
                            let prefix = format!("node_modules/{}/{}", name_str, sub_name);
                            pkg_map.push((sub_resolved, prefix));
                        }
                    }
                }
            } else {
                let resolved = fs::canonicalize(&path).unwrap_or(path);
                if resolved.is_dir() {
                    let prefix = format!("node_modules/{}", name_str);
                    pkg_map.push((resolved, prefix));
                }
            }
        }
    }

    if verbose {
        eprintln!(
            "[verbose] found {} package dirs in node_modules",
            pkg_map.len()
        );
    }

    // Walk each resolved package dir for .d.ts files.
    // follow_links=false — already resolved top-level symlinks.
    // Store (abs_path, rel_path) pairs for correct DB storage.
    let mut dts_files: Vec<(PathBuf, String)> = Vec::new();

    for (pkg_dir, nm_prefix) in &pkg_map {
        let mut builder = WalkBuilder::new(pkg_dir);
        builder
            .hidden(false)
            .git_ignore(false)
            .git_exclude(false)
            .follow_links(false)
            .max_depth(Some(8))
            .filter_entry(|entry| {
                if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                    if let Some(name) = entry.file_name().to_str() {
                        if name == "node_modules" || name.starts_with('.') {
                            return false;
                        }
                    }
                }
                true
            });

        for entry in builder.build().filter_map(|e| e.ok()) {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.ends_with(".d.ts") {
                    // Map resolved path back to node_modules/... relative path
                    let sub_path = path.strip_prefix(pkg_dir).unwrap_or(path).to_string_lossy();
                    let rel_path = if sub_path.is_empty() || sub_path == "." {
                        nm_prefix.clone()
                    } else {
                        format!("{}/{}", nm_prefix, sub_path)
                    };
                    dts_files.push((path.to_path_buf(), rel_path));
                }
            }
        }
    }

    dts_files
}

/// Index .d.ts files from node_modules (type declarations for external libraries).
/// These provide symbol definitions for imported libraries (e.g., React, lodash).
/// Only .d.ts files are indexed — not full JS/TS source from node_modules.
///
/// Handles pnpm (symlinks to store) by resolving top-level package symlinks
/// and mapping paths back to node_modules/... for storage.
/// Does NOT use follow_links to avoid loops on FUSE mounts (Arcadia).
pub fn index_node_modules_dts(conn: &mut Connection, root: &Path, progress: bool) -> Result<usize> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    let node_modules = root.join("node_modules");
    if !node_modules.exists() || !node_modules.is_dir() {
        return Ok(0);
    }

    if progress {
        eprintln!("Scanning node_modules for .d.ts type declarations...");
    }

    let walk_start = Instant::now();
    let verbose = std::env::var("AST_INDEX_VERBOSE").is_ok();
    let dts_files = collect_node_modules_dts_files(root);

    if dts_files.is_empty() {
        if verbose {
            eprintln!("[verbose] no .d.ts files found in node_modules");
        }
        return Ok(0);
    }

    if progress {
        eprintln!("Found {} .d.ts files in node_modules", dts_files.len());
    }
    if verbose {
        eprintln!(
            "[verbose] .d.ts walk completed in {:?}",
            walk_start.elapsed()
        );
    }

    // Parse in parallel and write to DB in chunks.
    // Uses parse_dts_file which takes an explicit rel_path (since real paths
    // may be in pnpm store, outside project root).
    let parsed_global = Arc::new(AtomicUsize::new(0));
    let total_files = dts_files.len();
    let chunk_size = effective_chunk_size(total_files);

    let num_threads = effective_num_threads();

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .stack_size(RAYON_WORKER_STACK_SIZE)
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build thread pool: {}", e))?;

    let mut total_count = 0;
    let root_path = db::normalize_root_for_storage(root);

    for chunk in dts_files.chunks(chunk_size) {
        let counter = parsed_global.clone();
        let total = total_files;
        let root_path = root_path.clone();

        let parsed_files: Vec<ParsedFile> = pool.install(|| {
            chunk
                .par_iter()
                .filter_map(|(abs_path, rel_path)| {
                    let result = parse_dts_file(abs_path, rel_path, &root_path).ok();
                    let c = counter.fetch_add(1, Ordering::Relaxed) + 1;
                    if progress && c % 1000 == 0 {
                        eprintln!("Parsed {} / {} .d.ts files...", c, total);
                    }
                    result
                })
                .collect()
        });

        write_batch_to_db(
            conn,
            parsed_files,
            &mut total_count,
            WriteMode::FreshRebuild,
        )?;
    }

    if progress {
        eprintln!("Indexed {} .d.ts files from node_modules", total_count);
    }

    Ok(total_count)
}

/// Parse a .d.ts file with an explicit relative path (for pnpm store paths)
fn parse_dts_file(file_path: &Path, rel_path: &str, root_path: &str) -> Result<ParsedFile> {
    let metadata = fs::metadata(file_path)?;
    let mtime = metadata
        .modified()?
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_secs() as i64;
    let size = metadata.len() as i64;

    if (size as u64) > max_file_size_bytes() {
        return Ok(ParsedFile {
            rel_path: rel_path.to_string(),
            root_path: root_path.to_string(),
            mtime,
            size,
            symbols: vec![],
            qualified_names: HashMap::new(),
            refs: vec![],
        });
    }

    let content = fs::read_to_string(file_path)?;
    let (symbols, refs) = parsers::parse_file_symbols(&content, parsers::FileType::TypeScript)?;

    Ok(ParsedFile {
        rel_path: rel_path.to_string(),
        root_path: root_path.to_string(),
        mtime,
        size,
        symbols,
        qualified_names: HashMap::new(),
        refs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_detect_android_project() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("settings.gradle.kts"), "").unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::Android);
    }

    #[test]
    fn test_detect_ios_project() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Package.swift"), "").unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::IOS);
    }

    #[test]
    fn test_detect_rust_project() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::Rust);
    }

    #[test]
    fn test_detect_python_project() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "").unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::Python);
    }

    #[test]
    fn test_detect_go_project() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("go.mod"), "").unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::Go);
    }

    #[test]
    fn test_detect_frontend_project() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::Frontend);
    }

    #[test]
    fn test_detect_perl_project() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("cpanfile"), "").unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::Perl);
    }

    #[test]
    fn test_detect_mixed_project() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::Mixed);
    }

    #[test]
    fn test_detect_bsl_project_by_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("module.bsl"), "").unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::Bsl);
    }

    #[test]
    fn test_detect_bsl_project_edt() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src/Configuration")).unwrap();
        fs::write(dir.path().join("src/Configuration/Configuration.mdo"), "").unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::Bsl);
    }

    #[test]
    fn test_detect_csharp_project() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("MyApp.sln"), "").unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::CSharp);
    }

    #[test]
    fn test_detect_csharp_project_csproj() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("MyApp.csproj"), "").unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::CSharp);
    }

    #[test]
    fn test_detect_cpp_project() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("CMakeLists.txt"), "").unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::Cpp);
    }

    #[test]
    fn test_detect_dart_project() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("pubspec.yaml"), "").unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::Dart);
    }

    #[test]
    fn test_detect_php_project() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("composer.json"), "{}").unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::PHP);
    }

    #[test]
    fn test_detect_ruby_project() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Gemfile"), "").unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::Ruby);
    }

    #[test]
    fn test_detect_ruby_project_gemspec() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("mylib.gemspec"), "").unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::Ruby);
    }

    #[test]
    fn test_detect_scala_project() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("build.sbt"), "").unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::Scala);
    }

    #[test]
    fn test_detect_unknown_project() {
        let dir = TempDir::new().unwrap();
        assert_eq!(detect_project_type(dir.path()), ProjectType::Unknown);
    }

    #[test]
    fn load_config_ignores_legacy_project_type_field() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(".ast-index.yaml"),
            r#"
project_type: dart
roots:
  - "../shared"
exclude:
  - "vendor"
include:
  - "src"
no_ignore: true
"#,
        )
        .unwrap();

        let config = load_config(dir.path()).expect("legacy config should still parse");
        assert_eq!(config.roots, Some(vec!["../shared".to_string()]));
        assert_eq!(config.exclude, Some(vec!["vendor".to_string()]));
        assert_eq!(config.include, Some(vec!["src".to_string()]));
        assert_eq!(config.no_ignore, Some(true));
    }

    #[test]
    fn test_excluded_dirs_contains_expected() {
        assert!(EXCLUDED_DIRS.contains(&"node_modules"));
        assert!(EXCLUDED_DIRS.contains(&"build"));
        assert!(EXCLUDED_DIRS.contains(&"target"));
        assert!(EXCLUDED_DIRS.contains(&"bazel-out"));
        assert!(EXCLUDED_DIRS.contains(&".gradle"));
        assert!(EXCLUDED_DIRS.contains(&"Pods"));
        assert!(EXCLUDED_DIRS.contains(&"DerivedData"));
    }

    #[test]
    fn test_parse_file_skips_large_files() {
        let dir = TempDir::new().unwrap();
        let large_file = dir.path().join("large.kt");
        let content = "a".repeat(1_100_000);
        fs::write(&large_file, &content).unwrap();

        let result = parse_file(dir.path(), &large_file).unwrap();
        assert!(result.symbols.is_empty(), "should skip large files");
        assert!(result.refs.is_empty());
    }

    #[test]
    fn test_parse_file_kotlin() {
        let dir = TempDir::new().unwrap();
        let kt_file = dir.path().join("Test.kt");
        fs::write(&kt_file, "class TestClass {\n    fun doSomething() {}\n}\n").unwrap();

        let result = parse_file(dir.path(), &kt_file).unwrap();
        assert!(result.symbols.iter().any(|s| s.name == "TestClass"));
        assert!(result.symbols.iter().any(|s| s.name == "doSomething"));
    }

    #[test]
    fn test_parse_file_swift() {
        let dir = TempDir::new().unwrap();
        let swift_file = dir.path().join("Test.swift");
        fs::write(
            &swift_file,
            "class MyView: UIView {\n    func setup() {}\n}\n",
        )
        .unwrap();

        let result = parse_file(dir.path(), &swift_file).unwrap();
        assert!(result.symbols.iter().any(|s| s.name == "MyView"));
        assert!(result.symbols.iter().any(|s| s.name == "setup"));
    }

    #[test]
    fn test_parse_file_python() {
        let dir = TempDir::new().unwrap();
        let py_file = dir.path().join("test.py");
        fs::write(
            &py_file,
            "class Service:\n    def process(self):\n        pass\n",
        )
        .unwrap();

        let result = parse_file(dir.path(), &py_file).unwrap();
        assert!(result.symbols.iter().any(|s| s.name == "Service"));
        assert!(result.symbols.iter().any(|s| s.name == "process"));
    }

    #[test]
    fn test_extract_py_list_strings() {
        let body = r#""foo>=1.0", "bar[extra]==2.0", 'baz; python_version>="3.8"'"#;
        let v = extract_py_list_strings(body);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], "foo>=1.0");
        assert_eq!(v[1], "bar[extra]==2.0");
    }

    #[test]
    fn test_strip_py_version() {
        assert_eq!(strip_py_version("foo"), "foo");
        assert_eq!(strip_py_version("foo>=1.0"), "foo");
        assert_eq!(strip_py_version("foo[extra]==1.0"), "foo");
        assert_eq!(strip_py_version("foo ~= 2.0"), "foo");
        assert_eq!(strip_py_version("foo ; python_version>='3.8'"), "foo");
    }

    #[test]
    fn test_index_modules_ya_make() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("library/cpp/foo")).unwrap();
        fs::create_dir_all(root.join("app/main")).unwrap();
        fs::write(root.join("library/cpp/foo/ya.make"), "LIBRARY()\nEND()\n").unwrap();
        fs::write(
            root.join("app/main/ya.make"),
            "PROGRAM()\nPEERDIR(\n    library/cpp/foo\n)\nEND()\n",
        )
        .unwrap();

        let conn = Connection::open_in_memory().unwrap();
        db::init_db(&conn).unwrap();

        let files = vec![
            root.join("library/cpp/foo/ya.make"),
            root.join("app/main/ya.make"),
        ];
        index_modules_from_files(&conn, root, &files).unwrap();

        let names: Vec<String> = conn
            .prepare("SELECT name FROM modules ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(names.contains(&"library/cpp/foo".to_string()));
        assert!(names.contains(&"app/main".to_string()));
    }

    #[test]
    fn test_index_deps_ya_make_peerdir() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("lib/a")).unwrap();
        fs::create_dir_all(root.join("lib/b")).unwrap();
        fs::create_dir_all(root.join("app")).unwrap();
        fs::write(root.join("lib/a/ya.make"), "LIBRARY()\nEND()\n").unwrap();
        fs::write(root.join("lib/b/ya.make"), "LIBRARY()\nEND()\n").unwrap();
        fs::write(
            root.join("app/ya.make"),
            "PROGRAM()\nPEERDIR(\n    lib/a\n    lib/b\n)\nEND()\n",
        )
        .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        db::init_db(&conn).unwrap();

        let files = vec![
            root.join("lib/a/ya.make"),
            root.join("lib/b/ya.make"),
            root.join("app/ya.make"),
        ];
        index_modules_from_files(&conn, root, &files).unwrap();
        let dep_count = index_module_dependencies(&mut conn, root, &files, false).unwrap();
        assert_eq!(dep_count, 2);

        let deps = get_module_deps(&conn, "app").unwrap();
        let dep_names: Vec<String> = deps.iter().map(|(n, _, _)| n.clone()).collect();
        assert!(dep_names.contains(&"lib/a".to_string()));
        assert!(dep_names.contains(&"lib/b".to_string()));
    }

    #[test]
    fn test_index_deps_gradle_standard_and_forma() {
        // Two consumer modules in one fixture:
        //   * feature/login uses the canonical Gradle `dependencies { implementation(project(...)) }`
        //   * feature/profile uses the Forma-style `androidLibrary(dependencies = deps(...) + deps(project(...)))`
        // Each consumer also lists external accessors (google.material, androidx.appcompat,
        // test.junit, test.espresso) to confirm the regex does not false-match non-project entries.
        // module_deps has no UNIQUE constraint, so the regex must produce exactly one edge per
        // declaration — a previous version with two overlapping patterns silently doubled
        // standard-form edges.
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        for sub in &[
            "core/network",
            "core/database",
            "feature/login",
            "feature/profile",
        ] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        // Leaf targets — empty build files so they only register as modules.
        fs::write(root.join("core/network/build.gradle.kts"), "").unwrap();
        fs::write(root.join("core/database/build.gradle.kts"), "").unwrap();

        // Standard Gradle consumer.
        fs::write(
            root.join("feature/login/build.gradle.kts"),
            r#"
            plugins {
                id("com.android.library")
                kotlin("android")
            }
            dependencies {
                implementation(project(":core:network"))
                implementation("androidx.appcompat:appcompat:1.6.1")
                testImplementation("junit:junit:4.13.2")
            }
            "#,
        )
        .unwrap();

        // Forma DSL consumer — mirrors the syntax shown in the Forma README
        // (https://github.com/formatools/forma): `dependencies = deps(...) + deps(project(...))`,
        // plus testDependencies/androidTestDependencies.
        fs::write(
            root.join("feature/profile/build.gradle.kts"),
            r#"
            androidLibrary(
                packageName = "tools.forma.sample.profile",
                dependencies = deps(
                    google.material,
                    androidx.appcompat,
                ) + deps(
                    project(":core:database"),
                ),
                testDependencies = deps(
                    test.junit,
                ),
                androidTestDependencies = deps(
                    test.espresso,
                ),
            )
            "#,
        )
        .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        db::init_db(&conn).unwrap();

        let files = vec![
            root.join("core/network/build.gradle.kts"),
            root.join("core/database/build.gradle.kts"),
            root.join("feature/login/build.gradle.kts"),
            root.join("feature/profile/build.gradle.kts"),
        ];
        index_modules_from_files(&conn, root, &files).unwrap();
        let dep_count = index_module_dependencies(&mut conn, root, &files, false).unwrap();

        // feature.login — exactly one internal edge to core.network via standard Gradle DSL.
        let login_deps = get_module_deps(&conn, "feature.login").unwrap();
        let login_names: Vec<&str> = login_deps.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(
            login_names,
            vec!["core.network"],
            "feature.login: expected only [core.network], got {:?}",
            login_names
        );
        assert_eq!(
            login_deps[0].2, "implementation",
            "feature.login dep_kind mismatch: {:?}",
            login_deps[0]
        );

        // feature.profile — exactly one internal edge to core.database via Forma deps(project(...)).
        // External accessors (google.material, androidx.appcompat, test.junit, test.espresso)
        // must not appear; they have no `project(...)` wrapper and no matching module exists.
        let profile_deps = get_module_deps(&conn, "feature.profile").unwrap();
        let profile_names: Vec<&str> = profile_deps.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(
            profile_names,
            vec!["core.database"],
            "feature.profile: expected only [core.database], got {:?}",
            profile_names
        );

        // Two consumers × one internal dep each = 2 total edges, with no duplicates.
        assert_eq!(dep_count, 2, "expected dep_count == 2, got {}", dep_count);
        let total_edges: i64 = conn
            .query_row("SELECT COUNT(*) FROM module_deps", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            total_edges, 2,
            "module_deps row count mismatch — duplicate edge inserted?"
        );
    }

    #[test]
    fn test_index_deps_gradle_forma_multi_project_per_block() {
        // Real-world Forma layout: a single `deps(...)` block declares many `project(...)` entries
        // separated by other deps and newlines. The wrapper-anchored regex
        // `\b(\w+)\s*\(\s*project\s*\(` only fires once per `deps(` (on the first project),
        // so without the project-only fallback the second and third project edges are silently
        // dropped — manifesting as a huge undercount on `ast-index dependents`.
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        for sub in &[
            "api/callback",
            "api/dto-common",
            "api/third",
            "feature/payments",
        ] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        fs::write(root.join("api/callback/build.gradle.kts"), "").unwrap();
        fs::write(root.join("api/dto-common/build.gradle.kts"), "").unwrap();
        fs::write(root.join("api/third/build.gradle.kts"), "").unwrap();

        fs::write(
            root.join("feature/payments/build.gradle.kts"),
            r#"
            androidLibrary(
                packageName = "tools.forma.sample.payments",
                dependencies = deps(
                    aar(Deps.Files.tapandpay),
                    aar(Deps.Files.saverification),
                ) + deps(
                    Deps.Libraries.rxJava,
                    Deps.Libraries.rxKotlin,
                ) + deps(
                    project(":api:callback"),
                    project(":api:dto-common"),
                    project(":api:third"),
                ),
            )
            "#,
        )
        .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        db::init_db(&conn).unwrap();

        let files = vec![
            root.join("api/callback/build.gradle.kts"),
            root.join("api/dto-common/build.gradle.kts"),
            root.join("api/third/build.gradle.kts"),
            root.join("feature/payments/build.gradle.kts"),
        ];
        index_modules_from_files(&conn, root, &files).unwrap();
        let dep_count = index_module_dependencies(&mut conn, root, &files, false).unwrap();

        let payments_deps = get_module_deps(&conn, "feature.payments").unwrap();
        let mut payments_names: Vec<&str> =
            payments_deps.iter().map(|(n, _, _)| n.as_str()).collect();
        payments_names.sort();
        assert_eq!(
            payments_names,
            vec!["api.callback", "api.dto-common", "api.third"],
            "feature.payments: expected all three project() edges, got {:?}",
            payments_names
        );
        assert_eq!(dep_count, 3, "expected dep_count == 3, got {}", dep_count);

        let total_edges: i64 = conn
            .query_row("SELECT COUNT(*) FROM module_deps", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            total_edges, 3,
            "module_deps row count mismatch — duplicate or missing edge"
        );
    }

    #[test]
    fn test_index_deps_gradle_project_in_comments_or_strings_is_ignored() {
        // The unanchored project-only fallback must NOT fire on `project("...")` text
        // outside a `dependencies = wrapper(...)` block: line comments, string literals,
        // or unrelated code. Otherwise an indexed module with a matching name produces
        // a phantom edge that silently inflates `ast-index dependents` output.
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        for sub in &["api/foo", "api/bar", "feature/consumer"] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        fs::write(root.join("api/foo/build.gradle.kts"), "").unwrap();
        fs::write(root.join("api/bar/build.gradle.kts"), "").unwrap();

        // Real dep: api.foo. Decoys: api.bar referenced only in a comment / string.
        fs::write(
            root.join("feature/consumer/build.gradle.kts"),
            r#"
            // Earlier draft used project(":api:bar") — kept as a note.
            val sample = "project(\":api:bar\")"
            androidLibrary(
                packageName = "tools.forma.sample.consumer",
                dependencies = deps(
                    project(":api:foo"),
                ),
            )
            // Trailing TODO: bring back project(":api:bar")
            "#,
        )
        .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        db::init_db(&conn).unwrap();
        let files = vec![
            root.join("api/foo/build.gradle.kts"),
            root.join("api/bar/build.gradle.kts"),
            root.join("feature/consumer/build.gradle.kts"),
        ];
        index_modules_from_files(&conn, root, &files).unwrap();
        let dep_count = index_module_dependencies(&mut conn, root, &files, false).unwrap();

        let consumer_deps = get_module_deps(&conn, "feature.consumer").unwrap();
        let names: Vec<&str> = consumer_deps.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec!["api.foo"],
            "feature.consumer must not pick up api.bar from comments/strings, got {:?}",
            names
        );
        assert_eq!(dep_count, 1, "expected exactly one edge, got {}", dep_count);
    }

    #[test]
    fn test_index_deps_python_pyproject() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("libs/shared")).unwrap();
        fs::create_dir_all(root.join("services/api")).unwrap();
        fs::write(
            root.join("libs/shared/pyproject.toml"),
            "[project]\nname = \"shared\"\n",
        )
        .unwrap();
        fs::write(
            root.join("services/api/pyproject.toml"),
            "[project]\nname = \"api\"\ndependencies = [\n  \"libs.shared>=1.0\",\n  \"requests>=2.0\",\n]\n",
        )
        .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        db::init_db(&conn).unwrap();

        let files = vec![
            root.join("libs/shared/pyproject.toml"),
            root.join("services/api/pyproject.toml"),
        ];
        index_modules_from_files(&conn, root, &files).unwrap();
        let dep_count = index_module_dependencies(&mut conn, root, &files, false).unwrap();
        // Only the internal dep (libs.shared) should be matched; "requests" is external
        assert_eq!(dep_count, 1);

        let deps = get_module_deps(&conn, "services.api").unwrap();
        let dep_names: Vec<String> = deps.iter().map(|(n, _, _)| n.clone()).collect();
        assert!(dep_names.contains(&"libs.shared".to_string()));
    }

    #[test]
    fn test_index_deps_python_poetry() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("libs/core")).unwrap();
        fs::create_dir_all(root.join("app")).unwrap();
        fs::write(
            root.join("libs/core/pyproject.toml"),
            "[project]\nname = \"core\"\n",
        )
        .unwrap();
        fs::write(
            root.join("app/pyproject.toml"),
            "[tool.poetry]\nname = \"app\"\n\n[tool.poetry.dependencies]\npython = \"^3.10\"\n\"libs.core\" = \"^1.0\"\nexternal = \"^2.0\"\n",
        )
        .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        db::init_db(&conn).unwrap();

        let files = vec![
            root.join("libs/core/pyproject.toml"),
            root.join("app/pyproject.toml"),
        ];
        index_modules_from_files(&conn, root, &files).unwrap();
        let dep_count = index_module_dependencies(&mut conn, root, &files, false).unwrap();
        assert_eq!(dep_count, 1);

        let deps = get_module_deps(&conn, "app").unwrap();
        assert!(deps.iter().any(|(n, _, _)| n == "libs.core"));
    }

    #[test]
    fn walk_error_summary_zero_files_does_not_fail() {
        let mut summary = WalkErrorSummary::default();
        summary.record_message("Permission denied (os error 13)".to_string());

        summary.finish(Path::new("/repo"), 0, false, false);
    }

    #[test]
    fn walk_error_summary_allows_partial_success() {
        let mut summary = WalkErrorSummary::default();
        summary.record_message("Permission denied (os error 13)".to_string());

        summary.finish(Path::new("/repo"), 1, false, false);
    }

    #[test]
    fn sample_parseable_files_without_ignore_finds_sources() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join(".gitignore"), "src/\n").unwrap();
        fs::write(root.join("src/Main.java"), "class Main {}\n").unwrap();

        let samples = sample_parseable_files_without_ignore(root, 5);
        assert_eq!(samples.len(), 1);
        assert!(samples[0].ends_with("src/Main.java"));
    }
}
