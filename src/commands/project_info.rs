//! Project insight commands
//!
//! - map: Compact project map (key types per directory)
//! - conventions: Auto-detect project conventions (architecture, frameworks, naming)

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use colored::Colorize;
use rusqlite::params;
use serde::Serialize;

use crate::db;

// ── map ──────────────────────────────────────────────────────────────

// --- Summary mode structs (default, no --module) ---

#[derive(Debug, Serialize)]
struct MapSummaryOutput {
    project_type: String,
    file_count: i64,
    module_count: i64,
    showing: usize,
    total_dirs: usize,
    groups: Vec<SummaryGroup>,
}

#[derive(Debug, Serialize)]
struct SummaryGroup {
    path: String,
    file_count: i64,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    kinds: HashMap<String, i64>,
}

// --- Detailed mode structs (with --module) ---

#[derive(Debug, Serialize)]
struct MapDetailOutput {
    project_type: String,
    file_count: i64,
    module_count: i64,
    groups: Vec<DetailGroup>,
}

#[derive(Debug, Serialize)]
struct DetailGroup {
    path: String,
    file_count: i64,
    symbols: Vec<MapSymbol>,
}

#[derive(Debug, Serialize)]
struct MapSymbol {
    name: String,
    kind: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    parents: Vec<String>,
    file: String,
}

/// Kind priority for sorting: lower = more important
fn kind_priority(kind: &str) -> u8 {
    match kind {
        "class" => 0,
        "interface" | "protocol" | "trait" => 1,
        "struct" => 2,
        "enum" => 3,
        "object" => 4,
        "actor" => 5,
        _ => 10,
    }
}

/// Short display label for kind
fn kind_label(kind: &str) -> &str {
    match kind {
        "class" => "cls",
        "interface" => "iface",
        "protocol" => "proto",
        "trait" => "trait",
        "struct" => "struct",
        "enum" => "enum",
        "object" => "obj",
        "actor" => "actor",
        "package" => "pkg",
        _ => kind,
    }
}

/// Detect project type from file extensions in the DB
fn detect_project_type(conn: &rusqlite::Connection) -> String {
    let mut ext_counts: Vec<(String, i64)> = Vec::new();
    let mut stmt = conn
        .prepare(
            r#"
            SELECT
                CASE
                    WHEN path LIKE '%.kt' THEN 'kt'
                    WHEN path LIKE '%.java' THEN 'java'
                    WHEN path LIKE '%.swift' THEN 'swift'
                    WHEN path LIKE '%.m' OR path LIKE '%.mm' THEN 'objc'
                    WHEN path LIKE '%.dart' THEN 'dart'
                    WHEN path LIKE '%.ts' OR path LIKE '%.tsx' THEN 'ts'
                    WHEN path LIKE '%.js' OR path LIKE '%.jsx' THEN 'js'
                    WHEN path LIKE '%.py' THEN 'py'
                    WHEN path LIKE '%.go' THEN 'go'
                    WHEN path LIKE '%.rs' THEN 'rs'
                    WHEN path LIKE '%.rb' THEN 'rb'
                    WHEN path LIKE '%.cs' THEN 'cs'
                    WHEN path LIKE '%.scala' THEN 'scala'
                    WHEN path LIKE '%.cpp' OR path LIKE '%.cc' OR path LIKE '%.cxx' THEN 'cpp'
                    WHEN path LIKE '%.pl' OR path LIKE '%.pm' THEN 'perl'
                    WHEN path LIKE '%.proto' THEN 'proto'
                    ELSE 'other'
                END AS ext,
                COUNT(*) AS cnt
            FROM files
            GROUP BY ext
            ORDER BY cnt DESC
            "#,
        )
        .unwrap();

    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))
        .unwrap();

    for r in rows.flatten() {
        ext_counts.push(r);
    }

    let labels: Vec<String> = ext_counts
        .iter()
        .filter(|(e, _)| e != "other")
        .take(2)
        .map(|(ext, _)| match ext.as_str() {
            "kt" => "Kotlin".into(),
            "java" => "Java".into(),
            "swift" => "Swift".into(),
            "objc" => "ObjC".into(),
            "dart" => "Dart".into(),
            "ts" => "TypeScript".into(),
            "js" => "JavaScript".into(),
            "py" => "Python".into(),
            "go" => "Go".into(),
            "rs" => "Rust".into(),
            "rb" => "Ruby".into(),
            "cs" => "C#".into(),
            "scala" => "Scala".into(),
            "cpp" => "C++".into(),
            "perl" => "Perl".into(),
            "proto" => "Protobuf".into(),
            other => other.to_string(),
        })
        .collect();

    let top = ext_counts.first().map(|(e, _)| e.as_str()).unwrap_or("");
    let platform = match top {
        "kt" | "java" => "Android",
        "swift" | "objc" => "iOS",
        "dart" => "Flutter",
        _ => "",
    };

    if platform.is_empty() {
        labels.join("/")
    } else {
        format!("{} ({})", platform, labels.join("/"))
    }
}

