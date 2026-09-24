# Index database schema

ast-index stores each project's code graph in a SQLite database under
`~/Library/Caches/ast-index/<hash>/index.db` on macOS or
`$XDG_CACHE_HOME/ast-index/<hash>/index.db` on Linux. `<hash>` is derived from
the normalized project root.

The authoritative DDL lives in `src/db.rs`. The current schema has 19 base
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
                                 symbols ── id match ── symbol_edges / symbol_metrics
git_commits ── git_commit_changes ── git_paths
   (fold of the live commits)
   └──▶ git_file_stats ── git_file_authors
```

The line between `subtrees.canonical_path` and `files.root_path` is a value
relationship, not a foreign key.

## Base tables

`PK` means primary key, `NN` means `NOT NULL`, and `UQ` means `UNIQUE`.
Every declared foreign key below uses `ON DELETE CASCADE`.

| Table | Columns | Declared foreign keys and constraints |
|---|---|---|
| `files` | `id INTEGER PK`, `path TEXT NN`, `root_path TEXT NN DEFAULT ''`, `mtime INTEGER NN`, `size INTEGER NN` | `UNIQUE(root_path, path)` |
| `symbols` | `id INTEGER PK`, `file_id INTEGER NN`, `name TEXT NN`, `qualified_name TEXT`, `kind TEXT NN`, `line INTEGER NN`, `end_line INTEGER`, `parent_id INTEGER`, `signature TEXT` | `file_id → files.id` |
| `modules` | `id INTEGER PK`, `name TEXT NN UQ`, `path TEXT NN`, `kind TEXT` | — |
| `module_deps` | `id INTEGER PK`, `module_id INTEGER NN`, `dep_module_id INTEGER NN`, `dep_kind TEXT` | `module_id → modules.id`, `dep_module_id → modules.id` |
| `inheritance` | `id INTEGER PK`, `child_id INTEGER NN`, `parent_name TEXT NN`, `kind TEXT NN` | `child_id → symbols.id` |
| `refs` | `id INTEGER PK`, `file_id INTEGER NN`, `name TEXT NN`, `line INTEGER NN`, `context TEXT` | `file_id → files.id` |
| `file_words` | `file_id INTEGER PK`, `mtime INTEGER NN`, `size INTEGER NN`, `words TEXT NN` | `file_id → files.id ON DELETE CASCADE` |
| `xml_usages` | `id INTEGER PK`, `module_id INTEGER`, `file_path TEXT NN`, `line INTEGER NN`, `class_name TEXT NN`, `usage_type TEXT`, `element_id TEXT` | `module_id → modules.id` |
| `resources` | `id INTEGER PK`, `module_id INTEGER`, `type TEXT NN`, `name TEXT NN`, `file_path TEXT NN`, `line INTEGER` | `module_id → modules.id` |
| `resource_usages` | `id INTEGER PK`, `resource_id INTEGER`, `usage_file TEXT NN`, `usage_line INTEGER NN`, `usage_type TEXT` | `resource_id → resources.id` |
| `transitive_deps` | `id INTEGER PK`, `module_id INTEGER NN`, `dependency_id INTEGER NN`, `depth INTEGER NN`, `path TEXT` | `module_id → modules.id`, `dependency_id → modules.id` |
| `storyboard_usages` | `id INTEGER PK`, `module_id INTEGER`, `file_path TEXT NN`, `line INTEGER NN`, `class_name TEXT NN`, `usage_type TEXT`, `storyboard_id TEXT` | `module_id → modules.id` |
| `ios_assets` | `id INTEGER PK`, `module_id INTEGER`, `type TEXT NN`, `name TEXT NN`, `file_path TEXT NN` | `module_id → modules.id` |
| `ios_asset_usages` | `id INTEGER PK`, `asset_id INTEGER`, `usage_file TEXT NN`, `usage_line INTEGER NN`, `usage_type TEXT` | `asset_id → ios_assets.id` |
| `metadata` | `key TEXT PK`, `value TEXT NN` | — |
| `subtrees` | `id INTEGER PK`, `name TEXT NN UQ`, `canonical_path TEXT NN UQ`, `original_path TEXT NN` | — |
| `git_file_stats` | `path TEXT PK`, `commits`, `fix_commits`, `lines_added`, `lines_deleted` (`INTEGER NN DEFAULT 0`), `first_commit_at INTEGER`, `last_commit_at INTEGER`, `current_lines INTEGER` | — |
| `git_file_authors` | `path TEXT NN`, `author TEXT NN` | `PRIMARY KEY(path, author)` |
| `git_commits` | `id INTEGER PK`, `sha TEXT NN UQ`, `order_key INTEGER NN`, `live INTEGER NN`, `authored_at INTEGER`, `author TEXT`, `is_fix INTEGER` | — |
| `git_paths` | `id INTEGER PK`, `hash INTEGER NN`, `path TEXT NN` | — |
| `git_commit_changes` | `commit_id INTEGER NN`, `path_id INTEGER NN`, `kind INTEGER NN`, `from_path_id INTEGER`, `added INTEGER NN`, `deleted INTEGER NN` | `PRIMARY KEY(commit_id, path_id)`, `WITHOUT ROWID` |
| `symbol_edges` | `source_id INTEGER NN`, `target_id INTEGER NN`, `confidence INTEGER NN`, `candidates INTEGER NN`, `ref_count INTEGER NN`, `line INTEGER NN` | `PRIMARY KEY(source_id, target_id)`, `WITHOUT ROWID` |
| `symbol_metrics` | `symbol_id INTEGER PK`, `fan_in`, `fan_in_files`, `fan_in_ambiguous`, `fan_out`, `fan_out_ambiguous`, `dependents` (`INTEGER NN`), `pagerank REAL NN`, `pagerank_pct REAL NN` | — |

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
root. Per-file lookups (`find_owning_symbol`, `get_file_symbols`,
`file_has_symbol_ranges`) take the owning root along with the path, so a path
shared by two roots never answers from the wrong one; for the primary root,
`''` and the normalized path recorded as metadata `project_root` are treated
as the same root. Attached roots are registered in `subtrees`; `original_path` preserves
what the user entered, while `canonical_path` is the normalized value used in
`files.root_path`.

### File words

`file_words` holds the distinct word runs (`[\p{Alnum}_]+`) of each indexed
file's text, sorted and newline-joined. Grep-based commands (`search`
contents, `callers`, `call-tree`) skip a file only when its words lack a run
containing every searched literal and its current `mtime` / `size` equal the
stored ones; a file without a row, or changed since indexing, is searched in
full. No row is written for minified files, files over the size cap or `.d.ts`
from `node_modules`.

### Symbols, references, and inheritance

`symbols.qualified_name` stores the parser-provided qualified name when one is
available. `signature` is nullable because not every language or declaration
has a useful signature.

`symbols.line` is the 1-based line where the definition starts; `end_line` is
the 1-based inclusive line where it ends, so a class range encloses the ranges
of its own methods. `end_line` is nullable: only parsers that report a range
fill it. Every tree-sitter parser does except CSS, SCSS and Less; the regex
parsers (Perl, WSDL/XSD, Vue and Svelte script blocks) store `NULL`. Within a
ranged language a few symbols still store `NULL`: 1C `#Область` regions, which
fold code without scoping it, and SQL `CREATE DOMAIN` statements. The range
starts at `line`, the declaration line, so decorators, annotations and
attributes written above a declaration fall outside it; they are indexed as
`annotation` symbols with ranges of their own. A declaration that scopes the
rest of its file without enclosing it in the syntax tree — a C#
`namespace A.B;`, a PHP `namespace A\B;`, a GDScript `class_name` — ends where
its scope does. Existing databases gain the column through an `ALTER TABLE` on
open and keep `NULL` until the affected files are re-indexed, so consumers must
treat `NULL` as "range unknown" rather than an error.

