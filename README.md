# kotlin-index

Fast code search for Android/Kotlin/Java projects using SQLite + FTS5.

**v2.5.0** - Compose & Coroutines support, 33 CLI commands!

## Features

- **File search** - by name or path pattern
- **Symbol search** - classes, interfaces, functions, properties with type filtering
- **Find Usages** - find all usages of a symbol in the project
- **Find Implementations** - find interface implementations or class inheritors
- **File outline** - structure with line numbers
- **Modules & dependencies** - Gradle module parsing, dependency graph
- **Kotlin + Java** - both languages supported via tree-sitter
- **Incremental indexing** - update only changed files
- **Fast search** - SQLite + FTS5, millisecond queries

## Installation

### Option 1: pip install (Recommended)

```bash
pip install kotlin-index
```

### Option 2: pip install with MCP support

```bash
pip install kotlin-index[mcp]
```

### Option 3: From source

```bash
git clone https://github.com/defendend/Claude-index-search-android-studio.git
cd Claude-index-search-android-studio
pip install -e .
```

## Quick Start

```bash
# Navigate to your Android project
cd /path/to/android/project

# Initialize index
kotlin-index init

# Search for code
kotlin-index search "PaymentMethod"
kotlin-index class "MainActivity"
kotlin-index usages "UserRepository"
```

## CLI Commands

### Search Commands

| Command | Description |
|---------|-------------|
| `kotlin-index search <query>` | Universal search (files + symbols + modules) |
| `kotlin-index file <query>` | Find files by name |
| `kotlin-index symbol <query>` | Find symbols (classes, functions, etc.) |
| `kotlin-index class <name>` | Find class/interface by name |
| `kotlin-index usages <name>` | Find symbol usages |
| `kotlin-index implementations <name>` | Find interface implementations |
| `kotlin-index hierarchy <class>` | Show class hierarchy (parents + children) |
| `kotlin-index annotations <name>` | Find classes with annotation (@Module, @Inject) |
| `kotlin-index changed` | Show symbols in changed files (git diff) |
| `kotlin-index outline <file>` | Show file structure |
| `kotlin-index callers <function>` | Find where a function is called |
| `kotlin-index imports <file>` | Show imports of a file |
| `kotlin-index provides <type>` | Find @Provides/@Binds for a type |
| `kotlin-index inject <type>` | Find @Inject points for a type |
| `kotlin-index todo` | Find TODO/FIXME/HACK comments |
| `kotlin-index deprecated` | Find @Deprecated items |
| `kotlin-index suppress [warning]` | Find @Suppress annotations |
| `kotlin-index extensions <type>` | Find extension functions for a type |
| `kotlin-index api <module>` | Show public API of a module |
| `kotlin-index deeplinks [query]` | Find deeplinks in AndroidManifest |
| `kotlin-index composables [query]` | Find @Composable functions |
| `kotlin-index previews` | Find @Preview functions |
| `kotlin-index suspend [query]` | Find suspend functions |
| `kotlin-index flows [query]` | Find Flow/StateFlow/SharedFlow usage |

### Module Commands

| Command | Description |
|---------|-------------|
| `kotlin-index module <query>` | Find modules |
| `kotlin-index deps <module>` | Show module dependencies |
| `kotlin-index dependents <module>` | Show modules depending on this one |

### Index Management

| Command | Description |
|---------|-------------|
| `kotlin-index init` | Initialize index for current project |
| `kotlin-index rebuild` | Rebuild entire index |
| `kotlin-index update` | Incremental update (only changed files) |
| `kotlin-index stats` | Show index statistics |

### Examples

```bash
# Find files
kotlin-index file "Fragment.kt"
kotlin-index file --exact "MainActivity.kt"

# Find symbols with type filter
kotlin-index symbol "Repository" --type class
kotlin-index symbol "onClick" --type function

# Find class definition
kotlin-index class "PaymentMethodsFragment"

# Find usages and implementations
kotlin-index usages "UserRepository"
kotlin-index implementations "BasePresenter"

# Module analysis
kotlin-index module "payments"
kotlin-index deps "features.payments.impl"
kotlin-index dependents "features.payments.api"

# File structure
kotlin-index outline "/path/to/File.kt"

# Rebuild specific index type
kotlin-index rebuild --type files
kotlin-index rebuild --type symbols
kotlin-index rebuild --type modules
```

## Claude Code Integration

### Option A: Skill (recommended for token efficiency)

Copy `skills/kotlin-index.md` to your project's `.claude/skills/` directory.

The skill provides Claude with knowledge of all CLI commands without MCP overhead.

### Option B: MCP Server

For IDE-like integration, use the MCP server:

1. Install with MCP support:
   ```bash
   pip install kotlin-index[mcp]
   ```

2. Create `.mcp.json` in project root:
   ```json
   {
     "mcpServers": {
       "kotlin-index": {
         "type": "stdio",
         "command": "kotlin-index",
         "args": ["mcp"],
         "env": {}
       }
     }
   }
   ```

