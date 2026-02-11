---
name: ast-index
description: This skill should be used when the user asks to "find a class", "search for symbol", "find usages", "find implementations", "search codebase", "find file", "class hierarchy", "find callers", "module dependencies", "unused dependencies", "project map", "project conventions", "project structure", "what frameworks", "what architecture", "find Perl subs", "Perl exports", "find Python class", "Go struct", "Go interface", "find React component", "find TypeScript interface", "find Rust struct", "find Ruby class", "find C# controller", "find Dart class", "find Flutter widget", "find mixin", "find Scala trait", "find case class", "find object", or needs fast code search in Android/Kotlin/Java, iOS/Swift/ObjC, Dart/Flutter, TypeScript/JavaScript, Rust, Ruby, C#, Scala, Perl, Python, Go, C++, or Protocol Buffers projects. Also triggered by mentions of "ast-index" CLI tool.
---

# ast-index - Code Search for Multi-Platform Projects

Fast native Rust CLI for structural code search in Android/Kotlin/Java, iOS/Swift/ObjC, Dart/Flutter, TypeScript/JavaScript, Rust, Ruby, C#, Scala, Perl, Python, Go, C++, and Proto projects using SQLite + FTS5 index.

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
| Android | Kotlin, Java | Gradle (build.gradle.kts) |
| iOS | Swift, Objective-C | SPM (Package.swift) |
| Web | TypeScript, JavaScript, React, Vue, Svelte | package.json |
| Rust | Rust | Cargo.toml |
| Ruby | Ruby, Rails, RSpec | Gemfile |
| .NET | C#, ASP.NET, Unity | *.csproj |
| Dart/Flutter | Dart | pubspec.yaml |
| Scala | Scala | Bazel (WORKSPACE, BUILD) |
| Perl | Perl | Makefile.PL, Build.PL |
| Python | Python | None (*.py files) |
| Go | Go | None (*.go files) |
| Proto | Protocol Buffers (proto2/proto3) | None (*.proto files) |
| WSDL | WSDL, XSD | None (*.wsdl, *.xsd files) |
| C/C++ | C, C++ (JNI, uservices) | None (*.cpp, *.h, *.hpp files) |
| Mixed | All above | All |

Project type is auto-detected by marker files (build.gradle.kts, Package.swift, Makefile.PL, etc.). Python, Go, Proto, WSDL, and C++ files are indexed alongside main project type.

## JSON Output

Most commands support `--format json` for structured output:

```bash
ast-index search "Query" --format json
ast-index symbol "Name" --format json
ast-index class "Name" --format json
ast-index usages "Symbol" --format json
ast-index implementations "Parent" --format json
ast-index refs "Symbol" --format json
ast-index stats --format json
ast-index unused-symbols --format json
ast-index map --format json
ast-index conventions --format json
```

## Core Commands

### Universal Search

**`search`** - Perform universal search across files, symbols, and modules simultaneously.

```bash
ast-index search "Payment"           # Finds files, classes, functions matching "Payment"
ast-index search "ViewModel"         # Returns files, symbols, modules in ranked order
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
```

### Class Search

**`class`** - Find class, interface, or protocol definitions.

```bash
ast-index class "BaseFragment"       # Find Android base fragment
ast-index class "UIViewController"   # Find iOS view controller subclass
```

### Usage Search

**`usages`** - Find all places where a symbol is used. Critical for refactoring.

```bash
ast-index usages "PaymentRepository" # Find all usages of repository
ast-index usages "onClick"           # Find all click handler usages
```

Performance: ~8ms for indexed symbols.

### Cross-References

**`refs`** - Show cross-references for a symbol: definitions, imports, and usages in one view.

```bash
ast-index refs "PaymentRepository"   # Definitions + imports + usages
ast-index refs "BaseFragment" --limit 10  # Limit results per section
```

### Implementation Search

**`implementations`** - Find all classes that extend or implement a given class/interface/protocol.

```bash
ast-index implementations "BasePresenter"  # Find all presenter implementations
ast-index implementations "Repository"     # Find repository implementations
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

**`call-tree`** - Show complete call hierarchy going UP (who calls the callers).

```bash
ast-index call-tree "processPayment" --depth 3 --limit 10
```

### File Analysis

**`imports`** - List all imports in a specific file.

```bash
ast-index imports "path/to/File.kt"  # Show Kotlin file imports
```

**`outline`** - Show all symbols defined in a file.

```bash
ast-index outline "PaymentFragment.kt"    # Show fragment structure
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

## Index Management

```bash
ast-index init                           # Initialize index for current project
```

```bash
ast-index rebuild                    # Full rebuild with dependencies
ast-index rebuild --no-deps          # Skip module dependency indexing
ast-index rebuild --no-ignore        # Include gitignored files
ast-index update                     # Incremental update
ast-index stats                      # Show index statistics
ast-index clear                      # Delete index for current project
ast-index watch                      # Watch for file changes and auto-update index
```

## Multi-Root Projects

Add additional source roots for monorepos or multi-project setups:

```bash
ast-index add-root /path/to/other/source    # Add source root
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

### Android/Kotlin/Java

Consult: `references/android-commands.md`

- **DI Commands**: `provides`, `inject`, `annotations`
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
