# kotlin-index

Fast code search for Android/Kotlin/Java projects using SQLite + FTS5.

**v2.0.0** - Now available as pip package with CLI!

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
| `kotlin-index outline <file>` | Show file structure |

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
