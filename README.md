# ast-index v3.8.0

Fast code search CLI for Android/Kotlin/Java, iOS/Swift/ObjC, Python, Go, C++, and Perl projects. Native Rust implementation.

## Supported Projects

| Platform | Languages | Module System |
|----------|-----------|---------------|
| Android | Kotlin, Java | Gradle |
| iOS | Swift, Objective-C | SPM (Package.swift) |
| Backend | Python, Go, C++ | None (file-based) |
| Perl | Perl | Makefile.PL, Build.PL |
| Schema | Protocol Buffers, WSDL/XSD | None |
| Mixed | All above | All |

Project type is auto-detected.

## Installation

### Homebrew (macOS/Linux)

```bash
brew tap defendend/ast-index
brew install ast-index
```

### Migration from kotlin-index

If you have the old `kotlin-index` installed:

```bash
brew uninstall kotlin-index
brew untap defendend/kotlin-index
brew tap defendend/ast-index
brew install ast-index
```

### From source

```bash
git clone https://github.com/defendend/Claude-ast-index-search.git
cd Claude-ast-index-search
cargo build --release
# Binary: target/release/ast-index (~4.4 MB)
```

## Quick Start

```bash
cd /path/to/android/project

# Build index
ast-index rebuild

# Search
ast-index search ViewModel
ast-index class BaseFragment
ast-index implementations Presenter
ast-index usages Repository
```

## Commands (46)

### Grep-based (no index required)

```bash
ast-index todo [PATTERN]           # TODO/FIXME/HACK comments
ast-index callers <FUNCTION>       # Function call sites
ast-index provides <TYPE>          # @Provides/@Binds for type
ast-index suspend [QUERY]          # Suspend functions
ast-index composables [QUERY]      # @Composable functions
ast-index deprecated [QUERY]       # @Deprecated items
ast-index suppress [QUERY]         # @Suppress annotations
ast-index inject <TYPE>            # @Inject points
ast-index annotations <ANN>        # Classes with annotation
ast-index deeplinks [QUERY]        # Deeplinks
ast-index extensions <TYPE>        # Extension functions
ast-index flows [QUERY]            # Flow/StateFlow/SharedFlow
ast-index previews [QUERY]         # @Preview functions
ast-index usages <SYMBOL>          # Symbol usages (falls back to grep)
```

### Index-based (requires rebuild)

```bash
ast-index search <QUERY>           # Universal search
ast-index file <PATTERN>           # Find files
ast-index symbol <NAME>            # Find symbols
ast-index class <NAME>             # Find classes/interfaces
ast-index implementations <PARENT> # Find implementations
ast-index hierarchy <CLASS>        # Class hierarchy tree
ast-index usages <SYMBOL>          # Symbol usages (indexed, ~8ms)
```

### Module analysis

```bash
ast-index module <PATTERN>         # Find modules
ast-index deps <MODULE>            # Module dependencies
ast-index dependents <MODULE>      # Dependent modules
ast-index unused-deps <MODULE>     # Find unused dependencies (v3.2: +transitive, XML, resources)
ast-index api <MODULE>             # Public API of module
```

### XML & Resource analysis (new in v3.2)

```bash
ast-index xml-usages <CLASS>       # Find class usages in XML layouts
ast-index resource-usages <RES>    # Find resource usages (@drawable/ic_name, R.string.x)
ast-index resource-usages --unused --module <MODULE>  # Find unused resources
```

### File analysis

```bash
ast-index outline <FILE>           # Symbols in file
ast-index imports <FILE>           # Imports in file
ast-index changed [--base BRANCH]  # Changed symbols (git diff)
```

### iOS-specific commands (new in v3.4)

```bash
ast-index storyboard-usages <CLASS>  # Class usages in storyboards/xibs
ast-index asset-usages [ASSET]       # iOS asset usages (xcassets)
ast-index asset-usages --unused --module <MODULE>  # Find unused assets
ast-index swiftui [QUERY]            # @State/@Binding/@Published props
ast-index async-funcs [QUERY]        # Swift async functions
ast-index publishers [QUERY]         # Combine publishers
ast-index main-actor [QUERY]         # @MainActor usages
```

### Perl-specific commands (new in v3.6)

