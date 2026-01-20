use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

/// Get the database path for the current project
pub fn get_db_path(project_root: &Path) -> Result<PathBuf> {
    // Check environment variable first
    if let Ok(path) = std::env::var("KOTLIN_INDEX_DB_PATH") {
        return Ok(PathBuf::from(path));
    }

    // Default to ~/.cache/kotlin-index/<project_hash>/index.db
    let cache_dir = dirs::cache_dir()
        .context("Could not find cache directory")?
        .join("kotlin-index");

    // Create hash from project root for unique DB per project
    let project_hash = simple_hash(project_root.to_string_lossy().as_ref());
    let db_dir = cache_dir.join(project_hash);

    std::fs::create_dir_all(&db_dir)?;
    Ok(db_dir.join("index.db"))
}

fn simple_hash(s: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:x}", hasher.finish())
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
    conn.pragma_update(None, "cache_size", "-64000")?;

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
    Property,
    TypeAlias,
}

impl SymbolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SymbolKind::Class => "class",
            SymbolKind::Interface => "interface",
            SymbolKind::Object => "object",
            SymbolKind::Enum => "enum",
            SymbolKind::Function => "function",
            SymbolKind::Property => "property",
            SymbolKind::TypeAlias => "typealias",
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
    // Wrap in double quotes to treat as literal phrase
    // Escape any existing double quotes
    let escaped = query.replace('"', "\"\"");
    format!("\"{}\"", escaped)
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
#[derive(Debug)]
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
        WHERE s.name = ?1 AND s.kind IN ('class', 'interface', 'object', 'enum')
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

/// Find implementations (subclasses/implementors)
pub fn find_implementations(
    conn: &Connection,
    parent_name: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    // Match exact name OR names ending with .ParentName (for qualified names)
    let pattern = format!("%.{}", parent_name);
    let mut stmt = conn.prepare(
        r#"
        SELECT s.name, s.kind, s.line, s.signature, f.path
        FROM inheritance i
        JOIN symbols s ON i.child_id = s.id
        JOIN files f ON s.file_id = f.id
        WHERE i.parent_name = ?1 OR i.parent_name LIKE ?2
        LIMIT ?3
        "#,
    )?;

    let results = stmt
        .query_map(params![parent_name, pattern, limit as i64], |row| {
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

/// Get database statistics
pub fn get_stats(conn: &Connection) -> Result<DbStats> {
    let file_count: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;
    let symbol_count: i64 = conn.query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))?;
    let module_count: i64 = conn.query_row("SELECT COUNT(*) FROM modules", [], |row| row.get(0))?;
    let refs_count: i64 = conn.query_row("SELECT COUNT(*) FROM refs", [], |row| row.get(0)).unwrap_or(0);

    Ok(DbStats {
        file_count,
        symbol_count,
        module_count,
        refs_count,
    })
}

#[derive(Debug)]
pub struct DbStats {
    pub file_count: i64,
    pub symbol_count: i64,
    pub module_count: i64,
    pub refs_count: i64,
}

/// Clear all data from the database
pub fn clear_db(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
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
#[derive(Debug)]
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
    let mut stmt = conn.prepare(
        r#"
        SELECT r.name, r.line, r.context, f.path
        FROM refs r
        JOIN files f ON r.file_id = f.id
        WHERE r.name = ?1
        ORDER BY f.path, r.line
        LIMIT ?2
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

/// Count references in the database
pub fn count_refs(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM refs", [], |row| row.get(0))?)
}
