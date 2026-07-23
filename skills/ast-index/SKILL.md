---
name: ast-index
description: This skill should be used when the user asks to "find a class", "search for symbol", "find usages", "find implementations", "search codebase", "find file", "class hierarchy", "find callers", "module dependencies", "unused dependencies", "project map", "project conventions", "project structure", "what frameworks", "what architecture", "find Perl subs", "Perl exports", "find Python class", "Go struct", "Go interface", "find React component", "find TypeScript interface", "find Rust struct", "find Ruby class", "find C# controller", "find Dart class", "find Flutter widget", "find mixin", "find Scala trait", "find case class", "find object", "find PHP class", "find Laravel model", "find PHP trait", or needs fast code search in Android/Kotlin/Java, iOS/Swift/ObjC, Dart/Flutter, TypeScript/JavaScript, Rust, Ruby, C#, Scala, PHP, Perl, Python, Go, C++, or Protocol Buffers projects. Also triggered by mentions of "ast-index" CLI tool.
user-invocable: false
---

# ast-index - Code Search for Multi-Platform Projects

Fast native Rust CLI for structural code search in Android/Kotlin/Java, iOS/Swift/ObjC, Dart/Flutter, TypeScript/JavaScript, Rust, Ruby, C#, Scala, PHP, Perl, Python, Go, C++, and Proto projects using SQLite + FTS5 index.

## Critical Rules

**ALWAYS use ast-index FIRST for any code search task.** These rules are mandatory:

1. **ast-index is the PRIMARY search tool** — use it before grep, ripgrep, or Search tool
2. **DO NOT duplicate results** — if ast-index found usages/implementations, that IS the complete answer
3. **DO NOT run grep "for completeness"** after ast-index returns results
4. **Use grep/Search ONLY when:**
   - ast-index returns empty results
   - Searching for regex patterns (ast-index uses literal match)
   - Searching for string literals inside code (`"some text"`)
   - Searching in comments content

**Why:** ast-index is 17-69x faster than grep (1-10ms vs 200ms-3s) and returns structured, accurate results.

## Prerequisites

Install the CLI before use:

```bash
brew tap defendend/ast-index
brew install ast-index
```

Initialize index in project root:

```bash
cd /path/to/project
ast-index rebuild
```

The index is stored at `~/Library/Caches/ast-index/<project-hash>/index.db` (macOS) or `~/.cache/ast-index/<project-hash>/index.db` (Linux). Rebuild deletes the DB file entirely and creates a fresh index.

## Supported Projects

| Platform | Languages | Module System |
|----------|-----------|---------------|
| Android/Java | Kotlin, Java | Gradle (build.gradle.kts), Maven (pom.xml) |
| iOS | Swift, Objective-C | SPM (Package.swift) |
| Web | TypeScript, JavaScript, React, Vue, Svelte | package.json |
| Rust | Rust | Cargo.toml |
| Ruby | Ruby, Rails, RSpec | Gemfile |
| .NET | C#, ASP.NET, Unity | *.csproj |
| Dart/Flutter | Dart | pubspec.yaml |
| Scala | Scala | Bazel (WORKSPACE, BUILD) |
| PHP | PHP | composer.json |
| Perl | Perl | Makefile.PL, Build.PL |
| Python | Python | None (*.py files) |
| Go | Go | None (*.go files) |
| Proto | Protocol Buffers (proto2/proto3) | None (*.proto files) |
| WSDL | WSDL, XSD | None (*.wsdl, *.xsd files) |
| C/C++ | C, C++ (JNI, uservices) | None (*.cpp, *.h, *.hpp files) |
| Godot | GDScript | project.godot |
| Mixed | All above | All |

Project type is auto-detected by marker files (build.gradle.kts, Package.swift, Makefile.PL, etc.). Python, Go, Proto, WSDL, and C++ files are indexed alongside main project type.

## Core Commands

### Explore (one-shot context)

**`explore`** - Rank the symbols most relevant to a query, print their source
(read fresh from disk), list graph neighbours (callers/subclasses), and locate
tests by path convention — in a single call. Prefer this over a search + read
loop when you want to understand an area. Language-agnostic and vendor-aware
(`node_modules` `.d.ts` and cross-stack matches are down-ranked, never deleted).

```bash
ast-index explore applicant merge MergeService   # bag of symbol/file names
ast-index explore "how does auth session work"   # natural-language question
ast-index explore PaymentService --rwr           # --rwr: re-rank via call/inheritance graph
ast-index explore Repository --max-files 8        # cap source files shown (default 6)
ast-index explore Session request --rwr --format json
```

