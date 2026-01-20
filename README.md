# kotlin-index v3.0.0

Fast code search CLI for Android/Kotlin/Java projects. Native Rust implementation.

## Installation

### Homebrew (macOS/Linux)

```bash
brew tap defendend/kotlin-index
brew install kotlin-index
```

### From source

```bash
git clone https://github.com/defendend/Claude-index-search-android-studio.git
cd Claude-index-search-android-studio
cargo build --release
# Binary: target/release/kotlin-index (~4.4 MB)
```

## Quick Start

```bash
cd /path/to/android/project

# Build index
kotlin-index rebuild

# Search
kotlin-index search ViewModel
kotlin-index class BaseFragment
kotlin-index implementations Presenter
kotlin-index usages Repository
```

## Commands (33)

### Grep-based (no index required)

```bash
kotlin-index todo [PATTERN]           # TODO/FIXME/HACK comments
kotlin-index callers <FUNCTION>       # Function call sites
kotlin-index provides <TYPE>          # @Provides/@Binds for type
kotlin-index suspend [QUERY]          # Suspend functions
kotlin-index composables [QUERY]      # @Composable functions
kotlin-index deprecated [QUERY]       # @Deprecated items
kotlin-index suppress [QUERY]         # @Suppress annotations
kotlin-index inject <TYPE>            # @Inject points
kotlin-index annotations <ANN>        # Classes with annotation
kotlin-index deeplinks [QUERY]        # Deeplinks
kotlin-index extensions <TYPE>        # Extension functions
kotlin-index flows [QUERY]            # Flow/StateFlow/SharedFlow
kotlin-index previews [QUERY]         # @Preview functions
kotlin-index usages <SYMBOL>          # Symbol usages (falls back to grep)
```

### Index-based (requires rebuild)

```bash
kotlin-index search <QUERY>           # Universal search
kotlin-index file <PATTERN>           # Find files
kotlin-index symbol <NAME>            # Find symbols
kotlin-index class <NAME>             # Find classes/interfaces
kotlin-index implementations <PARENT> # Find implementations
kotlin-index hierarchy <CLASS>        # Class hierarchy tree
kotlin-index usages <SYMBOL>          # Symbol usages (indexed, ~8ms)
```

### Module analysis

```bash
kotlin-index module <PATTERN>         # Find modules
kotlin-index deps <MODULE>            # Module dependencies
kotlin-index dependents <MODULE>      # Dependent modules
kotlin-index api <MODULE>             # Public API of module
```

**Note:** For deps/dependents, index dependencies first:
```bash
kotlin-index rebuild --deps
```

### File analysis

```bash
kotlin-index outline <FILE>           # Symbols in file
kotlin-index imports <FILE>           # Imports in file
kotlin-index changed [--base BRANCH]  # Changed symbols (git diff)
```

### Index management

```bash
kotlin-index init                     # Initialize DB
kotlin-index rebuild [--type TYPE]    # Full reindex
kotlin-index update                   # Incremental update
kotlin-index stats                    # Index statistics
kotlin-index version                  # Version info
```

## Performance

Benchmarks on large Android project (~29k files, ~300k symbols):

### Speed Comparison (vs Python)

| Category | Rust Wins | Python Wins | Equal |
|----------|-----------|-------------|-------|
| Grep-based (14) | 10 | 3 | 1 |
| Index-based (6) | 5 | 1 | 0 |
| Modules (4) | 4 | 0 | 0 |
| Files (3) | 3 | 0 | 0 |
| Management (6) | 4 | 1 | 1 |
| **TOTAL** | **26** | **5** | **2** |

### Top Speedups

| Command | Rust | Python | Speedup |
|---------|------|--------|---------|
| imports | 0.3ms | 90ms | **260x** |
| dependents | 2ms | 100ms | **100x** |
| deps | 3ms | 90ms | **90x** |
| class | 1ms | 90ms | **90x** |
| search | 11ms | 280ms | **14x** |
| usages | 8ms | 90ms | **12x** |

