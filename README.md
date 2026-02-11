# ast-index v3.13.1

Fast code search CLI for 15 programming languages. Native Rust implementation.

## Supported Projects

| Platform | Languages | File Extensions |
|----------|-----------|-----------------|
| Android | Kotlin, Java | `.kt`, `.java` |
| iOS | Swift, Objective-C | `.swift`, `.m`, `.h` |
| Web/Frontend | TypeScript, JavaScript | `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs`, `.vue`, `.svelte` |
| Systems | Rust | `.rs` |
| Backend | C#, Python, Go, C++, Scala | `.cs`, `.py`, `.go`, `.cpp`, `.cc`, `.c`, `.hpp`, `.scala`, `.sc` |
| Scripting | Ruby, Perl | `.rb`, `.pm`, `.pl`, `.t` |
| Mobile | Dart/Flutter | `.dart` |
| Schema | Protocol Buffers, WSDL/XSD | `.proto`, `.wsdl`, `.xsd` |

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

### Troubleshooting: Syntax errors on install

If `brew install ast-index` fails with merge conflict errors (`<<<<<<< HEAD`), reset your local tap:

```bash
cd /opt/homebrew/Library/Taps/defendend/homebrew-ast-index
git fetch origin
git reset --hard origin/main
brew install ast-index
```

## Quick Start

```bash
cd /path/to/project

# Build index
ast-index rebuild

# Search
ast-index search ViewModel
ast-index class BaseFragment
ast-index implementations Presenter
ast-index usages Repository
```

## Commands (46+)

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
ast-index symbol <NAME>            # Find any symbol by name
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

### XML & Resource analysis

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

### iOS-specific commands

```bash
ast-index storyboard-usages <CLASS>  # Class usages in storyboards/xibs
ast-index asset-usages [ASSET]       # iOS asset usages (xcassets)
ast-index asset-usages --unused --module <MODULE>  # Find unused assets
ast-index swiftui [QUERY]            # @State/@Binding/@Published props
ast-index async-funcs [QUERY]        # Swift async functions
ast-index publishers [QUERY]         # Combine publishers
ast-index main-actor [QUERY]         # @MainActor usages
```

### Perl-specific commands

```bash
ast-index perl-exports [QUERY]       # Find @EXPORT/@EXPORT_OK
ast-index perl-subs [QUERY]          # Find subroutines
ast-index perl-pod [QUERY]           # Find POD documentation (=head1, =item, etc.)
ast-index perl-tests [QUERY]         # Find Test::More assertions (ok, is, like, etc.)
ast-index perl-imports [QUERY]       # Find use/require statements
```

### Index management

```bash
ast-index init                     # Initialize DB
ast-index rebuild [--type TYPE]    # Full reindex
ast-index update                   # Incremental update
ast-index stats                    # Index statistics
ast-index version                  # Version info
```

## Language-Specific Features

### TypeScript/JavaScript (new in v3.9)

Supported elements:
- Classes, interfaces, type aliases, enums
- Functions (regular, arrow, async)
- React components and hooks (`useXxx`)
- Vue SFC (`<script>` extraction)
- Svelte components
- Decorators (@Controller, @Injectable, etc.)
- Namespaces, constants, imports/exports

```bash
ast-index class "Component"        # Find React/Vue components
ast-index search "use"             # Find React hooks
ast-index search "@Controller"     # Find NestJS controllers
ast-index class "Props"            # Find prop interfaces
```

### Rust (new in v3.9)

Supported elements:
- Structs, enums, traits
- Impl blocks (`impl Trait for Type`)
- Functions, macros (`macro_rules!`)
- Type aliases, constants, statics
- Modules, use statements
- Derive attributes

```bash
ast-index class "Service"          # Find structs
ast-index class "Repository"       # Find traits
ast-index search "impl"            # Find impl blocks
ast-index search "macro_rules"     # Find macros
```

### Ruby (new in v3.9)

Supported elements:
- Classes, modules
- Methods (def, def self.)
- RSpec DSL (describe, it, let)
- Rails patterns (has_many, validates, scope, callbacks)
- Require statements, include/extend

```bash
ast-index class "Controller"       # Find controllers
ast-index search "has_many"        # Find associations
ast-index search "describe"        # Find RSpec tests
ast-index search "scope"           # Find scopes
```

### C# (new in v3.9)

Supported elements:
- Classes, interfaces, structs, records
- Enums, delegates, events
- Methods, properties, fields
- ASP.NET attributes (@ApiController, @HttpGet, etc.)
- Unity attributes (@SerializeField)
- Namespaces, using statements

```bash
ast-index class "Controller"       # Find ASP.NET controllers
ast-index class "IRepository"      # Find interfaces
ast-index search "[HttpGet]"       # Find API endpoints
ast-index search "MonoBehaviour"   # Find Unity scripts
```