- Default (Stage A) ranks by lexical match + multi-term corroboration.
- `--rwr` (Stage B) builds a call/inheritance graph in memory and re-ranks by
  personalized PageRank, surfacing callers/subclasses in a "Graph neighbours"
  section. Slightly slower; best when you care about who-calls-what.

### Universal Search

**`search`** - Perform universal search across files, symbols, and modules simultaneously.

```bash
ast-index search "Payment"           # Finds files, classes, functions matching "Payment"
ast-index search "ViewModel"         # Returns files, symbols, modules in ranked order
ast-index search "Store" --fuzzy     # Fuzzy: exact → prefix → contains matching
ast-index search "Handler" --module "core/"  # Search within a module
ast-index search "UserService"       # Find Java/Spring services
ast-index search "@RestController"   # Find Spring REST controllers (annotation search)
ast-index search "@GetMapping"       # Find GET endpoint mappings
```

### File Search

**`file`** - Find files by name pattern.

```bash
ast-index file "Fragment.kt"         # Find files ending with Fragment.kt
ast-index file "ViewController"      # Find iOS view controllers
```

### Symbol Search

**`symbol`** - Find symbols (classes, interfaces, functions, properties) by name.

```bash
ast-index symbol "PaymentInteractor" # Find exact symbol
ast-index symbol "Presenter"         # Find all presenters
ast-index symbol "Store" --fuzzy     # Fuzzy: exact → prefix → contains matching
ast-index symbol "Mapper" --in-file "payments/" --limit 10  # Scoped search
ast-index symbol "@Service"          # Find all @Service annotations
```

### Class Search

**`class`** - Find class, interface, or protocol definitions.

```bash
ast-index class "BaseFragment"       # Find Android base fragment
ast-index class "UIViewController"   # Find iOS view controller subclass
ast-index class "Store" --fuzzy      # Find all classes containing "Store"
ast-index class "Repository" --module "features/payments"  # Filter by module
ast-index class "UserController"     # Find Java/Spring controller class
```

### Usage Search

**`usages`** - Find all places where a symbol is used. Critical for refactoring.

```bash
ast-index usages "PaymentRepository" # Find all usages of repository
ast-index usages "onClick"           # Find all click handler usages
ast-index usages "fetchData" --in-file "src/api/"  # Scoped to file path
ast-index usages "Repository" --module "features/auth" --limit 100
```

Performance: ~8ms for indexed symbols.

### Cross-References

**`refs`** - Show cross-references for a symbol: definitions, imports, and usages in one view.

```bash
ast-index refs "PaymentRepository"   # Definitions + imports + usages
ast-index refs "BaseFragment" --limit 10  # Limit results per section
```

### Implementation Search

**`implementations`** - Find all classes that extend or implement a given class/interface/protocol. Supports partial name matching with relevance ranking (exact → suffix → contains).

```bash
ast-index implementations "BasePresenter"  # Find all presenter implementations
ast-index implementations "Repository"     # Find repository implementations (exact match)
ast-index implementations "Service"        # Partial: finds UserService, PaymentService impls too
ast-index implementations "ViewModel" --module "features/"  # Scoped to module
```

### Class Hierarchy

**`hierarchy`** - Display complete class hierarchy tree.

```bash
ast-index hierarchy "BaseFragment"   # Show fragment inheritance tree
```

### Caller Search

**`callers`** - Find all places that call a specific function.

```bash
ast-index callers "onClick"          # Find all onClick calls
ast-index callers "fetchUser"        # Find API call sites
```

### Call Tree

**`call-tree`** - Show complete call hierarchy going UP (who calls the callers). Supports Kotlin, Java, Swift, Perl, ObjC.

```bash
ast-index call-tree "processPayment" --depth 3 --limit 10
ast-index call-tree "getUsers"       # Java: finds callers of getUsers() method
```

### File Analysis

**`imports`** - List all imports in a specific file.

```bash
ast-index imports "path/to/File.kt"  # Show Kotlin file imports
```

**`outline`** - Show all symbols defined in a file. Uses tree-sitter for accurate parsing of all supported languages.

```bash
ast-index outline "PaymentFragment.kt"    # Show Kotlin fragment structure
ast-index outline "UserController.java"   # Show Java controller methods
ast-index outline "App.tsx"               # Show TypeScript/React components and functions
ast-index outline "handler.rs"            # Show Rust structs, impls, functions
```

### Code Quality

