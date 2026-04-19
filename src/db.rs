#![allow(dead_code)]

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::fs::File;
use std::path::{Path, PathBuf};

/// Normalize project root path: canonicalize if possible, fallback to original.
/// This ensures the same DB is found after VFS remount (e.g. arc mount in Arcadia).
fn normalize_root(project_root: &Path) -> String {
    project_root.canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Get the database path for the current project
pub fn get_db_path(project_root: &Path) -> Result<PathBuf> {
    // Check env: new name first, fallback to old
    if let Ok(path) = std::env::var("AST_INDEX_DB_PATH")
        .or_else(|_| std::env::var("KOTLIN_INDEX_DB_PATH"))
    {
        return Ok(PathBuf::from(path));
    }

    let cache_dir = dirs::cache_dir()
        .context("Could not find cache directory")?
        .join("ast-index");

    let normalized = normalize_root(project_root);

    // Create hash from normalized project root for unique DB per project
    let project_hash = simple_hash(&normalized);
    let db_dir = cache_dir.join(&project_hash);

    // Also compute hash from raw path (for migration from pre-normalize DBs)
    let raw_hash = simple_hash(project_root.to_string_lossy().as_ref());
    let raw_dir = cache_dir.join(&raw_hash);

    // Migrate from raw-path hash to normalized hash if needed
    if !db_dir.join("index.db").exists() && raw_hash != project_hash && raw_dir.join("index.db").exists() {
        let _ = std::fs::create_dir_all(&db_dir);
        for suffix in ["index.db", "index.db-wal", "index.db-shm"] {
            let src = raw_dir.join(suffix);
            if src.exists() {
                let _ = std::fs::rename(&src, db_dir.join(suffix));
            }
        }
        let _ = std::fs::remove_dir(&raw_dir);
    }

    // Auto-migrate: if new hash dir doesn't have a DB, look for old one by metadata
    if !db_dir.join("index.db").exists() {
        if let Ok(entries) = std::fs::read_dir(&cache_dir) {
            for entry in entries.flatten() {
                let old_dir = entry.path();
                if old_dir.is_dir() && old_dir.file_name().map(|n| n.to_string_lossy().to_string()) != Some(project_hash.clone()) {
                    let old_db = old_dir.join("index.db");
                    if old_db.exists() {
                        // Check if this DB belongs to our project by reading metadata
                        if let Ok(conn) = rusqlite::Connection::open(&old_db) {
                            let root_str: Result<String, _> = conn.query_row(
                                "SELECT value FROM metadata WHERE key = 'project_root'",
                                [],
                                |row| row.get(0),
                            );
                            if let Ok(root_val) = root_str {
                                // Match against both raw and normalized paths
                                let stored_normalized = normalize_root(Path::new(&root_val));
                                if stored_normalized == normalized
                                    || root_val == project_root.to_string_lossy().as_ref()
                                {
                                    // Found old DB for this project — migrate
                                    let _ = std::fs::create_dir_all(&db_dir);
                                    for suffix in ["index.db", "index.db-wal", "index.db-shm"] {
                                        let src = old_dir.join(suffix);
                                        if src.exists() {
                                            let _ = std::fs::rename(&src, db_dir.join(suffix));
                                        }
                                    }
                                    let _ = std::fs::remove_dir(&old_dir);
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    std::fs::create_dir_all(&db_dir)?;
    Ok(db_dir.join("index.db"))
}

/// Deterministic hash (djb2 algorithm) — stable across Rust versions unlike DefaultHasher
fn simple_hash(s: &str) -> String {
    let mut hash: u64 = 5381;
    for byte in s.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(byte as u64);
    }
    format!("{:x}", hash)
}

/// Remove old kotlin-index cache dir entirely
pub fn cleanup_legacy_cache() {
    if let Some(cache_dir) = dirs::cache_dir() {
        let old_dir = cache_dir.join("kotlin-index");
        if old_dir.exists() {
            let _ = std::fs::remove_dir_all(&old_dir);
        }
    }
}

/// Migrate project DB from old kotlin-index dir to new ast-index dir
pub fn migrate_legacy_project(project_root: &Path) {
    let cache_dir = match dirs::cache_dir() {
        Some(d) => d,
        None => return,
    };
    let project_hash = simple_hash(project_root.to_string_lossy().as_ref());
    let old_db_dir = cache_dir.join("kotlin-index").join(&project_hash);
    let new_db_dir = cache_dir.join("ast-index").join(&project_hash);

    if !old_db_dir.exists() || new_db_dir.join("index.db").exists() {
        return;
    }

    let _ = std::fs::create_dir_all(&new_db_dir);
    for suffix in ["index.db", "index.db-wal", "index.db-shm"] {
        let src = old_db_dir.join(suffix);
        if src.exists() {
            let _ = std::fs::rename(&src, new_db_dir.join(suffix));
        }
    }
    // Remove old project dir if empty
    let _ = std::fs::remove_dir(&old_db_dir);
}

/// Acquire an exclusive lock file for rebuild operations.
/// Returns the lock file handle — lock is held until the handle is dropped.
/// If another process holds the lock, returns an error immediately.
pub fn acquire_rebuild_lock(project_root: &Path) -> Result<File> {
    use fs2::FileExt;

    let db_path = get_db_path(project_root)?;
    let lock_path = db_path.with_extension("lock");

    // Ensure parent dir exists
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let lock_file = File::create(&lock_path)?;
    lock_file.try_lock_exclusive()
        .map_err(|_| anyhow::anyhow!("Another rebuild is already running for this project. Wait for it to finish or remove {}", lock_path.display()))?;
    Ok(lock_file)
}

/// Delete DB file and WAL/SHM files for the project
pub fn delete_db(project_root: &Path) -> Result<()> {
    let db_path = get_db_path(project_root)?;
    for suffix in ["", "-wal", "-shm"] {
        let p = db_path.with_extension(format!("db{}", suffix));
        if p.exists() {
            std::fs::remove_file(&p)?;
        }
    }
    Ok(())
}

/// Initialize the database schema
pub fn init_db(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        -- Files table
        CREATE TABLE IF NOT EXISTS files (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL UNIQUE,
            mtime INTEGER NOT NULL,
            size INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_files_path ON files(path);

        -- Symbols table (classes, interfaces, functions, etc.)
        CREATE TABLE IF NOT EXISTS symbols (
            id INTEGER PRIMARY KEY,
            file_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            line INTEGER NOT NULL,
            parent_id INTEGER,
            signature TEXT,
            FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
        CREATE INDEX IF NOT EXISTS idx_symbols_kind ON symbols(kind);
        CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_id);

        -- FTS5 virtual table for full-text search
        CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
            name,
            signature,
            content=symbols,
            content_rowid=id
        );

        -- Triggers to keep FTS in sync
        CREATE TRIGGER IF NOT EXISTS symbols_ai AFTER INSERT ON symbols BEGIN
            INSERT INTO symbols_fts(rowid, name, signature) VALUES (new.id, new.name, new.signature);
        END;
        CREATE TRIGGER IF NOT EXISTS symbols_ad AFTER DELETE ON symbols BEGIN
            INSERT INTO symbols_fts(symbols_fts, rowid, name, signature) VALUES('delete', old.id, old.name, old.signature);
        END;
        CREATE TRIGGER IF NOT EXISTS symbols_au AFTER UPDATE ON symbols BEGIN
            INSERT INTO symbols_fts(symbols_fts, rowid, name, signature) VALUES('delete', old.id, old.name, old.signature);
            INSERT INTO symbols_fts(rowid, name, signature) VALUES (new.id, new.name, new.signature);
        END;

        -- Modules table
        CREATE TABLE IF NOT EXISTS modules (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            path TEXT NOT NULL,
            kind TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_modules_name ON modules(name);

        -- Module dependencies
        CREATE TABLE IF NOT EXISTS module_deps (
            id INTEGER PRIMARY KEY,
            module_id INTEGER NOT NULL,
            dep_module_id INTEGER NOT NULL,
            dep_kind TEXT,
            FOREIGN KEY (module_id) REFERENCES modules(id) ON DELETE CASCADE,
            FOREIGN KEY (dep_module_id) REFERENCES modules(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_module_deps_module ON module_deps(module_id);
        CREATE INDEX IF NOT EXISTS idx_module_deps_dep ON module_deps(dep_module_id);

        -- Inheritance/implementation relationships
        CREATE TABLE IF NOT EXISTS inheritance (
            id INTEGER PRIMARY KEY,
            child_id INTEGER NOT NULL,
            parent_name TEXT NOT NULL,
            kind TEXT NOT NULL,
            FOREIGN KEY (child_id) REFERENCES symbols(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_inheritance_child ON inheritance(child_id);
        CREATE INDEX IF NOT EXISTS idx_inheritance_parent ON inheritance(parent_name);

        -- References table (symbol usages)
        CREATE TABLE IF NOT EXISTS refs (
            id INTEGER PRIMARY KEY,
            file_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            line INTEGER NOT NULL,
            context TEXT,
            FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_refs_name ON refs(name);
        CREATE INDEX IF NOT EXISTS idx_refs_file ON refs(file_id);
        -- Composite covering index for find_references: lets SQLite avoid
        -- full table scan when filtering by name AND joining with files
        -- on large ref tables (millions of rows). See issue #19.
        CREATE INDEX IF NOT EXISTS idx_refs_name_file_line ON refs(name, file_id, line);

        -- XML usages (classes used in XML layouts)
        CREATE TABLE IF NOT EXISTS xml_usages (
            id INTEGER PRIMARY KEY,
            module_id INTEGER,
            file_path TEXT NOT NULL,
            line INTEGER NOT NULL,
            class_name TEXT NOT NULL,
            usage_type TEXT,
            element_id TEXT,
            FOREIGN KEY (module_id) REFERENCES modules(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_xml_usages_class ON xml_usages(class_name);
        CREATE INDEX IF NOT EXISTS idx_xml_usages_module ON xml_usages(module_id);

        -- Resources definitions
        CREATE TABLE IF NOT EXISTS resources (
            id INTEGER PRIMARY KEY,
            module_id INTEGER,
            type TEXT NOT NULL,
            name TEXT NOT NULL,
            file_path TEXT NOT NULL,
            line INTEGER,
            FOREIGN KEY (module_id) REFERENCES modules(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_resources_name ON resources(name);
        CREATE INDEX IF NOT EXISTS idx_resources_type ON resources(type);
        CREATE INDEX IF NOT EXISTS idx_resources_module ON resources(module_id);

        -- Resource usages
        CREATE TABLE IF NOT EXISTS resource_usages (
            id INTEGER PRIMARY KEY,
            resource_id INTEGER,
            usage_file TEXT NOT NULL,
            usage_line INTEGER NOT NULL,
            usage_type TEXT,
            FOREIGN KEY (resource_id) REFERENCES resources(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_resource_usages_resource ON resource_usages(resource_id);

        -- Transitive dependencies cache
        CREATE TABLE IF NOT EXISTS transitive_deps (
            id INTEGER PRIMARY KEY,
            module_id INTEGER NOT NULL,
            dependency_id INTEGER NOT NULL,
            depth INTEGER NOT NULL,
            path TEXT,
            FOREIGN KEY (module_id) REFERENCES modules(id) ON DELETE CASCADE,
            FOREIGN KEY (dependency_id) REFERENCES modules(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_transitive_deps_module ON transitive_deps(module_id);
        CREATE INDEX IF NOT EXISTS idx_transitive_deps_dep ON transitive_deps(dependency_id);

        -- iOS storyboard/xib usages
        CREATE TABLE IF NOT EXISTS storyboard_usages (
            id INTEGER PRIMARY KEY,
            module_id INTEGER,
            file_path TEXT NOT NULL,
            line INTEGER NOT NULL,
            class_name TEXT NOT NULL,
            usage_type TEXT,
            storyboard_id TEXT,
            FOREIGN KEY (module_id) REFERENCES modules(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_storyboard_usages_class ON storyboard_usages(class_name);
        CREATE INDEX IF NOT EXISTS idx_storyboard_usages_module ON storyboard_usages(module_id);

        -- iOS assets (from .xcassets)
        CREATE TABLE IF NOT EXISTS ios_assets (
            id INTEGER PRIMARY KEY,
            module_id INTEGER,
            type TEXT NOT NULL,
            name TEXT NOT NULL,
            file_path TEXT NOT NULL,
            FOREIGN KEY (module_id) REFERENCES modules(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_ios_assets_name ON ios_assets(name);
        CREATE INDEX IF NOT EXISTS idx_ios_assets_type ON ios_assets(type);

        -- iOS asset usages
        CREATE TABLE IF NOT EXISTS ios_asset_usages (
            id INTEGER PRIMARY KEY,
            asset_id INTEGER,
            usage_file TEXT NOT NULL,
            usage_line INTEGER NOT NULL,
            usage_type TEXT,
            FOREIGN KEY (asset_id) REFERENCES ios_assets(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_ios_asset_usages_asset ON ios_asset_usages(asset_id);

        -- Metadata for storing index settings
        CREATE TABLE IF NOT EXISTS metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        "#,
    )?;
    Ok(())
}

/// Open or create database connection
pub fn open_db(project_root: &Path) -> Result<Connection> {
    let db_path = get_db_path(project_root)?;
    let conn = Connection::open(&db_path)?;

    // Enable foreign keys and WAL mode for better performance
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // journal_mode returns result, use query_row
    let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "cache_size", "-8000")?; // 8 MB cache to limit memory
    let _: i64 = conn.query_row("PRAGMA busy_timeout = 5000", [], |row| row.get(0))?; // Wait up to 5s if DB is locked

    // Store normalized project root for hash migration
    let normalized = normalize_root(project_root);
    conn.execute(
        "CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        [],
    ).ok();
    conn.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES ('project_root', ?1)",
        params![&normalized],
    ).ok();

    Ok(conn)
}

/// Check if database exists and is initialized
pub fn db_exists(project_root: &Path) -> bool {
    if let Ok(db_path) = get_db_path(project_root) {
        if !db_path.exists() {
            return false;
        }
        // Also check if tables exist
        if let Ok(conn) = Connection::open(&db_path) {
            conn.query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='files'",
                [],
                |_| Ok(()),
            )
            .is_ok()
        } else {
            false
        }
    } else {
        false
    }
}

/// Symbol kinds
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Class,
    Interface,
    Object,
    Enum,
    Function,
    Procedure,
    Property,
    TypeAlias,
    // Perl-specific
    Package,
    Constant,
    // For imports/includes
    Import,
    // For annotations/decorators
    Annotation,
}

impl SymbolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SymbolKind::Class => "class",
            SymbolKind::Interface => "interface",
            SymbolKind::Object => "object",
            SymbolKind::Enum => "enum",
            SymbolKind::Function => "function",
            SymbolKind::Procedure => "procedure",
            SymbolKind::Property => "property",
            SymbolKind::TypeAlias => "typealias",
            SymbolKind::Package => "package",
            SymbolKind::Constant => "constant",
            SymbolKind::Import => "import",
            SymbolKind::Annotation => "annotation",
        }
    }
}

/// Insert or update a file record
pub fn upsert_file(conn: &Connection, path: &str, mtime: i64, size: i64) -> Result<i64> {
    conn.execute(
        "INSERT OR REPLACE INTO files (path, mtime, size) VALUES (?1, ?2, ?3)",
        params![path, mtime, size],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Insert a symbol
pub fn insert_symbol(
    conn: &Connection,
    file_id: i64,
    name: &str,
    kind: SymbolKind,
    line: usize,
    signature: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO symbols (file_id, name, kind, line, signature) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![file_id, name, kind.as_str(), line as i64, signature],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Insert inheritance relationship
pub fn insert_inheritance(
    conn: &Connection,
    child_id: i64,
    parent_name: &str,
    kind: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO inheritance (child_id, parent_name, kind) VALUES (?1, ?2, ?3)",
        params![child_id, parent_name, kind],
    )?;
    Ok(())
}

/// Escape FTS5 special characters
fn escape_fts5_query(query: &str) -> String {
    // Handle empty query
    if query.trim().is_empty() {
        return String::new();
    }
    // Check for prefix operator: * must stay OUTSIDE quotes for FTS5
    let (term, suffix) = if query.ends_with('*') {
        (&query[..query.len() - 1], "*")
    } else {
        (query, "")
    };
    // Wrap in double quotes to treat as literal phrase
    // Escape any existing double quotes
    let escaped = term.replace('"', "\"\"");
    format!("\"{}\"{}",  escaped, suffix)
}

/// Search symbols by name (FTS5)
pub fn search_symbols(conn: &Connection, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
    // Handle empty query
    if query.trim().is_empty() {
        return Ok(vec![]);
    }

    let escaped_query = escape_fts5_query(query);

    let mut stmt = conn.prepare(
        r#"
        SELECT s.name, s.kind, s.line, s.signature, f.path
        FROM symbols_fts fts
        JOIN symbols s ON fts.rowid = s.id
        JOIN files f ON s.file_id = f.id
        WHERE symbols_fts MATCH ?1
        LIMIT ?2
        "#,
    )?;

    let results = stmt
        .query_map(params![escaped_query, limit as i64], |row| {
            Ok(SearchResult {
                name: row.get(0)?,
                kind: row.get(1)?,
                line: row.get(2)?,
                signature: row.get(3)?,
                path: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Search result
#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub name: String,
    pub kind: String,
    pub line: i64,
    pub signature: Option<String>,
    pub path: String,
}

/// Find files by name pattern
pub fn find_files(conn: &Connection, pattern: &str, limit: usize) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT path FROM files WHERE path LIKE ?1 LIMIT ?2",
    )?;

    let pattern = format!("%{}%", pattern);
    let results = stmt
        .query_map(params![pattern, limit as i64], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Find symbols by name (exact match first, then prefix/contains if no results)
pub fn find_symbols_by_name(
    conn: &Connection,
    name: &str,
    kind: Option<&str>,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    // Try exact match first
    let exact_query = if kind.is_some() {
        r#"
        SELECT s.name, s.kind, s.line, s.signature, f.path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE s.name = ?1 AND s.kind = ?2
        LIMIT ?3
        "#
    } else {
        r#"
        SELECT s.name, s.kind, s.line, s.signature, f.path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE s.name = ?1
        LIMIT ?2
        "#
    };

    let mut stmt = conn.prepare(exact_query)?;

    let results: Vec<SearchResult> = if let Some(k) = kind {
        stmt.query_map(params![name, k, limit as i64], |row| {
            Ok(SearchResult {
                name: row.get(0)?,
                kind: row.get(1)?,
                line: row.get(2)?,
                signature: row.get(3)?,
                path: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map(params![name, limit as i64], |row| {
            Ok(SearchResult {
                name: row.get(0)?,
                kind: row.get(1)?,
                line: row.get(2)?,
                signature: row.get(3)?,
                path: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?
    };

    // If no exact match, try prefix match
    if results.is_empty() {
        let pattern = format!("{}%", name);
        let prefix_query = if kind.is_some() {
            r#"
            SELECT s.name, s.kind, s.line, s.signature, f.path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.name LIKE ?1 AND s.kind = ?2
            ORDER BY length(s.name)
            LIMIT ?3
            "#
        } else {
            r#"
            SELECT s.name, s.kind, s.line, s.signature, f.path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.name LIKE ?1
            ORDER BY length(s.name)
            LIMIT ?2
            "#
        };

        let mut stmt = conn.prepare(prefix_query)?;
        let results: Vec<SearchResult> = if let Some(k) = kind {
            stmt.query_map(params![pattern, k, limit as i64], |row| {
                Ok(SearchResult {
                    name: row.get(0)?,
                    kind: row.get(1)?,
                    line: row.get(2)?,
                    signature: row.get(3)?,
                    path: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![pattern, limit as i64], |row| {
                Ok(SearchResult {
                    name: row.get(0)?,
                    kind: row.get(1)?,
                    line: row.get(2)?,
                    signature: row.get(3)?,
                    path: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        return Ok(results);
    }

    Ok(results)
}

/// Find class-like symbols (class, interface, object, enum) by name - single query
pub fn find_class_like(
    conn: &Connection,
    name: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT s.name, s.kind, s.line, s.signature, f.path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE s.name = ?1 AND s.kind IN ('class', 'interface', 'object', 'enum', 'protocol', 'struct', 'actor', 'package')
        LIMIT ?2
        "#,
    )?;

    let results = stmt
        .query_map(params![name, limit as i64], |row| {
            Ok(SearchResult {
                name: row.get(0)?,
                kind: row.get(1)?,
                line: row.get(2)?,
                signature: row.get(3)?,
                path: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Convert glob pattern to SQL LIKE pattern: * → %, ? → _
pub fn glob_to_like(pattern: &str) -> String {
    let mut result = String::with_capacity(pattern.len() + 4);
    for ch in pattern.chars() {
        match ch {
            '*' => result.push('%'),
            '?' => result.push('_'),
            '%' => { result.push_str("\\%"); }
            '_' => { result.push_str("\\_"); }
            _ => result.push(ch),
        }
    }
    result
}

/// Find class-like symbols matching a glob pattern
pub fn find_class_like_pattern(
    conn: &Connection,
    like_pattern: &str,
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<SearchResult>> {
    let (scope_clause, scope_params) = scope.path_condition();

    let sql = format!(
        r#"
        SELECT s.name, s.kind, s.line, s.signature, f.path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE s.name LIKE ?1 ESCAPE '\' AND s.kind IN ('class', 'interface', 'object', 'enum', 'protocol', 'struct', 'actor', 'package'){}
        ORDER BY length(s.name), s.name
        LIMIT ?{}
        "#,
        scope_clause,
        2 + scope_params.len()
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    all_params.push(Box::new(like_pattern.to_string()));
    for p in &scope_params {
        all_params.push(Box::new(p.clone()));
    }
    all_params.push(Box::new(limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = all_params.iter().map(|p| p.as_ref()).collect();
    let results = stmt
        .query_map(param_refs.as_slice(), |row| {
            Ok(SearchResult {
                name: row.get(0)?,
                kind: row.get(1)?,
                line: row.get(2)?,
                signature: row.get(3)?,
                path: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Find symbols matching a glob pattern with optional kind filter
pub fn find_symbols_by_pattern(
    conn: &Connection,
    like_pattern: &str,
    kind: Option<&str>,
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<SearchResult>> {
    let (scope_clause, scope_params) = scope.path_condition();

    let kind_clause = if kind.is_some() {
        format!(" AND s.kind = ?{}", 2 + scope_params.len())
    } else {
        String::new()
    };

    let limit_idx = if kind.is_some() {
        3 + scope_params.len()
    } else {
        2 + scope_params.len()
    };

    let sql = format!(
        r#"
        SELECT s.name, s.kind, s.line, s.signature, f.path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE s.name LIKE ?1 ESCAPE '\'{}{}
        ORDER BY length(s.name), s.name
        LIMIT ?{}
        "#,
        scope_clause,
        kind_clause,
        limit_idx
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    all_params.push(Box::new(like_pattern.to_string()));
    for p in &scope_params {
        all_params.push(Box::new(p.clone()));
    }
    if let Some(k) = kind {
        all_params.push(Box::new(k.to_string()));
    }
    all_params.push(Box::new(limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = all_params.iter().map(|p| p.as_ref()).collect();
    let results = stmt
        .query_map(param_refs.as_slice(), |row| {
            Ok(SearchResult {
                name: row.get(0)?,
                kind: row.get(1)?,
                line: row.get(2)?,
                signature: row.get(3)?,
                path: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Find implementations (subclasses/implementors)
pub fn find_implementations(
    conn: &Connection,
    parent_name: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    // Match exact name or qualified suffix (%.Name)
    let suffix_pattern = format!("%.{}", parent_name);
    let mut stmt = conn.prepare(
        r#"
        SELECT s.name, s.kind, s.line, s.signature, f.path
        FROM inheritance i
        JOIN symbols s ON i.child_id = s.id
        JOIN files f ON s.file_id = f.id
        WHERE i.parent_name = ?1 OR i.parent_name LIKE ?2
        ORDER BY
            CASE
                WHEN i.parent_name = ?1 THEN 0
                ELSE 1
            END, s.name
        LIMIT ?3
        "#,
    )?;

    let results = stmt
        .query_map(params![parent_name, suffix_pattern, limit as i64], |row| {
            Ok(SearchResult {
                name: row.get(0)?,
                kind: row.get(1)?,
                line: row.get(2)?,
                signature: row.get(3)?,
                path: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

pub fn find_implementations_scoped(
    conn: &Connection,
    parent_name: &str,
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<SearchResult>> {
    if scope.is_empty() {
        return find_implementations(conn, parent_name, limit);
    }

    let suffix_pattern = format!("%.{}", parent_name);
    let (scope_clause, scope_params) = scope.path_condition();

    let sql = format!(
        r#"
        SELECT s.name, s.kind, s.line, s.signature, f.path
        FROM inheritance i
        JOIN symbols s ON i.child_id = s.id
        JOIN files f ON s.file_id = f.id
        WHERE (i.parent_name = ?1 OR i.parent_name LIKE ?2){}
        ORDER BY
            CASE
                WHEN i.parent_name = ?1 THEN 0
                ELSE 1
            END, s.name
        LIMIT ?{}
        "#,
        scope_clause,
        3 + scope_params.len()
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    all_params.push(Box::new(parent_name.to_string()));
    all_params.push(Box::new(suffix_pattern));
    for p in &scope_params {
        all_params.push(Box::new(p.clone()));
    }
    all_params.push(Box::new(limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = all_params.iter().map(|p| p.as_ref()).collect();
    let results = stmt
        .query_map(param_refs.as_slice(), |row| {
            Ok(SearchResult {
                name: row.get(0)?,
                kind: row.get(1)?,
                line: row.get(2)?,
                signature: row.get(3)?,
                path: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Find parents (inherited types) of a symbol, scoped to the file that defines it.
/// When scope is empty, returns parents for all symbols with that name.
pub fn find_parents_scoped(
    conn: &Connection,
    child_name: &str,
    scope: &SearchScope,
) -> Result<Vec<(String, String)>> {
    let (scope_clause, scope_params) = scope.path_condition();
    let sql = format!(
        "SELECT i.parent_name, i.kind \
         FROM inheritance i \
         JOIN symbols s ON i.child_id = s.id \
         JOIN files f ON s.file_id = f.id \
         WHERE s.name = ?1{}",
        scope_clause
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(child_name.to_string())];
    for p in &scope_params {
        all_params.push(Box::new(p.clone()));
    }
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = all_params.iter().map(|p| p.as_ref()).collect();
    let results = stmt
        .query_map(param_refs.as_slice(), |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    Ok(results)
}

/// Get database statistics
pub fn get_stats(conn: &Connection) -> Result<DbStats> {
    let file_count: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;
    let symbol_count: i64 = conn.query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))?;
    let module_count: i64 = conn.query_row("SELECT COUNT(*) FROM modules", [], |row| row.get(0))?;
    let refs_count: i64 = conn.query_row("SELECT COUNT(*) FROM refs", [], |row| row.get(0)).unwrap_or(0);
    let xml_usages_count: i64 = conn.query_row("SELECT COUNT(*) FROM xml_usages", [], |row| row.get(0)).unwrap_or(0);
    let resources_count: i64 = conn.query_row("SELECT COUNT(*) FROM resources", [], |row| row.get(0)).unwrap_or(0);
    let storyboard_usages_count: i64 = conn.query_row("SELECT COUNT(*) FROM storyboard_usages", [], |row| row.get(0)).unwrap_or(0);
    let ios_assets_count: i64 = conn.query_row("SELECT COUNT(*) FROM ios_assets", [], |row| row.get(0)).unwrap_or(0);

    Ok(DbStats {
        file_count,
        symbol_count,
        module_count,
        refs_count,
        xml_usages_count,
        resources_count,
        storyboard_usages_count,
        ios_assets_count,
    })
}

#[derive(Debug, Serialize)]
pub struct DbStats {
    pub file_count: i64,
    pub symbol_count: i64,
    pub module_count: i64,
    pub refs_count: i64,
    pub xml_usages_count: i64,
    pub resources_count: i64,
    pub storyboard_usages_count: i64,
    pub ios_assets_count: i64,
}

/// Clear all data from the database
pub fn clear_db(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        DELETE FROM ios_asset_usages;
        DELETE FROM ios_assets;
        DELETE FROM storyboard_usages;
        DELETE FROM resource_usages;
        DELETE FROM resources;
        DELETE FROM xml_usages;
        DELETE FROM transitive_deps;
        DELETE FROM refs;
        DELETE FROM inheritance;
        DELETE FROM module_deps;
        DELETE FROM modules;
        DELETE FROM symbols;
        DELETE FROM files;
        "#,
    )?;
    Ok(())
}

/// Reference result
#[derive(Debug, Serialize)]
pub struct RefResult {
    pub name: String,
    pub line: i64,
    pub context: Option<String>,
    pub path: String,
}

/// Find references (usages) of a symbol
pub fn find_references(
    conn: &Connection,
    name: &str,
    limit: usize,
) -> Result<Vec<RefResult>> {
    // Early materialization: filter and sort refs using covering index BEFORE
    // joining with files. Avoids SQLite planner choosing full scan on large
    // tables (~12M rows) when ORDER BY references the joined table. See #19.
    //
    // Inner ORDER BY (file_id, line) is free because idx_refs_name_file_line
    // has exactly this sort order. Outer ORDER BY f.path reshuffles the tiny
    // result set (bounded by LIMIT) so output is stable for users.
    let mut stmt = conn.prepare(
        r#"
        SELECT r.name, r.line, r.context, f.path
        FROM (
            SELECT name, file_id, line, context
            FROM refs
            WHERE name = ?1
            ORDER BY file_id, line
            LIMIT ?2
        ) r
        JOIN files f ON f.id = r.file_id
        ORDER BY f.path, r.line
        "#,
    )?;

    let results = stmt
        .query_map(params![name, limit as i64], |row| {
            Ok(RefResult {
                name: row.get(0)?,
                line: row.get(1)?,
                context: row.get(2)?,
                path: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Search references by name (prefix match, grouped by unique name)
pub fn search_refs(conn: &Connection, query: &str, limit: usize) -> Result<Vec<(String, i64)>> {
    let pattern = format!("{}%", query);
    let mut stmt = conn.prepare(
        r#"
        SELECT r.name, COUNT(*) as usage_count
        FROM refs r
        WHERE r.name LIKE ?1
        GROUP BY r.name
        ORDER BY
            CASE WHEN r.name = ?2 THEN 0
                 WHEN r.name LIKE ?1 THEN 1
                 ELSE 2
            END,
            usage_count DESC
        LIMIT ?3
        "#,
    )?;
    let results = stmt
        .query_map(params![pattern, query, limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

/// Count references in the database
pub fn count_refs(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM refs", [], |row| row.get(0))?)
}

/// Find import statements for a symbol name
pub fn find_imports(conn: &Connection, name: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT s.name, s.kind, s.line, s.signature, f.path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE s.kind = 'import' AND s.name = ?1
        LIMIT ?2
        "#,
    )?;

    let results = stmt
        .query_map(params![name, limit as i64], |row| {
            Ok(SearchResult {
                name: row.get(0)?,
                kind: row.get(1)?,
                line: row.get(2)?,
                signature: row.get(3)?,
                path: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Find all cross-references for a symbol: definitions, imports, and usages
pub fn find_cross_references(
    conn: &Connection,
    name: &str,
    limit: usize,
) -> Result<(Vec<SearchResult>, Vec<SearchResult>, Vec<RefResult>)> {
    // 1. Definitions (non-import symbols)
    let definitions = find_symbols_by_name(conn, name, None, limit)?
        .into_iter()
        .filter(|s| s.kind != "import")
        .collect();

    // 2. Imports
    let imports = find_imports(conn, name, limit)?;

    // 3. Usages (refs table)
    let usages = find_references(conn, name, limit)?;

    Ok((definitions, imports, usages))
}

/// Fuzzy search for symbols: exact → prefix → contains cascade
pub fn search_symbols_fuzzy(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    // Single query: contains match with ranking by relevance
    // exact match (name = query) first, then prefix, then contains — sorted by length
    let contains_pattern = format!("%{}%", query);
    let mut stmt = conn.prepare(
        r#"
        SELECT s.name, s.kind, s.line, s.signature, f.path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE s.name LIKE ?1
        ORDER BY
            CASE WHEN s.name = ?2 THEN 0
                 WHEN s.name LIKE ?3 THEN 1
                 ELSE 2 END,
            length(s.name)
        LIMIT ?4
        "#,
    )?;
    let prefix_pattern = format!("{}%", query);
    let results: Vec<SearchResult> = stmt
        .query_map(params![contains_pattern, query, prefix_pattern, limit as i64], |row| {
            Ok(SearchResult {
                name: row.get(0)?,
                kind: row.get(1)?,
                line: row.get(2)?,
                signature: row.get(3)?,
                path: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Scope filter for narrowing search results by file path or module
pub struct SearchScope<'a> {
    pub in_file: Option<&'a str>,
    pub module: Option<&'a str>,
    /// Directory prefix filter: only return results under this path (relative to project root)
    pub dir_prefix: Option<&'a str>,
}

impl<'a> SearchScope<'a> {
    pub fn none() -> Self {
        SearchScope { in_file: None, module: None, dir_prefix: None }
    }

    pub fn is_empty(&self) -> bool {
        self.in_file.is_none() && self.module.is_none() && self.dir_prefix.is_none()
    }

    /// Build WHERE clause fragment and collect params
    fn path_condition(&self) -> (String, Vec<String>) {
        let mut conditions = Vec::new();
        let mut params = Vec::new();
        if let Some(prefix) = self.dir_prefix {
            conditions.push("f.path LIKE ?".to_string());
            params.push(format!("{}%", prefix));
        }
        if let Some(file) = self.in_file {
            conditions.push("f.path LIKE ?".to_string());
            params.push(format!("%{}%", file));
        }
        if let Some(module) = self.module {
            conditions.push("f.path LIKE ?".to_string());
            params.push(format!("{}%", module));
        }
        if conditions.is_empty() {
            (String::new(), params)
        } else {
            (format!(" AND {}", conditions.join(" AND ")), params)
        }
    }
}

/// Search symbols with scope filtering (file/module)
pub fn search_symbols_scoped(
    conn: &Connection,
    query: &str,
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<SearchResult>> {
    if scope.is_empty() {
        return search_symbols(conn, query, limit);
    }

    if query.trim().is_empty() {
        return Ok(vec![]);
    }

    let escaped_query = escape_fts5_query(query);
    let (scope_clause, scope_params) = scope.path_condition();

    let sql = format!(
        r#"
        SELECT s.name, s.kind, s.line, s.signature, f.path
        FROM symbols_fts fts
        JOIN symbols s ON fts.rowid = s.id
        JOIN files f ON s.file_id = f.id
        WHERE symbols_fts MATCH ?1{}
        LIMIT ?{}
        "#,
        scope_clause,
        2 + scope_params.len()
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    all_params.push(Box::new(escaped_query));
    for p in &scope_params {
        all_params.push(Box::new(p.clone()));
    }
    all_params.push(Box::new(limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = all_params.iter().map(|p| p.as_ref()).collect();
    let results = stmt
        .query_map(param_refs.as_slice(), |row| {
            Ok(SearchResult {
                name: row.get(0)?,
                kind: row.get(1)?,
                line: row.get(2)?,
                signature: row.get(3)?,
                path: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Find symbols by name with scope filtering
pub fn find_symbols_by_name_scoped(
    conn: &Connection,
    name: &str,
    kind: Option<&str>,
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<SearchResult>> {
    if scope.is_empty() {
        return find_symbols_by_name(conn, name, kind, limit);
    }

    let (scope_clause, scope_params) = scope.path_condition();

    let mut sql = format!(
        "SELECT s.name, s.kind, s.line, s.signature, f.path FROM symbols s JOIN files f ON s.file_id = f.id WHERE s.name = ?1{}",
        scope_clause
    );
    if kind.is_some() {
        sql.push_str(&format!(" AND s.kind = ?{}", 2 + scope_params.len()));
        sql.push_str(&format!(" LIMIT ?{}", 3 + scope_params.len()));
    } else {
        sql.push_str(&format!(" LIMIT ?{}", 2 + scope_params.len()));
    }

    let mut stmt = conn.prepare(&sql)?;
    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    all_params.push(Box::new(name.to_string()));
    for p in &scope_params {
        all_params.push(Box::new(p.clone()));
    }
    if let Some(k) = kind {
        all_params.push(Box::new(k.to_string()));
    }
    all_params.push(Box::new(limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = all_params.iter().map(|p| p.as_ref()).collect();
    let results = stmt
        .query_map(param_refs.as_slice(), |row| {
            Ok(SearchResult {
                name: row.get(0)?,
                kind: row.get(1)?,
                line: row.get(2)?,
                signature: row.get(3)?,
                path: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Find class-like symbols with scope filtering
pub fn find_class_like_scoped(
    conn: &Connection,
    name: &str,
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<SearchResult>> {
    if scope.is_empty() {
        return find_class_like(conn, name, limit);
    }

    let (scope_clause, scope_params) = scope.path_condition();

    let sql = format!(
        r#"
        SELECT s.name, s.kind, s.line, s.signature, f.path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE s.name = ?1 AND s.kind IN ('class', 'interface', 'object', 'enum', 'protocol', 'struct', 'actor', 'package'){}
        LIMIT ?{}
        "#,
        scope_clause,
        2 + scope_params.len()
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    all_params.push(Box::new(name.to_string()));
    for p in &scope_params {
        all_params.push(Box::new(p.clone()));
    }
    all_params.push(Box::new(limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = all_params.iter().map(|p| p.as_ref()).collect();
    let results = stmt
        .query_map(param_refs.as_slice(), |row| {
            Ok(SearchResult {
                name: row.get(0)?,
                kind: row.get(1)?,
                line: row.get(2)?,
                signature: row.get(3)?,
                path: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Find references with scope filtering
pub fn find_references_scoped(
    conn: &Connection,
    name: &str,
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<RefResult>> {
    if scope.is_empty() {
        return find_references(conn, name, limit);
    }

    let (scope_clause, scope_params) = scope.path_condition();

    // Early materialization with scope pushed into the subquery via IN clause.
    // Avoids materializing millions of refs when scope narrows by path. See #19.
    //
    // Scope filter is applied at files table (small, ~tens of thousands),
    // producing a small file_id set, then refs are filtered by both name
    // AND file_id — both covered by idx_refs_name_file_line.
    let scope_subquery = if scope_params.is_empty() {
        String::new()
    } else {
        // Strip leading " AND " and wrap in file_id IN subselect
        let bare_conditions = scope_clause.trim_start_matches(" AND ");
        format!(" AND file_id IN (SELECT id FROM files f WHERE {})", bare_conditions)
    };

    let sql = format!(
        r#"
        SELECT r.name, r.line, r.context, f.path
        FROM (
            SELECT name, file_id, line, context
            FROM refs
            WHERE name = ?1{}
            ORDER BY file_id, line
            LIMIT ?{}
        ) r
        JOIN files f ON f.id = r.file_id
        ORDER BY f.path, r.line
        "#,
        scope_subquery,
        2 + scope_params.len()
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    all_params.push(Box::new(name.to_string()));
    for p in &scope_params {
        all_params.push(Box::new(p.clone()));
    }
    all_params.push(Box::new(limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = all_params.iter().map(|p| p.as_ref()).collect();
    let results = stmt
        .query_map(param_refs.as_slice(), |row| {
            Ok(RefResult {
                name: row.get(0)?,
                line: row.get(1)?,
                context: row.get(2)?,
                path: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Get extra source roots stored in metadata
pub fn get_extra_roots(conn: &Connection) -> Result<Vec<String>> {
    let result: Result<String, _> = conn.query_row(
        "SELECT value FROM metadata WHERE key = 'extra_roots'",
        [],
        |row| row.get(0),
    );
    match result {
        Ok(json) => {
            let roots: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
            Ok(roots)
        }
        Err(_) => Ok(vec![]),
    }
}

/// Add an extra source root
pub fn add_extra_root(conn: &Connection, path: &str) -> Result<()> {
    let mut roots = get_extra_roots(conn)?;
    if !roots.contains(&path.to_string()) {
        roots.push(path.to_string());
    }
    let json = serde_json::to_string(&roots)?;
    conn.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES ('extra_roots', ?1)",
        params![json],
    )?;
    Ok(())
}

/// Remove an extra source root
pub fn remove_extra_root(conn: &Connection, path: &str) -> Result<bool> {
    let mut roots = get_extra_roots(conn)?;
    let len_before = roots.len();
    roots.retain(|r| r != path);
    if roots.len() == len_before {
        return Ok(false);
    }
    let json = serde_json::to_string(&roots)?;
    conn.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES ('extra_roots', ?1)",
        params![json],
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn
    }

    #[test]
    fn test_simple_hash_deterministic() {
        let h1 = simple_hash("/Users/test/project");
        let h2 = simple_hash("/Users/test/project");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_simple_hash_different() {
        let h1 = simple_hash("/Users/test/project1");
        let h2 = simple_hash("/Users/test/project2");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_init_db() {
        let conn = create_test_db();
        // Check tables exist
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='files'",
            [], |row| row.get(0)
        ).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_escape_fts5_query_simple() {
        assert_eq!(escape_fts5_query("MyClass"), "\"MyClass\"");
    }

    #[test]
    fn test_escape_fts5_query_prefix() {
        assert_eq!(escape_fts5_query("Slow*"), "\"Slow\"*");
        assert_eq!(escape_fts5_query("SlowUpstream*"), "\"SlowUpstream\"*");
    }

    #[test]
    fn test_escape_fts5_query_empty() {
        assert_eq!(escape_fts5_query(""), "");
        assert_eq!(escape_fts5_query("   "), "");
    }

    #[test]
    fn test_escape_fts5_query_with_quotes() {
        assert_eq!(escape_fts5_query("say \"hello\""), "\"say \"\"hello\"\"\"");
    }

    #[test]
    fn test_upsert_and_search() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "src/main.kt", 1000, 100).unwrap();
        assert!(file_id > 0);

        insert_symbol(&conn, file_id, "MyService", SymbolKind::Class, 10, Some("class MyService")).unwrap();
        insert_symbol(&conn, file_id, "processData", SymbolKind::Function, 20, Some("fun processData()")).unwrap();

        let results = search_symbols(&conn, "MyService", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "MyService");
        assert_eq!(results[0].kind, "class");
        assert_eq!(results[0].path, "src/main.kt");
    }

    #[test]
    fn test_search_empty_query() {
        let conn = create_test_db();
        let results = search_symbols(&conn, "", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_find_files() {
        let conn = create_test_db();
        upsert_file(&conn, "src/main.kt", 1000, 100).unwrap();
        upsert_file(&conn, "src/utils/Helper.kt", 2000, 200).unwrap();

        let files = find_files(&conn, "Helper", 10).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].contains("Helper"));
    }

    #[test]
    fn test_find_symbols_by_name() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "src/model.kt", 1000, 100).unwrap();
        insert_symbol(&conn, file_id, "User", SymbolKind::Class, 5, Some("data class User")).unwrap();
        insert_symbol(&conn, file_id, "UserRepository", SymbolKind::Interface, 20, Some("interface UserRepository")).unwrap();

        let results = find_symbols_by_name(&conn, "User", None, 10).unwrap();
        assert!(results.len() >= 1);
        assert!(results.iter().any(|r| r.name == "User"));
    }

    #[test]
    fn test_upsert_file_updates_mtime() {
        let conn = create_test_db();
        let _id1 = upsert_file(&conn, "src/main.kt", 1000, 100).unwrap();
        let id2 = upsert_file(&conn, "src/main.kt", 2000, 200).unwrap();
        assert!(id2 > 0, "upsert should succeed for same path with different mtime");
    }

    #[test]
    fn test_clear_db() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "src/main.kt", 1000, 100).unwrap();
        insert_symbol(&conn, file_id, "Test", SymbolKind::Class, 1, Some("class Test")).unwrap();

        clear_db(&conn).unwrap();

        let results = search_symbols(&conn, "Test", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_get_stats() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "src/main.kt", 1000, 100).unwrap();
        insert_symbol(&conn, file_id, "Foo", SymbolKind::Class, 1, Some("class Foo")).unwrap();
        insert_symbol(&conn, file_id, "bar", SymbolKind::Function, 5, Some("fun bar()")).unwrap();

        let stats = get_stats(&conn).unwrap();
        assert_eq!(stats.file_count, 1);
        assert_eq!(stats.symbol_count, 2);
    }

    #[test]
    fn test_insert_and_find_inheritance() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "src/model.kt", 1000, 100).unwrap();
        insert_symbol(&conn, file_id, "Child", SymbolKind::Class, 1, Some("class Child : Parent()")).unwrap();

        let child_id: i64 = conn.query_row(
            "SELECT id FROM symbols WHERE name = 'Child'", [], |row| row.get(0)
        ).unwrap();
        insert_inheritance(&conn, child_id, "Parent", "extends").unwrap();

        let impls = find_implementations(&conn, "Parent", 10).unwrap();
        assert_eq!(impls.len(), 1);
        assert_eq!(impls[0].name, "Child");
    }

    #[test]
    fn test_count_refs() {
        let conn = create_test_db();
        let count = count_refs(&conn).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_glob_to_like() {
        assert_eq!(glob_to_like("*Mailer"), "%Mailer");
        assert_eq!(glob_to_like("*Email*Service*"), "%Email%Service%");
        assert_eq!(glob_to_like("User?"), "User_");
        assert_eq!(glob_to_like("exact"), "exact");
        // Existing SQL wildcards should be escaped
        assert_eq!(glob_to_like("100%"), "100\\%");
        assert_eq!(glob_to_like("a_b"), "a\\_b");
    }

    #[test]
    fn test_find_class_like_pattern() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "app/mailers/user_mailer.rb", 1000, 100).unwrap();
        insert_symbol(&conn, file_id, "UserMailer", SymbolKind::Class, 1, Some("class UserMailer")).unwrap();
        insert_symbol(&conn, file_id, "AdminMailer", SymbolKind::Class, 10, Some("class AdminMailer")).unwrap();
        insert_symbol(&conn, file_id, "MailerHelper", SymbolKind::Package, 20, Some("module MailerHelper")).unwrap();

        let scope = SearchScope::none();
        // Glob: *Mailer → %Mailer
        let results = find_class_like_pattern(&conn, "%Mailer", 10, &scope).unwrap();
        assert_eq!(results.len(), 2, "should match UserMailer and AdminMailer: {:?}", results.iter().map(|r| &r.name).collect::<Vec<_>>());
        // MailerHelper is a package, should also match class-like kinds
        let results = find_class_like_pattern(&conn, "%Mailer%", 10, &scope).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_find_symbols_by_pattern() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "app/services/email_service.rb", 1000, 100).unwrap();
        insert_symbol(&conn, file_id, "EmailService", SymbolKind::Class, 1, Some("class EmailService")).unwrap();
        insert_symbol(&conn, file_id, "send_email", SymbolKind::Function, 10, Some("def send_email")).unwrap();
        insert_symbol(&conn, file_id, "EmailValidator", SymbolKind::Class, 20, Some("class EmailValidator")).unwrap();

        let scope = SearchScope::none();
        // All symbols matching *Email*
        let results = find_symbols_by_pattern(&conn, "%Email%", None, 10, &scope).unwrap();
        assert_eq!(results.len(), 3);
        // Only classes
        let results = find_symbols_by_pattern(&conn, "%Email%", Some("class"), 10, &scope).unwrap();
        assert_eq!(results.len(), 2);
        // Only functions
        let results = find_symbols_by_pattern(&conn, "%email%", Some("function"), 10, &scope).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "send_email");
    }
}