```bash
ast-index perl-exports [QUERY]       # Find @EXPORT/@EXPORT_OK
ast-index perl-subs [QUERY]          # Find subroutines
ast-index perl-pod [QUERY]           # Find POD documentation (=head1, =item, etc.)
ast-index perl-tests [QUERY]         # Find Test::More assertions (ok, is, like, etc.)
ast-index perl-imports [QUERY]       # Find use/require statements
```

### Python support (new in v3.8)

```bash
# Index Python files (.py)
ast-index rebuild                    # Auto-detects Python files

# Indexed symbols:
# - class ClassName
# - def function_name / async def function_name
# - @decorator
# - import module / from module import name

# Commands work with Python:
ast-index class "ClassName"          # Find Python classes
ast-index symbol "function"          # Find functions
ast-index outline "file.py"          # Show file structure
ast-index imports "file.py"          # Show imports (including from X import Y)
ast-index usages "ClassName"         # Find usages
```

### Go support (new in v3.8)

```bash
# Index Go files (.go)
ast-index rebuild                    # Auto-detects Go files

# Indexed symbols:
# - package name
# - type Name struct / type Name interface
# - func Name() / func (r *T) Method()
# - import "module"

# Commands work with Go:
ast-index class "StructName"         # Find structs/interfaces
ast-index symbol "FuncName"          # Find functions
ast-index outline "file.go"          # Show file structure
ast-index imports "file.go"          # Show imports (including import blocks)
ast-index usages "TypeName"          # Find usages
```

### Index management

```bash
ast-index init                     # Initialize DB
ast-index rebuild [--type TYPE]    # Full reindex
ast-index update                   # Incremental update
ast-index stats                    # Index statistics
ast-index version                  # Version info
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

-- New in v3.2.0:
xml_usages (id, module_id, file_path, line, class_name, usage_type, element_id)
resources (id, module_id, type, name, file_path, line)
resource_usages (id, resource_id, usage_file, usage_line, usage_type)
transitive_deps (id, module_id, dependency_id, depth, path)

-- New in v3.4.0 (iOS):
storyboard_usages (id, module_id, file_path, line, class_name, usage_type, storyboard_id)
ios_assets (id, module_id, type, name, file_path)
ios_asset_usages (id, asset_id, usage_file, usage_line, usage_type)
```

## Changelog

### 3.8.0
- **Python support** — index and search Python codebases
  - Index: `class`, `def`, `async def`, decorators
  - Imports: `import module`, `from module import name`
  - File types: `.py`
  - `outline` and `imports` commands work with Python files
- **Go support** — index and search Go codebases
  - Index: `package`, `type struct`, `type interface`, `func`, methods with receivers
  - Imports: single imports and import blocks
  - File types: `.go`
  - `outline` and `imports` commands work with Go files
- **Performance** — `deeplinks` command 200x faster (optimized pattern)

### 3.7.0
- **call-tree command** — show complete call hierarchy going UP (who calls the callers)
  - `ast-index call-tree "functionName" --depth 3 --limit 10`
  - Works across Kotlin, Java, Swift, Objective-C, and Perl
- **--no-ignore flag** — index gitignored directories like `build/`
  - `ast-index rebuild --no-ignore`
  - Useful for finding generated code like `BuildConfig.java`

### 3.6.0
- **Perl support** — index and search Perl codebases
  - Index: `package`, `sub`, `use constant`, `our` variables
  - Inheritance: `use base`, `use parent`, `@ISA`
  - File types: `.pm`, `.pl`, `.t`, `.pod`
  - New commands: `perl-exports`, `perl-subs`, `perl-pod`, `perl-tests`, `perl-imports`
  - Grep commands now search Perl files: `todo`, `callers`, `deprecated`, `annotations`
  - `imports` command now parses Perl `use`/`require` statements
  - Perl packages indexed as modules for `module` command
  - Project detection: `Makefile.PL`, `Build.PL`, `cpanfile`

### 3.5.0
- **Renamed to ast-index** — project renamed from `kotlin-index` to `ast-index`
  - New CLI command: `ast-index` (was `kotlin-index`)
  - New Homebrew tap: `defendend/ast-index` (was `defendend/kotlin-index`)
  - New repo: `Claude-ast-index-search` (was `Claude-index-search-android-studio`)

### 3.4.1
- **Fix grep-based commands for iOS** — 6 commands now work with Swift/ObjC:
  - `todo` — search in .swift/.m/.h files
  - `callers` — support Swift function call patterns
  - `deprecated` — support `@available(*, deprecated)` syntax
  - `annotations` — search in Swift/ObjC files (@objc, @IBAction, etc.)
  - `deeplinks` — add iOS patterns (openURL, CFBundleURLSchemes, NSUserActivity)
  - `extensions` — support Swift `extension Type` syntax