`symbols.parent_id` is a reserved, nullable compatibility column. It has no
foreign-key constraint, and current indexing code does not populate it.
Consumers must not treat it as a containment hierarchy.

`refs.name` and `inheritance.parent_name` are intentionally string-based:
neither points to a specific `symbols` row. References therefore remain
language-agnostic, and an inheritance parent may live outside the index.
`inheritance.child_id` is the resolved side of an inheritance edge.

### VCS history signals

Git history is collected on demand by `hotspots --collect` into a per-commit
store, and the per-file tables are derived from it.

- `git_commits` has one row per commit the collector has seen, merges and
  commits outside the project included. `live = 1` marks the commits the
  collected HEAD reaches; the others are kept so switching back to a branch
  does not re-read its diffs, until they outnumber `max(1000, live / 4)` and
  are dropped all at once. `order_key` is the corrected commit date: the
  committer date, raised to one past the latest parent's, so it never puts a
  commit before its parent and does not depend on the range a commit was read
  in. `authored_at`, `author` (lower-cased email) and `is_fix` (bugfix subject
  heuristic) are `NULL` for commits that did not change the project.
- `git_paths` interns project-relative paths; `hash` is a 64-bit FNV-1a of the
  path, indexed instead of the text.
- `git_commit_changes` records what a commit did to a path: `kind = 0` a
  change of `path_id`, `1` a rename from `from_path_id` to `path_id`, `2`
  `path_id` moved out of the project. `added` / `deleted` are numstat line
  counts (0 for binary files).
