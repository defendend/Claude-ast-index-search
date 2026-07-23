# Index database schema

ast-index stores each project's code graph in a SQLite database under
`~/Library/Caches/ast-index/<hash>/index.db` on macOS or
`$XDG_CACHE_HOME/ast-index/<hash>/index.db` on Linux. `<hash>` is derived from
the normalized project root.

The authoritative DDL lives in `src/db.rs`. The current schema has 15 base
tables plus the `symbols_fts` FTS5 virtual table. Fresh rebuilds create the
base tables first, bulk-load data, and add secondary indexes and FTS only
afterward.

## Logical overview

```text
subtrees.canonical_path ── value match ── files.root_path
                                              │
                                  ┌───────────┴───────────┐
                                  ▼                       ▼
                               symbols                   refs
                                  │
                                  ▼
                             inheritance

modules ── module_deps
   │    └─ transitive_deps
   ├────── resources ── resource_usages
   ├────── xml_usages
   ├────── ios_assets ── ios_asset_usages
   └────── storyboard_usages

metadata                         symbols ── external content ── symbols_fts
```

The line between `subtrees.canonical_path` and `files.root_path` is a value
relationship, not a foreign key.

## Base tables

`PK` means primary key, `NN` means `NOT NULL`, and `UQ` means `UNIQUE`.
Every declared foreign key below uses `ON DELETE CASCADE`.

| Table | Columns | Declared foreign keys and constraints |
|---|---|---|
| `files` | `id INTEGER PK`, `path TEXT NN`, `root_path TEXT NN DEFAULT ''`, `mtime INTEGER NN`, `size INTEGER NN` | `UNIQUE(root_path, path)` |
| `symbols` | `id INTEGER PK`, `file_id INTEGER NN`, `name TEXT NN`, `qualified_name TEXT`, `kind TEXT NN`, `line INTEGER NN`, `parent_id INTEGER`, `signature TEXT` | `file_id → files.id` |
| `modules` | `id INTEGER PK`, `name TEXT NN UQ`, `path TEXT NN`, `kind TEXT` | — |
| `module_deps` | `id INTEGER PK`, `module_id INTEGER NN`, `dep_module_id INTEGER NN`, `dep_kind TEXT` | `module_id → modules.id`, `dep_module_id → modules.id` |
| `inheritance` | `id INTEGER PK`, `child_id INTEGER NN`, `parent_name TEXT NN`, `kind TEXT NN` | `child_id → symbols.id` |
| `refs` | `id INTEGER PK`, `file_id INTEGER NN`, `name TEXT NN`, `line INTEGER NN`, `context TEXT` | `file_id → files.id` |
| `xml_usages` | `id INTEGER PK`, `module_id INTEGER`, `file_path TEXT NN`, `line INTEGER NN`, `class_name TEXT NN`, `usage_type TEXT`, `element_id TEXT` | `module_id → modules.id` |
| `resources` | `id INTEGER PK`, `module_id INTEGER`, `type TEXT NN`, `name TEXT NN`, `file_path TEXT NN`, `line INTEGER` | `module_id → modules.id` |
| `resource_usages` | `id INTEGER PK`, `resource_id INTEGER`, `usage_file TEXT NN`, `usage_line INTEGER NN`, `usage_type TEXT` | `resource_id → resources.id` |
| `transitive_deps` | `id INTEGER PK`, `module_id INTEGER NN`, `dependency_id INTEGER NN`, `depth INTEGER NN`, `path TEXT` | `module_id → modules.id`, `dependency_id → modules.id` |
| `storyboard_usages` | `id INTEGER PK`, `module_id INTEGER`, `file_path TEXT NN`, `line INTEGER NN`, `class_name TEXT NN`, `usage_type TEXT`, `storyboard_id TEXT` | `module_id → modules.id` |
| `ios_assets` | `id INTEGER PK`, `module_id INTEGER`, `type TEXT NN`, `name TEXT NN`, `file_path TEXT NN` | `module_id → modules.id` |
| `ios_asset_usages` | `id INTEGER PK`, `asset_id INTEGER`, `usage_file TEXT NN`, `usage_line INTEGER NN`, `usage_type TEXT` | `asset_id → ios_assets.id` |
| `metadata` | `key TEXT PK`, `value TEXT NN` | — |
| `subtrees` | `id INTEGER PK`, `name TEXT NN UQ`, `canonical_path TEXT NN UQ`, `original_path TEXT NN` | — |

### File identity and roots

`files.path` is relative to the source root that owns the file.
`files.root_path` records that root's normalized absolute path. A primary
project and an attached subtree may contain the same relative path, so file
identity is the pair `(root_path, path)`, enforced by
`UNIQUE(root_path, path)`.

`mtime` and `size` are the incremental freshness inputs. A file is considered
unchanged only when both values still match.

The empty-string default on `root_path` remains for compatibility with older
databases and direct compatibility helpers. Current indexing writes an owning
root. Attached roots are registered in `subtrees`; `original_path` preserves
what the user entered, while `canonical_path` is the normalized value used in
`files.root_path`.

### Symbols, references, and inheritance

`symbols.qualified_name` stores the parser-provided qualified name when one is
available. `signature` is nullable because not every language or declaration
has a useful signature.

`symbols.parent_id` is a reserved, nullable compatibility column. It has no
foreign-key constraint, and current indexing code does not populate it.
Consumers must not treat it as a containment hierarchy.

