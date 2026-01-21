---
name: ast-index
description: This skill should be used when the user asks to "find a class", "search for symbol", "find usages", "find implementations", "search codebase", "find file", "class hierarchy", "find callers", "module dependencies", "unused dependencies", "find Perl subs", "Perl exports", "find Python class", "Go struct", "Go interface", or needs fast code search in Android/Kotlin/Java, iOS/Swift/ObjC, Perl, Python, Go, C++, or Protocol Buffers projects. Also triggered by mentions of "ast-index" CLI tool.
---

# ast-index - Code Search for Multi-Platform Projects

Fast native Rust CLI for structural code search in Android/Kotlin/Java, iOS/Swift/ObjC, Perl, Python, Go, C++, and Proto projects using SQLite + FTS5 index.

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

The index is stored at `~/.cache/ast-index/<project-hash>/index.db` and needs rebuilding when project structure changes significantly.

## Supported Projects

| Platform | Languages | Module System |
|----------|-----------|---------------|
| Android | Kotlin, Java | Gradle (build.gradle.kts) |
| iOS | Swift, Objective-C | SPM (Package.swift) |
| Perl | Perl | Makefile.PL, Build.PL |
| Python | Python | None (*.py files) |
| Go | Go | None (*.go files) |
| Proto | Protocol Buffers (proto2/proto3) | None (*.proto files) |
| WSDL | WSDL, XSD | None (*.wsdl, *.xsd files) |
| C/C++ | C, C++ (JNI, uservices) | None (*.cpp, *.h, *.hpp files) |
| Mixed | All above | All |

Project type is auto-detected by marker files (build.gradle.kts, Package.swift, Makefile.PL, etc.). Python, Go, Proto, WSDL, and C++ files are indexed alongside main project type.

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

## Index Management

```bash
ast-index rebuild                    # Full rebuild with dependencies
ast-index rebuild --no-deps          # Skip module dependency indexing
ast-index rebuild --no-ignore        # Include gitignored files
ast-index update                     # Incremental update
ast-index stats                      # Show index statistics
```

## Performance Reference

| Command | Time | Notes |
|---------|------|-------|
| search | ~10ms | Indexed FTS5 search |
| class | ~1ms | Direct index lookup |
| usages | ~8ms | Indexed reference search |
| imports | ~0.3ms | File-based lookup |
| callers | ~1s | Grep-based search |
| rebuild | ~25s | Full project indexing |

## Platform-Specific Commands

### Android/Kotlin/Java

Consult: `references/android-commands.md`

- **DI Commands**: `provides`, `inject`, `annotations`
- **Compose Commands**: `composables`, `previews`
- **Coroutines Commands**: `suspend`, `flows`
- **XML Commands**: `xml-usages`, `resource-usages`

### iOS/Swift/ObjC

Consult: `references/ios-commands.md`

- **Storyboard & XIB**: `storyboard-usages`
- **Assets**: `asset-usages`
- **SwiftUI**: `swiftui`
- **Swift Concurrency**: `async-funcs`, `main-actor`

### Python

Consult: `references/python-commands.md`

- Index: `class`, `def`, `async def`, decorators
- `outline` and `imports` work with Python files

### Go

Consult: `references/go-commands.md`

- Index: `package`, `type struct`, `type interface`, `func`
- `outline` and `imports` work with Go files

### Perl

Consult: `references/perl-commands.md`

- **Exports**: `perl-exports`
- **Subroutines**: `perl-subs`
- **POD**: `perl-pod`

### Module Analysis

Consult: `references/module-commands.md`

- **Module Search**: `module`
- **Dependencies**: `deps`, `dependents`
- **Unused Dependencies**: `unused-deps`

## Workflow Recommendations

1. Run `ast-index rebuild` once in project root to build the index
2. Use `ast-index search` for quick universal search when exploring
3. Use `ast-index class` for precise class/interface lookup
4. Use `ast-index usages` to find all references before refactoring
5. Use `ast-index implementations` to understand inheritance
6. Use `ast-index changed --base main` before code review
7. Run `ast-index update` periodically to keep index fresh

## Additional Resources

For detailed platform-specific commands, consult:

- **`references/android-commands.md`** - DI, Compose, Coroutines, XML
- **`references/ios-commands.md`** - Storyboard, SwiftUI, Combine
- **`references/perl-commands.md`** - Perl exports, subs, POD
- **`references/python-commands.md`** - Python classes, functions
- **`references/go-commands.md`** - Go structs, interfaces
- **`references/module-commands.md`** - Module dependencies