- `git_file_stats` and `git_file_authors` fold the live commits in
  `(order_key, sha)` order, a rename handing the old path's history to the new
  one. `path` is relative to the project root, the same key space as
  `files.path`; rows exist only for paths present in the working tree,
  including paths the indexer never parses.

When HEAD moves, commits only the old HEAD reaches leave the live set, commits
only the new HEAD reaches join it, and only the paths those commits touched
(plus paths linked to them by renames) are refolded. Merges record no
changes, so when one joins or leaves, every path that differs between the old
and the new HEAD (`git diff-tree --name-only`) is refolded as well: a file
only a merge edited or deleted gets its `current_lines` and its row from the
working tree again. The result equals a full collection at the same HEAD row
for row. `current_lines` is read from the working tree when a path is
refolded, so an uncommitted edit reaches it on the next refold of that path
or on `--full`.

The history does not depend on the code index, so a full `rebuild` copies all
five tables and every `git_signals_*` metadata key from the live generation
into the staged one, in one transaction over one snapshot, before the staged
generation is sealed and published. It copies nothing, leaving the tables
empty for the next `hotspots --collect`, when `git_signals_store` is not the
current layout, `git_signals_repo_root` / `git_signals_scope` do not match the
project's working tree, `git_signals_commits` disagrees with the live commits
in the store, a table's columns differ from the current schema, or reading
the live generation fails. A partial
`rebuild --type …` starts from a copy of the live generation and keeps the
history with everything else.

### Symbol graph

`symbol_edges` and `symbol_metrics` are filled only by `graph build`; `rebuild`
starts a database with both tables empty and `update` never writes them.

An edge `source_id -> target_id` means the symbol that owns a reference (the
narrowest definition containing its line, as `find_owning_symbol` computes it;
import and annotation lines count for the definition around them) depends on
the definition that reference names. `refs` rows are folded per pair:
`ref_count` is how many references, `line` the first one. `confidence` records
how the target was chosen among same-named definitions: `0` local (same file),
`1` scoped (namespace path, lexical nesting, constant receiver, inheritance),
`2` import, `3` unique, `4` ambiguous. An ambiguous reference is stored as one
edge per candidate, each with `candidates = k` (up to 8; references with more
candidates are not stored). Resolved edges have `candidates = 1`.

Files under a `node_modules` path segment (`db::is_third_party_path`) are
neither sources nor targets of edges; a project's `vendor/` directory and its
own `.d.ts` files are ordinary nodes. Search ranking demotes a wider set
(`db::is_vendor_path`: packages plus every `.d.ts`), because a declaration
should rank below the implementation it describes, while a graph edge into
the project's own declaration file is still a real dependency.