### Full Benchmark Results

#### Grep-based commands

| Command | Rust | Python | Winner |
|---------|------|--------|--------|
| todo | 0.79s | 1.43s | Rust 1.8x |
| callers | 1.02s | 1.24s | Rust 1.2x |
| provides | 1.76s | 1.61s | Python 1.1x |
| suspend | 0.93s | 1.46s | Rust 1.6x |
| composables | 1.35s | 1.28s | Python 1.1x |
| deprecated | 1.20s | 1.06s | Python 1.1x |
| suppress | 1.14s | 1.15s | Equal |
| inject | 0.59s | 2.57s | Rust 4.4x |
| annotations | 1.07s | 1.21s | Rust 1.1x |
| deeplinks | 1.29s | 1.45s | Rust 1.1x |
| extensions | 1.09s | 1.15s | Rust 1.1x |
| flows | 1.13s | 1.11s | Equal |
| previews | 1.11s | 1.18s | Rust 1.1x |
| usages | 0.008s | 0.09s | Rust 12x |

#### Index-based commands

| Command | Rust | Python | Winner |
|---------|------|--------|--------|
| search | 0.02s | 0.28s | Rust 14x |
| file | 0.03s | 0.09s | Rust 3x |
| symbol | 0.36s | 0.10s | Python 3.6x |
| class | 0.00s | 0.09s | Rust 90x |
| implementations | 0.03s | 0.42s | Rust 14x |
| hierarchy | 0.03s | 0.07s | Rust 2.3x |

#### Module commands

| Command | Rust | Python | Winner |
|---------|------|--------|--------|
| module | 0.01s | 0.15s | Rust 15x |
| deps | 0.00s | 0.09s | Rust 90x |
| dependents | 0.00s | 0.10s | Rust 100x |
| api | 0.03s | 0.47s | Rust 16x |

#### File analysis

| Command | Rust | Python | Winner |
|---------|------|--------|--------|
| outline | 0.01s | 0.15s | Rust 15x |
| imports | 0.00s | 0.09s | Rust 260x |
| changed | 0.07s | 0.62s | Rust 9x |

#### Index management

| Command | Rust | Python | Winner |
|---------|------|--------|--------|
| rebuild | 24.7s | ~36s | Rust 1.5x |
| rebuild --deps | 32.7s | N/A | Rust only |
| update | 0.89s | 1.61s | Rust 1.8x |
| stats | 0.41s | 0.20s | Python 2x |

### Size Comparison

| Metric | Rust | Python |
|--------|------|--------|
| Binary | ~4.4 MB | ~273 MB (venv) |
| DB size | 180 MB | ~100 MB |
| Symbols | 299,393 | 264,023 |
| Refs | 900,079 | 438,208 |

## Architecture

- **grep-searcher** — ripgrep internals for fast searching
- **SQLite + FTS5** — full-text search index
- **rayon** — parallel file parsing
- **ignore** — gitignore-aware directory traversal

### Database Schema

```sql
files (id, path, mtime, size)
symbols (id, file_id, name, kind, line, signature)
symbols_fts (name, signature)  -- FTS5
inheritance (child_id, parent_name, kind)
modules (id, name, path)
module_deps (module_id, dep_module_id, dep_kind)
refs (id, file_id, name, line, context)
```

## Changelog

### 3.0.0
- **Major release** — complete Rust rewrite, replacing Python version
- 26 of 33 commands faster than Python
- Top speedups: imports (260x), dependents (100x), deps/class (90x)
- Full index with 900K+ references
- Fixed `hierarchy` multiline class declarations
- Fixed `provides` Java support and suffix matching
- Removed all project-specific references

### 0.2.0
- Added references indexing (refs table)
- `usages` command uses index (~8ms vs 1.4s grep)
- Optimized grep commands with memory-mapped files
- crossbeam-channel for faster concurrency

### 0.1.0
- Initial release
- 31 commands
- FTS5 index for symbols
- Module dependency tracking
- Incremental updates

## License

MIT