### 3.4.0
- **iOS storyboard/xib analysis** — `storyboard-usages` command to find class usages in storyboards and xibs
- **iOS assets support** — index and search xcassets (images, colors), `asset-usages` command with `--unused` flag
- **SwiftUI commands** — `swiftui` command to find @State, @Binding, @Published, @ObservedObject properties
- **Swift concurrency** — `async-funcs` for async functions, `main-actor` for @MainActor usages
- **Combine support** — `publishers` command to find PassthroughSubject, CurrentValueSubject, AnyPublisher
- **CocoaPods/Carthage** — detect and index dependencies from Podfile and Cartfile

### 3.3.0
- **iOS/Swift/ObjC support** — auto-detect project type and index Swift/ObjC files
- Swift: class, struct, enum, protocol, actor, extension, func, init, var/let, typealias
- ObjC: @interface, @protocol, @implementation, methods, @property, typedef, categories
- SPM module detection from Package.swift (.target, .testTarget, .binaryTarget)
- Inheritance and protocol conformance tracking for Swift/ObjC

### 3.2.0
- Add `xml-usages` command — find class usages in XML layouts
- Add `resource-usages` command — find resource usages (drawable, string, color, etc.)
- Add `resource-usages --unused` — find unused resources in a module
- Update `unused-deps` with transitive dependency checking (via api deps)
- Update `unused-deps` with XML layout usage checking
- Update `unused-deps` with resource usage checking
- New flags: `--no-transitive`, `--no-xml`, `--no-resources`, `--strict`
- Index XML layouts (5K+ usages in YandexGo project)
- Index resources (63K+ resources, 15K+ usages)
- Build transitive dependency cache (11K+ entries)

### 3.1.0
- Add `unused-deps` command — find unused module dependencies
- Module dependencies now indexed by default (use `--no-deps` to skip)

### 3.0.0 (Rust)
- **Major release** — complete Rust rewrite, replacing Python version
- 26 of 33 commands faster than Python
- Top speedups: imports (260x), dependents (100x), deps/class (90x)
- Full index with 900K+ references
- Fixed `hierarchy` multiline class declarations
- Fixed `provides` Java support and suffix matching

### Python versions (1.0.0 - 2.5.2)

> Legacy Python code archived in `legacy-python-mcp/` folder

#### 2.5.2
- Project-specific databases: Each project now has its own index database

#### 2.5.1
- Use ripgrep for 10-15x faster grep-based searches

#### 2.5.0
- Add `composables`, `previews`, `suspend`, `flows` commands

#### 2.4.1
- Fix `callers`, `outline`, `api` commands

#### 2.4.0
- Add `todo`, `deprecated`, `suppress`, `extensions`, `api`, `deeplinks` commands

#### 2.3.0
- Add `callers`, `imports`, `provides`, `inject` commands

#### 2.2.0
- Add `hierarchy`, `annotations`, `changed` commands

#### 2.1.0
- Fix `class` command, add `update` command

#### 2.0.0
- pip package, CLI with typer + rich, Skill for Claude Code, MCP server

#### 1.2.0
- Java support (tree-sitter-java), Find Usages, Find Implementations

#### 1.1.0
- Incremental indexing, better module detection

#### 1.0.0
- Initial release: File/symbol/module search, MCP server

## IDE Integration

### Cursor

Add to `.cursor/rules` or project's `CLAUDE.md`:

```markdown
## Code Search

Use `ast-index` CLI for fast code search:

\`\`\`bash
# Search class/interface/protocol
ast-index class "ClassName"

# Find implementations
ast-index implementations "BaseClass"

# Find usages
ast-index usages "SymbolName"

# Module dependencies
ast-index deps "module.name"
\`\`\`

Run `ast-index rebuild` in project root before first use.
```

### Claude Code Plugin

#### Install Plugin

```bash
# Add marketplace (once)
claude plugin marketplace add defendend/Claude-ast-index-search

# Install plugin
claude plugin install ast-index

# Restart Claude Code
```

#### Update Plugin

```bash
# Update CLI
brew upgrade ast-index

# Update plugin
claude plugin update ast-index
```

#### Uninstall Plugin

```bash
claude plugin uninstall ast-index
```

## License

MIT