### Dart/Flutter (new in v3.10)

Supported elements:
- Classes with Dart 3 modifiers (abstract, sealed, final, base, interface, mixin class)
- Mixins, extensions, extension types
- Enhanced enums with implements/with
- Functions, constructors, factory constructors
- Getters/setters, typedefs, properties
- Imports/exports

```bash
ast-index class "Widget"           # Find widget classes
ast-index class "Provider"         # Find providers
ast-index search "mixin"           # Find mixins
ast-index implementations "State"  # Find State implementations
ast-index outline "main.dart"      # Show file structure
ast-index imports "app.dart"       # Show imports
```

### Python

```bash
ast-index class "ClassName"        # Find Python classes
ast-index symbol "function"        # Find functions
ast-index outline "file.py"        # Show file structure
ast-index imports "file.py"        # Show imports
```

### Go

```bash
ast-index class "StructName"       # Find structs/interfaces
ast-index symbol "FuncName"        # Find functions
ast-index outline "file.go"        # Show file structure
ast-index imports "file.go"        # Show imports
```

## Performance

Benchmarks on large Android project (~29k files, ~300k symbols):

| Command | Rust | grep | Speedup |
|---------|------|------|---------|
| imports | 0.3ms | 90ms | **260x** |
| dependents | 2ms | 100ms | **100x** |
| deps | 3ms | 90ms | **90x** |
| class | 1ms | 90ms | **90x** |
| search | 11ms | 280ms | **14x** |
| usages | 8ms | 90ms | **12x** |

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
xml_usages (id, module_id, file_path, line, class_name, usage_type, element_id)
resources (id, module_id, type, name, file_path, line)
resource_usages (id, resource_id, usage_file, usage_line, usage_type)
transitive_deps (id, module_id, dependency_id, depth, path)
storyboard_usages (id, module_id, file_path, line, class_name, usage_type, storyboard_id)
ios_assets (id, module_id, type, name, file_path)
ios_asset_usages (id, asset_id, usage_file, usage_line, usage_type)
```

## Changelog

### 3.13.1
- **Fix plugin skill discovery** — added `"skills"` field to `plugin.json`, fixing "Unknown skill: ast-index" error when invoking `/ast-index`

### 3.13.0
- **Scala language support** — tree-sitter parser for class, case class, object, trait, enum (Scala 3), def, val/var, type alias, given
- **Bazel project detection** — `WORKSPACE`, `WORKSPACE.bazel`, `MODULE.bazel` as project root markers
- **4x faster rebuild on non-Android/iOS projects** — skip XML layouts, storyboards, iOS assets, CocoaPods phases when no platform markers present (309s → 83s on 83k files)
- **Git default branch detection** — correctly parse `origin/trunk`, `origin/develop` from symbolic-ref, not just main/master

### 3.12.0
- **Tree-sitter AST parsing for 12 languages** — replaced regex-based parsers with tree-sitter for Kotlin, Java, Swift, ObjC, Python, Go, Rust, Ruby, C#, C++, Dart, Proto, and TypeScript. Parsing is now based on real ASTs instead of regex heuristics — more accurate symbol extraction, correct handling of nested constructs, and fewer false positives
- **Grouped `--help` output** — commands organized into 8 logical categories (Index Management, Search & Navigation, Module Commands, Code Patterns, Android, iOS, Perl, Project Configuration) instead of a flat alphabetical list
- **Updated project description** — "Fast code search for multi-language projects"

### 3.11.2
- **Fix `watch` command on large projects** — switched from kqueue to FSEvents (macOS) / inotify (Linux), fixes "Too many open files" error

### 3.11.1
- **Fix `changed` command** — auto-detect default git branch (`origin/main` or `origin/master`)
- **Fix `api` command** — accept module names with dots (e.g. `module.name` → `module/name`)
- **Updated skill docs** — added `--format json`, `unused-symbols`, `watch`, multi-root commands

### 3.11.0
- **10x faster `unused-deps`** — replaced filesystem scanning (WalkDir + read_to_string) with index-based SQL queries to `refs` table. `core` module (225 deps) now completes in ~6s instead of 60s+ timeout
- **Fixed transitive dependency logic** — correctly checks `transitive_deps` table (api chain reachability) instead of re-scanning symbols
- **Multi-VCS support for `changed`** — auto-detects VCS, auto-selects base branch (`trunk` for arc, `origin/main` for git), normalizes `origin/` prefix
- **Removed skill copying from initialize commands** — `/initialize-*` no longer copies skill files to project directory

### 3.10.4
- **2.6x faster indexing on large projects** — fix Dart parser allocating lines vector per class declaration

### 3.10.2
- **Fix `changed` command** — use `merge-base` instead of direct diff to show only current branch changes
- **Multi-VCS support** — auto-detect arc vs git, use correct VCS commands

### 3.10.1
- **Fix indexing hangs on large monorepos** — disable symlink following, add max depth limit
- **Expanded excluded directories** — added `bazel-out`, `bazel-bin`, `buck-out`, `out`, `.metals`, `.dart_tool` and more
- **Better progress reporting** — output after every chunk instead of every 4th
- **GitHub Actions release workflow** — automated builds for darwin-arm64, darwin-x86_64, linux-x86_64, windows-x86_64

### 3.10.0
- **Dart/Flutter support** — index and search Dart/Flutter codebases
  - Classes with Dart 3 modifiers: `abstract`, `sealed`, `final`, `base`, `interface`, `mixin class`
  - Mixins: `mixin Foo on Bar`
  - Extensions and extension types (Dart 3.3)
  - Enhanced enums with `with`/`implements`
  - Functions, constructors, factory constructors
  - Getters/setters, typedefs, properties
  - Imports/exports
  - Multiline class declarations
  - File types: `.dart`
- **20 new tests** — comprehensive test coverage for Dart parser

### 3.9.3
- **Simplified plugin installation** — `install-claude-plugin` now calls `claude plugin marketplace add` and `claude plugin install` instead of manual file copying
- **Updated README** — plugin install instructions now use official `claude plugin` CLI commands

### 3.9.2
- **Fix OOM crashes on large projects** (70K+ files)
  - Batched indexing: parse and write to DB in chunks of 500 files instead of loading everything into memory
  - Limited rayon thread pool to max 8 threads to cap peak memory
  - Skip files > 1 MB (minified/generated code)
  - Skip lines > 2000 chars in ref parser
  - Truncate ref context to 500 chars (was unbounded — minified JS lines caused 12 GB+ databases)
  - Reduced SQLite cache from 64 MB to 8 MB
- **Hardcoded directory exclusions** — always skip `node_modules`, `__pycache__`, `build`, `dist`, `target`, `vendor`, `.gradle`, `Pods`, `DerivedData`, `.next`, `.nuxt`, `.venv`, `.cache` etc. regardless of `.gitignore`
- **New project type detection** — Frontend (`package.json`), Python (`pyproject.toml`), Go (`go.mod`), Rust (`Cargo.toml`)
- **LazyLock regex** — all 146 regex compilations cached via `std::sync::LazyLock` (was recompiling per file)

### 3.9.1
- **Performance fix** — grep-based commands now use early termination
  - Commands like `deeplinks`, `todo`, `callers` etc. stop scanning after finding `limit` results
  - Up to 100-1000x faster on large codebases (29k files: 4-35s → 10-50ms)

### 3.9.0
- **TypeScript/JavaScript support** — index and search web projects
  - React: components, hooks (useXxx), JSX/TSX
  - Vue: SFC script extraction, defineComponent
  - Svelte: component props extraction
  - NestJS/Angular: decorators (@Controller, @Injectable, @Component)
  - Node.js: ES modules, CommonJS
  - File types: `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs`, `.vue`, `.svelte`
- **Rust support** — index and search Rust codebases
  - Structs, enums, traits, impl blocks
  - Functions, macros, type aliases
  - Derive attributes tracking
  - File types: `.rs`
- **Ruby support** — index and search Ruby/Rails codebases
  - Classes, modules, methods
  - RSpec DSL (describe, it, let, context)
  - Rails: associations, validations, scopes, callbacks
  - File types: `.rb`
- **C# support** — index and search .NET projects
  - Classes, interfaces, structs, records
  - ASP.NET: controllers, HTTP attributes
  - Unity: MonoBehaviour, SerializeField
  - File types: `.cs`
- **Explore agent** — deep code investigation with confirmations
- **Review agent** — change analysis with impact assessment
- **63 tests** — comprehensive test coverage for all parsers

### 3.8.5
- **Documentation** — added troubleshooting section for brew install merge conflict errors

### 3.8.2
- **Plugin improvements**
  - Added C++, Protocol Buffers, and WSDL/XSD reference documentation
  - Added "Critical Rules" section to SKILL.md for better Claude integration
  - Initialize commands now copy skill documentation to project `.claude/` directory
  - Updated plugin description to include all supported languages

### 3.8.1
- **search command fix** — `-l/--limit` parameter now correctly limits file results
- **Content search** — `search` command now also searches file contents (not just filenames and symbols)

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
- **Renamed to ast-index** — project renamed from `kotlin-index`
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
- Index XML layouts (5K+ usages in large Android projects)
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

From terminal:
```bash
# Add marketplace (once)
claude plugin marketplace add defendend/Claude-ast-index-search

# Install plugin
claude plugin install ast-index
```

Or if ast-index binary is already installed (via brew or ya tool):
```bash
ast-index install-claude-plugin
```

Restart Claude Code to activate the plugin.

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