3. Add to `.git/info/exclude`:
   ```
   .mcp.json
   ```

4. Restart Claude Code

### MCP Tools

When using MCP server, the following tools are available:

| Tool | Description |
|------|-------------|
| `find_file(query, limit=20)` | Find files by name |
| `find_file_exact(name)` | Find file by exact name |
| `find_symbol(query, symbol_type?, limit=20)` | Find symbols |
| `find_class(name)` | Find class/interface |
| `get_file_outline(file_path)` | File structure |
| `find_usages(symbol_name, limit=50)` | Find usages |
| `find_implementations(interface_name)` | Find implementations |
| `find_module(query, limit=20)` | Find modules |
| `get_module_deps(module_name)` | Module dependencies |
| `get_module_dependents(module_name)` | Dependents |
| `search(query, limit=10)` | Universal search |
| `rebuild_index(type="all")` | Rebuild index |
| `update_index()` | Incremental update |
| `get_index_stats()` | Statistics |

## Configuration

| Environment Variable | Description |
|---------------------|-------------|
| `KOTLIN_INDEX_PROJECT_ROOT` | Project root (auto-detected) |
| `KOTLIN_INDEX_DB_PATH` | Database path (default: `~/.cache/kotlin-index/index.db`) |

## Symbol Types

- `class` - classes
- `interface` - interfaces
- `object` - Kotlin objects
- `function` - functions
- `property` - properties (val/var)
- `enum` - enum classes

## Architecture

```
kotlin-index/
├── pyproject.toml          # pip packaging
├── plugin.json             # Claude Code plugin config
├── skills/
│   └── kotlin-index.md     # Skill for Claude Code
└── src/kotlin_index/
    ├── __init__.py         # Package init
    ├── cli.py              # CLI (typer)
    ├── server.py           # MCP server (FastMCP)
    ├── db/
    │   ├── database.py     # SQLite
    │   └── schema.py       # DB schema
    └── indexer/
        ├── file_indexer.py    # File indexing
        ├── module_indexer.py  # Gradle parsing
        └── symbol_indexer.py  # Kotlin/Java AST (tree-sitter)
```

## Technologies

- **typer + rich** - CLI framework
- **FastMCP** - MCP server framework
- **SQLite + FTS5** - full-text search
- **tree-sitter-kotlin** - Kotlin AST parsing
- **tree-sitter-java** - Java AST parsing

## Performance

| Operation | Time |
|-----------|------|
| Full indexing | ~60 sec* |
| Search | < 100 ms |

*Depends on project size

## When to Rebuild

Use incremental updates for:
- After editing files
- After `git pull` / `git checkout`

Use full rebuild for:
- After adding/removing many files
- Index issues

## Troubleshooting

### "too many SQL variables"
Operations are batched in `db/database.py`.

### Modules = 0
Check file filter in `module_indexer.py`.

### Symbols not found by type
Check tree-sitter node types in `symbol_indexer.py`.

## Changelog

### v2.5.0
- Add `composables` command: find @Composable functions with optional filter
- Add `previews` command: find @Preview functions for Compose
- Add `suspend` command: find suspend functions with optional filter
- Add `flows` command: find Flow/StateFlow/SharedFlow usage, grouped by type

### v2.4.1
- Fix `callers` command: now uses grep-based search instead of references index
- Fix `outline` command: support relative paths
- Fix `api` command: accept both path format (features/payments/api) and dot format (features.payments.api)

### v2.4.0
- Add `todo` command: find TODO/FIXME/HACK comments with grouping
- Add `deprecated` command: find @Deprecated classes and functions
- Add `suppress` command: audit @Suppress annotations, grouped by warning type
- Add `extensions` command: find extension functions for a type
- Add `api` command: show public API of a module (classes, functions)
- Add `deeplinks` command: find deeplinks in AndroidManifest and @DeepLink annotations

### v2.3.0
- Add `callers` command: find where a function is called (uses references index)
- Add `imports` command: show imports of a file
- Add `provides` command: find @Provides/@Binds methods that provide a type (Dagger DI)
- Add `inject` command: find where a type is injected (@Inject constructor/field)

### v2.2.0
- Add `hierarchy` command: show class hierarchy (parents and children)
- Add `annotations` command: find classes with specific annotation (@Module, @Inject, etc.)
- Add `changed` command: show symbols in changed files (git diff)

### v2.1.0
- Fix `class` command: now supports contains search (e.g., `kotlin-index class Interactor`)
- Add `update` command for incremental indexing
- Add `--limit` option to `class` command

### v2.0.0
- pip package installation (`pip install kotlin-index`)
- CLI with typer + rich
- Skill for Claude Code
- plugin.json for `/plugins install`
- MCP server as optional (`pip install kotlin-index[mcp]`)

### v1.2.0
- Java support (tree-sitter-java)
- Find Usages
- Find Implementations
- Generic type inheritance support

### v1.1.0
- Incremental indexing
- Better module detection

### v1.0.0
- Initial release
- File/symbol/module search
- MCP server

## License

MIT