**`todo`** - Find TODO/FIXME/HACK comments in code.

```bash
ast-index todo                           # Find all TODO comments
ast-index todo --limit 10                # Limit results
```

**`deprecated`** - Find @Deprecated annotations.

```bash
ast-index deprecated                     # Find deprecated items
```

**`unused-symbols`** - Find potentially unused exported symbols.

```bash
ast-index unused-symbols --module path/to/module   # In specific module
ast-index unused-symbols --export-only             # Only exported (public) symbols
```

### Git/Arc Integration

**`changed`** - Show symbols changed in git/arc diff. Auto-detects VCS and base branch.

```bash
ast-index changed                        # Auto: trunk (arc) or origin/main (git)
ast-index changed --base main            # Explicit base branch
ast-index changed --base trunk           # For arc projects
```

### Public API

**`api`** - Show public API of a module. Accepts module path or module name (dots converted to slashes).

```bash
ast-index api "path/to/module"           # By path
ast-index api "module.name"              # By module name (dots → slashes)
```

## Project Insights

### Project Map

**`map`** - Show compact project overview: top directories by size with symbol kind counts. Use `--module` to drill down into a specific area with full class/inheritance details.

```bash
ast-index map                                # Summary: top 50 dirs with kind counts (~54 lines)
ast-index map --limit 20                     # Show only top 20 directories
ast-index map --module features/payments     # Detailed: classes with inheritance for a module
ast-index map --module src/core --per-dir 10 # More symbols per directory in detailed mode
ast-index map --format json                  # JSON output
```

Summary mode output (default, no `--module`):
```
Project: Android (Kotlin/Java) | 29144 files | 859 modules | top 50 of 728 dirs

  features/taxi_order/impl/          1626 files | 371 iface, 94 obj, 1834 cls
  features/masstransit/impl/          862 files | 165 obj, 1261 cls, 280 iface
```

Detailed mode output (with `--module`):
```
features/payments/impl/ (250 files)
  PaymentInteractor : class > BaseInteractor
  PaymentRepository : interface
  PaymentMapper : class
```

### Project Conventions

**`conventions`** - Auto-detect architecture patterns, frameworks, and naming conventions from the indexed codebase. Runs read-only SQL queries — no file scanning needed.

```bash
ast-index conventions                        # Text output (~30 lines)
ast-index conventions --format json          # JSON output
```

Detects:
- **Architecture**: Clean Architecture, Feature-sliced, BLoC, MVC, MVVM, MVP, Redux, Composition API, Hooks
- **Frameworks**: DI (Hilt, Dagger, Koin), Async (Coroutines, RxJava, Combine), Network (Retrofit, OkHttp), DB (Room, Realm), UI (Compose, SwiftUI, React, Flutter), Testing (JUnit, Kotest, XCTest, pytest, Jest)
- **Naming patterns**: ViewModel, Repository, UseCase, Service, Controller, Fragment, etc. (with counts)

## Common Flags

Most search commands (`search`, `symbol`, `class`, `usages`, `implementations`) support:

| Flag | Description |
|------|-------------|
| `--fuzzy` | Fuzzy search: exact match → prefix match → contains match |
| `--in-file <PATH>` | Filter results by file path (substring match) |
| `--module <PATH>` | Filter results by module path (substring match) |
| `--limit <N>` | Max results to return |
| `--format json` | JSON output for structured processing |

**When to use `--fuzzy`:** When you don't know the exact name. `ast-index class "Store" --fuzzy` finds `Store`, `StoreImpl`, `AppStore`, `DataStoreRepository`, etc. The `--fuzzy` flag performs three-stage matching: exact match first, then prefix match, then contains match.

## Index Management

```bash
ast-index rebuild                        # Full rebuild with dependencies
ast-index rebuild --no-deps              # Skip module dependency indexing
ast-index rebuild --no-ignore            # Include gitignored files (build/, etc.)
ast-index rebuild --sub-projects         # Index each sub-project separately (large monorepos)
ast-index rebuild -j 32                  # Use 32 threads (for network/FUSE filesystems)
ast-index rebuild -v                     # Verbose logging with timing per step
ast-index update                         # Incremental update (changed files only)
ast-index stats                          # Show index statistics
ast-index clear                          # Delete index for current project
ast-index restore /path/to/index.db      # Restore index from a .db file
ast-index watch                          # Watch for file changes and auto-update index
```

### Directory-Scoped Search

When running search from a subdirectory, results are automatically limited to that subtree (the index DB in cache is used to detect the project root):