Rails schema dumps add two symbol kinds: `table` (one per `create_table`) and
`column` (named `table.column`). They are never matched by name: a column is
an edge target only for code inside a model whose table declares it (the
model-to-table match follows Active Record's naming rules; `graph build`
stores what it matched in `symbol_graph_summary.schema`).

`symbol_metrics` has one row per symbol that touches any edge. `fan_in`,
`fan_out`, `fan_in_files`, `dependents` (distinct symbols reaching this one
within 3 hops) and `pagerank` count resolved edges only; `fan_in_ambiguous`
and `fan_out_ambiguous` count the ambiguous ones. `pagerank` is scaled so the
average node scores 1.0; `pagerank_pct` is its midrank percentile.

Neither table declares a foreign key: a cascading delete would slow every
incremental update, and rows that silently disappeared would make an outdated
graph look current. Instead `graph build` stores the index's write generation
(`index_generation`) and the highest row ids of `files`, `symbols` and `refs`
as `symbol_graph_fingerprint`, and queries compare it with the live index — a
metadata read and three rowid seeks, not a scan of the tables.

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
| `minified_filter` | `1` while no minified JavaScript/CSS file is in `files`, written by `rebuild` and `update` with the filter on. An index from an older version or built with `AST_INDEX_SKIP_MINIFIED=0` lacks it; the next `update` with the filter on then checks unchanged files too and drops the minified ones. |
| `git_signals_head`, `git_signals_repo_root`, `git_signals_scope`, `git_signals_collected_at` | Commit cursor and bookkeeping of the last `hotspots --collect`. |
| `git_signals_commits` | Live commits that changed the project (the analysed history). |
| `git_signals_paths` | Paths that carry history once renames are followed, deleted ones included. |
| `git_signals_store` | Layout of the per-commit store (`commits-v2`, bumped when a stored column such as `is_fix` changes meaning); history collected with another layout or without it is recollected once. |
| `index_generation` | Count of the writes to `files`, `symbols`, `refs` and `inheritance` (each `rebuild` / `update` batch, file deletion, clear); an update that finds nothing to change leaves it alone. Absent in an index no version with the counter has written, read as 0. |
| `symbol_graph_fingerprint` | `generation:<n>/ids:<files>:<symbols>:<refs>` — the `index_generation` and the highest row ids when the graph was built. The ids also move when a version without the counter re-indexes files. A mismatch, including the row-count digest older versions stored, marks the graph stale. |
| `symbol_graph_built_at` | Unix timestamp in milliseconds of the last `graph build`. |
| `symbol_graph_summary` | JSON summary of the last build: edges and references per confidence level, references not linked per reason. |

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
  `idx_symbols_file_line_end` on `(file_id, line, end_line)` (covers "which
  symbol contains this line"; its `file_id` prefix serves every per-file
  lookup and the cascade from `files`).
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
- Symbol graph:
  `idx_symbol_edges_target` on `(target_id, confidence)` for incoming-edge
  lookups (outgoing ones use the primary key). It is created with the table
  and dropped/recreated around each `graph build` bulk load.
- Git history:
  `idx_git_commit_changes_path` on `path_id`,
  `idx_git_commit_changes_renames` (partial, `kind = 1`) on
  `(commit_id, from_path_id, path_id)`, and
  `idx_git_paths_hash` on `hash`.
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
- `idx_symbols_file` on `(file_id)`: the leftmost prefix of
  `idx_symbols_file_line_end`.

Older databases drop those indexes when opened. The qualified-name index is
also migrated to its current partial definition, a missing
`symbols.end_line` column is added, and a missing
`idx_symbols_file_line_end` is created (always before `idx_symbols_file` is
dropped). This optimization changes index structures only: all base tables
and their raw columns remain available to `ast-index query` and
`ast-index schema` for compatibility.

`restore` applies the same migrations to its private snapshot before it
validates the snapshot, so a backup taken in any earlier index layout restores
into the current one. A backup made by this version lacks `idx_symbols_file`,
which the validation of older releases requires: restore it with this version
or later.

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