/// Truncate path to first N segments
fn dir_prefix(path: &str, depth: usize) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= depth + 1 {
        parts[..parts.len().saturating_sub(1)].join("/")
    } else {
        parts[..depth].join("/")
    }
}

pub fn cmd_map(
    root: &Path,
    module: Option<&str>,
    per_dir: usize,
    limit: usize,
    format: &str,
) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!("{}", "Index not found. Run 'ast-index rebuild' first.".red());
        return Ok(());
    }

    let conn = db::open_db(root)?;
    let stats = db::get_stats(&conn)?;
    let project_type = detect_project_type(&conn);

    let depth = if stats.file_count > 5000 { 3 } else { 2 };

    if module.is_some() {
        cmd_map_detailed(&conn, &project_type, &stats, module, per_dir, limit, depth, format)?;
    } else {
        cmd_map_summary(&conn, &project_type, &stats, limit, depth, format)?;
    }

    eprintln!("{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Summary mode: directories + file counts + kind counts, sorted by file_count desc
fn cmd_map_summary(
    conn: &rusqlite::Connection,
    project_type: &str,
    stats: &db::DbStats,
    limit: usize,
    depth: usize,
    format: &str,
) -> Result<()> {
    // Count files per directory
    let mut dir_file_counts: HashMap<String, i64> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT path FROM files")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for path in rows.flatten() {
            let dir = dir_prefix(&path, depth);
            *dir_file_counts.entry(dir).or_insert(0) += 1;
        }
    }

    // Count symbols by kind per directory
    let mut dir_kind_counts: HashMap<String, HashMap<String, i64>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            r#"
            SELECT f.path, s.kind
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.parent_id IS NULL
              AND s.kind IN ('class','interface','struct','enum','object','protocol','trait','actor','package')
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows.flatten() {
            let dir = dir_prefix(&row.0, depth);
            *dir_kind_counts.entry(dir).or_default().entry(row.1).or_insert(0) += 1;
        }
    }

    // Build groups, sort by file_count desc
    let mut groups: Vec<SummaryGroup> = dir_file_counts
        .into_iter()
        .map(|(dir, fc)| {
            let kinds = dir_kind_counts.remove(&dir).unwrap_or_default();
            SummaryGroup {
                path: if dir.is_empty() { ".".to_string() } else { format!("{}/", dir) },
                file_count: fc,
                kinds,
            }
        })
        .collect();

    groups.sort_by(|a, b| b.file_count.cmp(&a.file_count));
    let total_dirs = groups.len();
    groups.truncate(limit);

    if format == "json" {
        let output = MapSummaryOutput {
            project_type: project_type.to_string(),
            file_count: stats.file_count,
            module_count: stats.module_count,
            showing: groups.len(),
            total_dirs,
            groups,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    // Text output
    println!(
        "{}",
        format!(
            "Project: {} | {} files | {} modules | top {} of {} dirs",
            project_type, stats.file_count, stats.module_count, groups.len(), total_dirs
        )
        .bold()
    );
    println!();

    for g in &groups {
        // Build compact kind summary: "12 cls, 3 iface, 2 enum"
        let mut kind_pairs: Vec<(&str, i64)> = g.kinds.iter()
            .map(|(k, &v)| (kind_label(k), v))
            .collect();
        kind_pairs.sort_by(|a, b| kind_priority(a.0).cmp(&kind_priority(b.0)));

        let kinds_str = if kind_pairs.is_empty() {
            String::new()
        } else {
            let items: Vec<String> = kind_pairs.iter()
                .map(|(k, v)| format!("{} {}", v, k))
                .collect();
            format!(" | {}", items.join(", "))
        };

        println!(
            "  {:60} {:>5} files{}",
            g.path.cyan(),
            g.file_count,
            kinds_str,
        );
    }

    if total_dirs > groups.len() {
        println!(
            "\n{}",
            format!(
                "  ... and {} more dirs. Use --limit or --module <path> to drill down.",
                total_dirs - groups.len()
            )
            .dimmed()
        );
    }

    Ok(())
}

/// Detailed mode: symbols with inheritance per directory (when --module is used)
fn cmd_map_detailed(
    conn: &rusqlite::Connection,
    project_type: &str,
    stats: &db::DbStats,
    module: Option<&str>,
    per_dir: usize,
    limit: usize,
    depth: usize,
    format: &str,
) -> Result<()> {
    let module_filter = module.map(|m| format!("{}%", m));
    let sql = if module_filter.is_some() {
        r#"
        SELECT s.name, s.kind, s.line, f.path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE s.parent_id IS NULL
          AND s.kind IN ('class','interface','struct','enum','object','protocol','trait','actor','package')
          AND f.path LIKE ?1
        ORDER BY f.path, s.line
        "#
    } else {
        r#"
        SELECT s.name, s.kind, s.line, f.path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE s.parent_id IS NULL
          AND s.kind IN ('class','interface','struct','enum','object','protocol','trait','actor','package')
        ORDER BY f.path, s.line
        "#
    };

    let mut stmt = conn.prepare(sql)?;

    struct RawSym {
        name: String,
        kind: String,
        path: String,
    }

    let rows: Vec<RawSym> = if let Some(ref mf) = module_filter {
        stmt.query_map(params![mf], |row| {
            Ok(RawSym {
                name: row.get(0)?,
                kind: row.get(1)?,
                path: row.get(3)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect()
    } else {
        stmt.query_map([], |row| {
            Ok(RawSym {
                name: row.get(0)?,
                kind: row.get(1)?,
                path: row.get(3)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect()
    };

    // Batch-load inheritance (deduplicated)
    let mut inheritance_map: HashMap<String, Vec<String>> = HashMap::new();
    {
        let inh_sql = if module_filter.is_some() {
            r#"
            SELECT DISTINCT s.name, i.parent_name
            FROM inheritance i
            JOIN symbols s ON i.child_id = s.id
            JOIN files f ON s.file_id = f.id
            WHERE s.parent_id IS NULL AND f.path LIKE ?1
            ORDER BY s.name
            "#
        } else {
            r#"
            SELECT DISTINCT s.name, i.parent_name
            FROM inheritance i
            JOIN symbols s ON i.child_id = s.id
            WHERE s.parent_id IS NULL
            ORDER BY s.name
            "#
        };
        let mut inh_stmt = conn.prepare(inh_sql)?;
        let inh_rows = if let Some(ref mf) = module_filter {
            inh_stmt.query_map(params![mf], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect::<Vec<_>>()
        } else {
            inh_stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect::<Vec<_>>()
        };
        for (name, parent) in inh_rows {
            let parents = inheritance_map.entry(name).or_default();
            if !parents.contains(&parent) {
                parents.push(parent);
            }
        }
    }

    // Group by directory prefix
    let mut groups_map: HashMap<String, Vec<&RawSym>> = HashMap::new();
    for sym in &rows {
        let dir = dir_prefix(&sym.path, depth);
        groups_map.entry(dir).or_default().push(sym);
    }

    // Count files per directory (filtered if module)
    let mut dir_file_counts: HashMap<String, i64> = HashMap::new();
    {
        let fc_sql = if module_filter.is_some() {
            "SELECT path FROM files WHERE path LIKE ?1"
        } else {
            "SELECT path FROM files"
        };
        let mut fc_stmt = conn.prepare(fc_sql)?;
        let file_rows: Vec<String> = if let Some(ref mf) = module_filter {
            fc_stmt.query_map(params![mf], |row| row.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .collect()
        } else {
            fc_stmt.query_map([], |row| row.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .collect()
        };
        for path in &file_rows {
            let dir = dir_prefix(path, depth);
            *dir_file_counts.entry(dir).or_insert(0) += 1;
        }
    }

    // Build groups sorted by file_count desc, apply limit
    let mut dir_keys: Vec<String> = groups_map.keys().cloned().collect();
    dir_keys.sort_by(|a, b| {
        let fa = dir_file_counts.get(a).copied().unwrap_or(0);
        let fb = dir_file_counts.get(b).copied().unwrap_or(0);
        fb.cmp(&fa)
    });
    dir_keys.truncate(limit);

    let mut groups: Vec<DetailGroup> = Vec::new();
    for dir in &dir_keys {
        let syms = &groups_map[dir];
        let mut sorted: Vec<&RawSym> = syms.clone();
        sorted.sort_by(|a, b| {
            kind_priority(&a.kind)
                .cmp(&kind_priority(&b.kind))
                .then(a.name.cmp(&b.name))
        });
        sorted.truncate(per_dir);

        let map_syms: Vec<MapSymbol> = sorted
            .iter()
            .map(|s| {
                let parents = inheritance_map
                    .get(&s.name)
                    .cloned()
                    .unwrap_or_default();
                let file = s
                    .path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&s.path)
                    .to_string();
                MapSymbol {
                    name: s.name.clone(),
                    kind: s.kind.clone(),
                    parents,
                    file,
                }
            })
            .collect();

        let fc = dir_file_counts.get(dir).copied().unwrap_or(0);
        groups.push(DetailGroup {
            path: if dir.is_empty() {
                ".".to_string()
            } else {
                format!("{}/", dir)
            },
            file_count: fc,
            symbols: map_syms,
        });
    }

    if format == "json" {
        let output = MapDetailOutput {
            project_type: project_type.to_string(),
            file_count: stats.file_count,
            module_count: stats.module_count,
            groups,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    // Text output
    println!(
        "{}",
        format!(
            "Project: {} | {} files | {} modules",
            project_type, stats.file_count, stats.module_count
        )
        .bold()
    );
    println!();

    for g in &groups {
        if g.symbols.is_empty() {
            continue;
        }
        println!(
            "{} ({} files)",
            g.path.cyan(),
            g.file_count
        );
        for s in &g.symbols {
            let parents_str = if s.parents.is_empty() {
                String::new()
            } else {
                format!(" > {}", s.parents.join(", "))
            };
            println!(
                "  {} : {}{}",
                s.name.yellow(),
                s.kind,
                parents_str
            );
        }
        println!();
    }

    Ok(())
}

// ── conventions ──────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct ConventionsOutput {
    architecture: Vec<String>,
    frameworks: HashMap<String, Vec<FrameworkHit>>,
    naming_patterns: Vec<NamingPattern>,
}

#[derive(Debug, Serialize)]
struct FrameworkHit {
    name: String,
    count: i64,
}

#[derive(Debug, Serialize)]
struct NamingPattern {
    suffix: String,
    count: i64,
}

/// Known suffix patterns to look for
const NAMING_SUFFIXES: &[&str] = &[
    "ViewModel", "Repository", "UseCase", "Service", "Controller",
    "Interactor", "Presenter", "Factory", "Mapper", "Provider",
    "Manager", "Handler", "Adapter", "Delegate", "Store", "Reducer",
    "Component", "Fragment", "Activity", "Screen", "View", "Widget",
    "Bloc", "Cubit", "Test", "Spec", "Module", "Router", "Navigator",
    "Middleware", "Interceptor", "Gateway",
];

/// Known import prefixes → (category, display_name)
const FRAMEWORK_RULES: &[(&str, &str, &str)] = &[
    // DI
    ("dagger", "DI", "Dagger"),
    ("hilt", "DI", "Hilt"),
    ("koin", "DI", "Koin"),
    ("kodein", "DI", "Kodein"),
    ("javax.inject", "DI", "javax.inject"),
    ("com.google.inject", "DI", "Guice"),
    ("org.springframework.beans", "DI", "Spring"),
    ("org.springframework.context", "DI", "Spring"),
    // Async
    ("kotlinx.coroutines", "Async", "Coroutines"),
    ("io.reactivex", "Async", "RxJava"),
    ("rx.", "Async", "Rx"),
    ("combine", "Async", "Combine"),
    ("kotlinx.coroutines.flow", "Async", "Flow"),
    // Network
    ("retrofit", "Network", "Retrofit"),
    ("okhttp", "Network", "OkHttp"),
    ("alamofire", "Network", "Alamofire"),
    ("ktor", "Network", "Ktor"),
    // DB
    ("androidx.room", "DB", "Room"),
    ("io.realm", "DB", "Realm"),
    ("app.cash.sqldelight", "DB", "SQLDelight"),
    ("coredata", "DB", "CoreData"),
    // UI
    ("androidx.compose", "UI", "Jetpack Compose"),
    ("swiftui", "UI", "SwiftUI"),
    ("react", "UI", "React"),
    ("vue", "UI", "Vue"),
    ("svelte", "UI", "Svelte"),
    ("flutter", "UI", "Flutter"),
    // Testing
    ("org.junit", "Testing", "JUnit"),
    ("io.kotest", "Testing", "Kotest"),
    ("xctest", "Testing", "XCTest"),
    ("pytest", "Testing", "pytest"),
    ("jest", "Testing", "Jest"),
    ("rspec", "Testing", "RSpec"),
    ("testing", "Testing", "testing"),
    ("org.mockito", "Testing", "Mockito"),
    ("io.mockk", "Testing", "MockK"),
    // Serialization
    ("kotlinx.serialization", "Serialization", "kotlinx.serialization"),
    ("com.google.gson", "Serialization", "Gson"),
    ("com.squareup.moshi", "Serialization", "Moshi"),
    ("com.fasterxml.jackson", "Serialization", "Jackson"),
];

/// Architecture detection patterns (path-based)
const ARCH_PATTERNS: &[(&[&str], &str)] = &[
    (&["/presentation/", "/domain/", "/data/"], "Clean Architecture"),
    (&["/feature/"], "Feature-sliced"),
    (&["/features/"], "Feature-sliced"),
    (&["/bloc/", "/state/", "/event/"], "BLoC"),
    (&["/views/", "/controllers/"], "MVC"),
    (&["/viewmodel/", "/view/", "/model/"], "MVVM"),
    (&["/presenter/"], "MVP"),
    (&["/reducers/", "/actions/", "/store/"], "Redux"),
    (&["/composables/"], "Composition API"),
    (&["/hooks/"], "Hooks pattern"),
];

pub fn cmd_conventions(root: &Path, format: &str) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!("{}", "Index not found. Run 'ast-index rebuild' first.".red());
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // A. Naming patterns — suffix counts from symbols
    let mut naming: Vec<NamingPattern> = Vec::new();
    {
        let mut stmt = conn.prepare(
            r#"
            SELECT COUNT(*) FROM symbols
            WHERE kind IN ('class','interface','struct','enum','object','protocol','trait','actor')
              AND name LIKE ?1
            "#,
        )?;
        for &suffix in NAMING_SUFFIXES {
            let pattern = format!("%{}", suffix);
            let count: i64 = stmt.query_row(params![pattern], |row| row.get(0))?;
            if count >= 3 {
                naming.push(NamingPattern {
                    suffix: suffix.to_string(),
                    count,
                });
            }
        }
    }
    naming.sort_by(|a, b| b.count.cmp(&a.count));

    // B. Frameworks — from refs WHERE context LIKE 'import%'
    let mut fw_map: HashMap<String, HashMap<String, i64>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            r#"
            SELECT name, COUNT(*) as cnt FROM (
                SELECT name FROM refs WHERE context LIKE 'import%'
                UNION ALL
                SELECT name FROM symbols WHERE kind = 'import'
            )
            GROUP BY name
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;

        for row in rows.flatten() {
            let (import_name, cnt) = row;
            let lower = import_name.to_lowercase();
            for &(prefix, category, display) in FRAMEWORK_RULES {
                if lower.contains(prefix) {
                    let cat_map = fw_map.entry(category.to_string()).or_default();
                    *cat_map.entry(display.to_string()).or_insert(0) += cnt;
                    break;
                }
            }
        }
    }

    // Convert to sorted output
    let mut frameworks: HashMap<String, Vec<FrameworkHit>> = HashMap::new();
    for (cat, hits) in &fw_map {
        let mut sorted: Vec<FrameworkHit> = hits
            .iter()
            .map(|(name, &count)| FrameworkHit {
                name: name.clone(),
                count,
            })
            .collect();
        sorted.sort_by(|a, b| b.count.cmp(&a.count));
        frameworks.insert(cat.clone(), sorted);
    }

    // C. Architecture detection from file paths
    let mut arch: Vec<String> = Vec::new();
    {
        let mut path_stmt = conn.prepare("SELECT path FROM files LIMIT 50000")?;
        let paths: Vec<String> = path_stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();

        let lower_paths: Vec<String> = paths.iter().map(|p| format!("/{}/", p.to_lowercase())).collect();

        for &(markers, label) in ARCH_PATTERNS {
            if arch.contains(&label.to_string()) {
                continue;
            }
            let all_found = markers.iter().all(|marker| {
                lower_paths.iter().any(|p| p.contains(marker))
            });
            if all_found {
                arch.push(label.to_string());
            }
        }
    }

    // Output
    if format == "json" {
        let output = ConventionsOutput {
            architecture: arch,
            frameworks,
            naming_patterns: naming,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    // Text output
    println!("{}", "Project Conventions:".bold());
    println!();

    if !arch.is_empty() {
        println!("{} {}", "Architecture:".cyan(), arch.join(", "));
        println!();
    }

    let mut cats: Vec<&String> = frameworks.keys().collect();
    cats.sort();
    for cat in cats {
        let hits = &frameworks[cat];
        let items: Vec<String> = hits
            .iter()
            .map(|h| format!("{} ({})", h.name, h.count))
            .collect();
        println!("{} {}", format!("{}:", cat).cyan(), items.join(", "));
    }
    if !frameworks.is_empty() {
        println!();
    }

    if !naming.is_empty() {
        println!("{}", "Naming Patterns:".cyan());
        for np in &naming {
            println!("  {:20} {}", np.suffix, np.count);
        }
        println!();
    }

    eprintln!("{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}