`refs.name` and `inheritance.parent_name` are intentionally string-based:
neither points to a specific `symbols` row. References therefore remain
language-agnostic, and an inheritance parent may live outside the index.
`inheritance.child_id` is the resolved side of an inheritance edge.

### Platform-specific data

The Android tables are `resources`, `resource_usages`, and `xml_usages`.
The iOS tables are `ios_assets`, `ios_asset_usages`, and
`storyboard_usages`. Their nullable `module_id` or asset/resource IDs allow
data to be recorded even when the corresponding owning row cannot be
resolved.

## Metadata keys

`metadata` is a string-to-string store. Production code currently uses these
keys:

| Key | Meaning |
|---|---|
| `project_root` | Normalized project root used to validate cache ownership and migrations. |
| `no_ignore` | `1` when indexing was configured to include ignored files. |
| `bypass_size_check` | Persistent opt-in created by `rebuild --force --remember` to bypass the candidate-file cap. |
| `experimental_fast_rebuild` | `1` or `0`, recording the experimental rebuild mode for later commands. |
| `last_update_at` | Unix timestamp in milliseconds for the last completed file-index update. |
| `index_update_dirty_at` | Unix timestamp in milliseconds marking an incremental update that may be partial; removed when completion is published. |
| `last_modules_indexed_at` | Unix timestamp in milliseconds for completed module indexing. |

`extra_roots` is a legacy migration input only. On open, its JSON array is
moved into `subtrees` and the metadata row is deleted. There is no
`schema_version` metadata key in the current implementation.

## Secondary indexes

The current explicit secondary indexes are:

- Files and symbols:
  `idx_files_path`,
  `idx_symbols_name`,
  `idx_symbols_qualified_name` (partial, only where `qualified_name IS NOT NULL`),
  `idx_symbols_kind`, and
  `idx_symbols_file`.
- Modules and dependency edges:
  `idx_module_deps_module`,
  `idx_module_deps_dep`,
  `idx_transitive_deps_module`, and
  `idx_transitive_deps_dep`.
- Inheritance and references:
  `idx_inheritance_child`,
  `idx_inheritance_parent`,
  `idx_refs_file`, and
  `idx_refs_name_file_line` on `(name, file_id, line)`.
- Android data:
  `idx_xml_usages_class`,
  `idx_xml_usages_module`,
  `idx_resources_name`,
  `idx_resources_type`,
  `idx_resources_module`, and
  `idx_resource_usages_resource`.
- iOS data:
  `idx_storyboard_usages_class`,
  `idx_storyboard_usages_module`,
  `idx_ios_assets_name`,
  `idx_ios_assets_type`, and
  `idx_ios_asset_usages_asset`.

SQLite also creates indexes for the schema's `PRIMARY KEY` and `UNIQUE`
constraints where needed, including `files(root_path, path)`, `modules(name)`,
`subtrees(name)`, and `subtrees(canonical_path)`.

Fresh databases intentionally do not create these redundant historical
indexes:

- `idx_files_root_path_path`: duplicates `UNIQUE(root_path, path)`.
- `idx_modules_name`: duplicates `UNIQUE(name)`.
- `idx_refs_name`: the leftmost `name` prefix is already covered by
  `idx_refs_name_file_line`.

Older databases drop those indexes when opened. The qualified-name index is
also migrated to its current partial definition. This optimization changes
index structures only: all 15 base tables and their raw columns remain
available to `ast-index query` and `ast-index schema` for compatibility.

## Full-text search

`symbols_fts` is an FTS5 external-content virtual table:

```sql
CREATE VIRTUAL TABLE symbols_fts USING fts5(
    name,
    signature,
    content=symbols,
    content_rowid=id
);
```

It indexes `symbols.name` and `symbols.signature`, using `symbols.id` as the
row ID. The `symbols_ai`, `symbols_ad`, and `symbols_au` triggers synchronize
inserts, deletes, and updates. FTS is rebuilt after a fresh bulk load.

## Common query patterns

Find a symbol by qualified name:

```sql
SELECT s.name, s.qualified_name, s.kind, f.root_path, f.path, s.line
FROM symbols AS s
JOIN files AS f ON f.id = s.file_id
WHERE s.qualified_name = ?1;
```

Find implementations or subclasses by parent name:

```sql
SELECT s.name, f.root_path, f.path, s.line
FROM inheritance AS i
JOIN symbols AS s ON s.id = i.child_id
JOIN files AS f ON f.id = s.file_id
WHERE i.parent_name = ?1
   OR i.parent_name LIKE '%.' || ?1;
```

Address one file unambiguously:

```sql
SELECT s.name, s.kind, s.line
FROM symbols AS s
JOIN files AS f ON f.id = s.file_id
WHERE f.root_path = ?1 AND f.path = ?2
ORDER BY s.line;
```

## Inspecting the live database

```bash
ast-index db-path
ast-index schema
ast-index query "SELECT * FROM symbols WHERE name = ?1 LIMIT 20" foo

sqlite3 "$(ast-index db-path)" ".indexes"
sqlite3 "$(ast-index db-path)" ".schema"
```

`ast-index query` accepts read-only `SELECT`, `WITH`, and `EXPLAIN`
statements. Use the SQLite CLI only when the complete raw schema, including
indexes, virtual tables, and triggers, is needed.