```bash
cd /project/root
ast-index rebuild                        # Index entire project, creates .ast-index-root
ast-index search "Payment"              # Finds results across entire project

cd /project/root/services/payments
ast-index search "Payment"              # Only finds results within services/payments/
ast-index class "PaymentService"        # Only classes in services/payments/ subtree
```

This works with all search commands: `search`, `symbol`, `class`, `implementations`, `usages`.

## Multi-Root Projects

Add additional source roots for monorepos or multi-project setups. Extra roots are indexed alongside the primary project and their module dependencies are included.

```bash
ast-index add-root /path/to/other/source    # Add source root (warns on overlap)
ast-index add-root ./subdir --force         # Force add even if inside project root
ast-index remove-root /path/to/other/source # Remove source root
ast-index list-roots                        # List configured roots
```

## Utility Commands

```bash
ast-index version                    # Show CLI version
ast-index help                       # Show help message
ast-index help <command>             # Show help for specific command
ast-index install-claude-plugin      # Install Claude Code plugin to ~/.claude/plugins/
```

## Programmatic Access (SQL & SDK)

For complex analysis that requires combining multiple queries, filtering, or aggregation — use direct SQL access instead of chaining multiple commands.

### Structural Search (ast-grep)

Structural code search using AST patterns. Requires `sg` (ast-grep) installed.

```bash
# Simple pattern matching
ast-index agrep "router.launch($$$)" --lang kotlin
ast-index agrep "@Inject constructor($$$)" --lang kotlin
ast-index agrep "suspend fun $NAME($$$)" --lang kotlin

# Find classes extending a specific base
ast-index agrep "class $NAME : BasePresenter($$$)" --lang kotlin

# Find async functions (Swift)
ast-index agrep "func $NAME($$$) async" --lang swift

# Find React components
ast-index agrep "export default function $NAME($$$)" --lang typescript

# JSON output for programmatic use
ast-index agrep "@Composable fun $NAME($$$)" --lang kotlin --json
```

**Pattern syntax** (ast-grep):
- `$NAME` — matches a single AST node (captures as metavariable)
- `$$$` — matches zero or more AST nodes (variadic)
- Everything else is literal pattern matching against the AST

**When to use `agrep` vs built-in commands:**
- `composables`, `inject`, `suspend` → use built-in (faster, pre-configured)
- Custom structural patterns, negative matching, context queries → use `agrep`

### Raw SQL Query

```bash
# Execute any SELECT query against the index database (JSON output)
ast-index query "SELECT s.name, s.kind, f.path FROM symbols s JOIN files f ON s.file_id = f.id WHERE s.kind = 'class'"

# With limit
ast-index query "SELECT * FROM symbols WHERE name LIKE '%Service%'" --limit 50

# Complex joins — find classes that implement an interface but have no usages
ast-index query "SELECT s.name, f.path FROM symbols s JOIN files f ON s.file_id = f.id JOIN inheritance i ON s.id = i.child_id WHERE i.parent_name = 'Repository' AND s.name NOT IN (SELECT name FROM refs)"

# Find files with most symbols (complexity hotspots)
ast-index query "SELECT f.path, COUNT(*) as sym_count FROM symbols s JOIN files f ON s.file_id = f.id GROUP BY f.id ORDER BY sym_count DESC LIMIT 20"

# Find unused classes (defined but never referenced)
ast-index query "SELECT s.name, s.kind, f.path FROM symbols s JOIN files f ON s.file_id = f.id WHERE s.kind IN ('class', 'interface') AND s.name NOT IN (SELECT name FROM refs) AND s.name NOT IN (SELECT parent_name FROM inheritance)"

# Module dependency analysis — find modules with most dependencies
ast-index query "SELECT m.name, COUNT(*) as dep_count FROM module_deps md JOIN modules m ON md.module_id = m.id GROUP BY m.id ORDER BY dep_count DESC"

# CTE — transitive dependency chain
ast-index query "WITH RECURSIVE chain AS (SELECT md.dep_module_id, m.name, 1 as depth FROM module_deps md JOIN modules m ON md.dep_module_id = m.id WHERE md.module_id = (SELECT id FROM modules WHERE name LIKE '%payments%') UNION ALL SELECT md.dep_module_id, m.name, c.depth + 1 FROM chain c JOIN module_deps md ON md.module_id = c.dep_module_id JOIN modules m ON md.dep_module_id = m.id WHERE c.depth < 5) SELECT DISTINCT name, depth FROM chain ORDER BY depth, name"
```

**Security**: Only `SELECT`, `WITH`, and `EXPLAIN` queries allowed. Mutations (`INSERT`, `UPDATE`, `DELETE`, `DROP`) are blocked.

### Database Schema

```bash
# Show all tables with columns and row counts (JSON)
ast-index schema
```

Key tables:
| Table | Description | Key columns |
|-------|-------------|-------------|
| `files` | Indexed source files | `path`, `mtime`, `size` |
| `symbols` | Classes, functions, properties, etc. | `name`, `kind`, `line`, `file_id`, `signature` |
| `symbols_fts` | FTS5 full-text search on symbols | `name`, `signature` |
| `inheritance` | Parent-child type relationships | `child_id`, `parent_name`, `kind` |
| `refs` | Symbol references/usages | `name`, `line`, `file_id`, `context` |
| `modules` | Build modules (Gradle, Maven, etc.) | `name`, `path`, `kind` |
| `module_deps` | Module → module dependencies | `module_id`, `dep_module_id` |
| `transitive_deps` | Pre-computed transitive deps | `module_id`, `dependency_id`, `depth` |
| `xml_usages` | Android XML layout class usages | `class_name`, `file_path` |
| `resources` | Android resource definitions | `type`, `name`, `file_path` |

### Direct Database Access (SDK / Scripting)

```bash
# Get the SQLite database path for external tools
ast-index db-path
# Output: /Users/me/.cache/ast-index/abc123/index.db
```

Use this path with any language that supports SQLite:

```python
import sqlite3, json
db_path = subprocess.check_output(["ast-index", "db-path"]).decode().strip()
conn = sqlite3.connect(db_path)

# Example: find all classes implementing an interface with no references
unused_impls = conn.execute("""
    SELECT s.name, f.path, i.parent_name
    FROM symbols s
    JOIN files f ON s.file_id = f.id
    JOIN inheritance i ON s.id = i.child_id
    WHERE s.name NOT IN (SELECT name FROM refs)
    ORDER BY i.parent_name, s.name
""").fetchall()
for name, path, parent in unused_impls:
    print(f"  {name} implements {parent} — {path}")
```

**When to use `query` vs predefined commands:**
- Simple lookups → use `search`, `class`, `usages` (faster, formatted output)
- Complex joins, aggregation, negative conditions → use `query` (one call instead of N)
- Batch analysis, scripting, CI pipelines → use `db-path` + direct SQLite access

## Performance Reference

| Command | Time | Notes |
|---------|------|-------|
| search | ~10ms | Indexed FTS5 search |
| class | ~1ms | Direct index lookup |
| usages | ~8ms | Indexed reference search |
| imports | ~0.3ms | File-based lookup |
| callers | ~1s | Grep-based search |
| map | ~1-3s | SQL aggregation (scales with project size) |
| conventions | ~1-4s | SQL aggregation + import matching |
| rebuild | ~25s–5m | Full project indexing (depends on size) |

## Platform-Specific Commands

### Android/Kotlin/Java/Spring

Consult: `references/android-commands.md`

Java parser indexes: classes, interfaces, enums, methods, constructors, fields, and significant annotations (@RestController, @Service, @Repository, @Component, @Entity, @GetMapping, @PostMapping, @Autowired, @Override, @Transactional, @SpringBootApplication, @Test, @Inject, @Data, @Builder, etc.).

Maven modules (pom.xml) are fully supported alongside Gradle modules.

- **DI Commands**: `provides`, `inject` (@Inject + @Autowired), `annotations`
- **Compose Commands**: `composables`, `previews`
- **Coroutines Commands**: `suspend`, `flows`
- **XML Commands**: `xml-usages`, `resource-usages`
- **Code Quality**: `deprecated`, `suppress`, `todo`
- **Extensions**: `extensions`
- **Navigation**: `deeplinks`

### iOS/Swift/ObjC

Consult: `references/ios-commands.md`

- **Storyboard & XIB**: `storyboard-usages`
- **Assets**: `asset-usages`
- **SwiftUI**: `swiftui`
- **Swift Concurrency**: `async-funcs`, `main-actor`
- **Combine**: `publishers`

### TypeScript/JavaScript

Consult: `references/typescript-commands.md`

- Index: `class`, `interface`, `type`, `function`, `const`, decorators
- Supports: React (hooks, components), Vue SFC, Svelte, NestJS, Angular
- `outline` and `imports` work with TS/JS files

### Rust

Consult: `references/rust-commands.md`

- Index: `struct`, `enum`, `trait`, `impl`, `fn`, `macro_rules!`, `mod`
- Supports: Derives, attributes (`#[test]`, `#[derive]`)
- `outline` and `imports` work with Rust files

### Ruby

Consult: `references/ruby-commands.md`

- Index: `class`, `module`, `def`, Rails DSL
- Supports: RSpec (`describe`, `it`, `let`), Rails (associations, validations)
- `outline` and `imports` work with Ruby files

### C#/.NET

Consult: `references/csharp-commands.md`

- Index: `class`, `interface`, `struct`, `record`, `enum`, methods, properties
- Supports: ASP.NET attributes, Unity (`MonoBehaviour`, `SerializeField`)
- `outline` and `imports` work with C# files

### Dart/Flutter

Consult: `references/dart-commands.md`

- Index: `class`, `mixin`, `extension`, `extension type`, `enum`, `typedef`, functions, constructors
- Supports: Dart 3 modifiers (sealed, final, base, interface, mixin class)
- `outline` and `imports` work with Dart files

### Python

Consult: `references/python-commands.md`

- Index: `class`, `def`, `async def`, decorators
- `outline` and `imports` work with Python files

### Go

Consult: `references/go-commands.md`

- Index: `package`, `type struct`, `type interface`, `func`
- `outline` and `imports` work with Go files

### Scala

- Index: `class`, `case class`, `object`, `trait`, `enum` (Scala 3), `def`, `val`, `var`, `type`, `given`
- Supports: Inheritance (`extends`/`with`), companion objects
- `outline` and `imports` work with Scala files

### PHP

- Index: `namespace`, `class`, `interface`, `trait`, `enum`, `function`, `method`, `const`, `property`, `use`
- Supports: Laravel (models, traits, facades), `extends`/`implements`, namespace imports
- `outline` and `imports` work with PHP files

### Perl

Consult: `references/perl-commands.md`

- **Exports**: `perl-exports`
- **Subroutines**: `perl-subs`
- **POD**: `perl-pod`
- **Imports**: `perl-imports`
- **Tests**: `perl-tests`

### Module Analysis

Consult: `references/module-commands.md`

- **Module Search**: `module`
- **Dependencies**: `deps`, `dependents`
- **Unused Dependencies**: `unused-deps`
- **Unused Symbols**: `unused-symbols`
- **Public API**: `api`

## Workflow Recommendations

1. Run `ast-index rebuild` once in project root to build the index
2. **Start a session** with `ast-index conventions` + `ast-index map` to understand project structure (~80 lines, ~500 tokens)
3. Use `ast-index map --module <path>` to drill down into specific areas
4. Use `ast-index search` for quick universal search when exploring
5. Use `ast-index class` for precise class/interface lookup
6. Use `ast-index usages` to find all references before refactoring
7. Use `ast-index implementations` to understand inheritance
8. Use `ast-index changed --base main` before code review
9. Run `ast-index update` periodically to keep index fresh

## JSON Output (Optional)

**Default output is human-readable plain text** — use it unless you specifically need structured data for scripting.

Add `--format json` only when:
- Parsing output programmatically (pipelines, scripts)
- Need exact field values (file paths, line numbers, symbol kinds)
- Integrating with another tool

Supported commands: `search`, `symbol`, `class`, `usages`, `implementations`, `refs`, `stats`, `unused-symbols`, `map`, `conventions`.

```bash
ast-index search "Query" --format json | jq '.results[].path'
```

## Additional Resources

For detailed platform-specific commands, consult:

- **`references/android-commands.md`** - DI, Compose, Coroutines, XML
- **`references/ios-commands.md`** - Storyboard, SwiftUI, Combine
- **`references/typescript-commands.md`** - React, Vue, Svelte, NestJS, Angular
- **`references/rust-commands.md`** - Structs, traits, impl blocks, macros
- **`references/ruby-commands.md`** - Rails, RSpec, classes, modules
- **`references/csharp-commands.md`** - ASP.NET, Unity, controllers, interfaces
- **`references/dart-commands.md`** - Dart/Flutter classes, mixins, extensions
- **`references/perl-commands.md`** - Perl exports, subs, POD
- **`references/python-commands.md`** - Python classes, functions
- **`references/go-commands.md`** - Go structs, interfaces
- **`references/cpp-commands.md`** - C/C++ classes, JNI functions
- **`references/proto-commands.md`** - Protocol Buffers messages, services
- **`references/wsdl-commands.md`** - WSDL services, XSD types
- **`references/module-commands.md`** - Module dependencies
