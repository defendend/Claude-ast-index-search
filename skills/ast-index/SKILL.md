---
name: ast-index
description: This skill should be used when the user asks to "find a class", "search for symbol", "find usages", "find implementations", "search codebase", "find file", "class hierarchy", "find callers", "module dependencies", "unused dependencies", "find Perl subs", "Perl exports", or needs fast code search in Android/Kotlin/Java, iOS/Swift/ObjC, or Perl projects. Also triggered by mentions of "ast-index" CLI tool.
---

# ast-index - Code Search for Multi-Platform Projects

Fast native Rust CLI for structural code search in Android/Kotlin/Java, iOS/Swift/ObjC, and Perl projects using SQLite + FTS5 index.

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
| Mixed | All above | All |

Project type is auto-detected by marker files (build.gradle.kts, Package.swift, Makefile.PL, etc.).

## Core Commands

### Universal Search

**`search`** - Perform universal search across files, symbols, and modules simultaneously. Best for quick exploration when unsure what type of entity to look for.

```bash
ast-index search "Payment"           # Finds files, classes, functions matching "Payment"
ast-index search "ViewModel"         # Returns files, symbols, modules in ranked order
```

Returns: Combined results from file, symbol, and module searches, ranked by relevance.

### File Search

**`file`** - Find files by name pattern. Supports partial matching and glob patterns.

```bash
ast-index file "Fragment.kt"         # Find files ending with Fragment.kt
ast-index file "ViewController"      # Find iOS view controllers
ast-index file "Test"                # Find all test files
```

Returns: List of file paths matching the pattern with modification times.

### Symbol Search

**`symbol`** - Find symbols (classes, interfaces, functions, properties) by name. Searches the indexed symbol table.

```bash
ast-index symbol "PaymentInteractor" # Find exact symbol
ast-index symbol "Presenter"         # Find all presenters
ast-index symbol "ViewModel"         # Find view models
```

Returns: Symbol name, kind (class/interface/function), file path, and line number.

### Class Search

**`class`** - Find class, interface, or protocol definitions. More precise than symbol search, focuses only on type definitions.

```bash
ast-index class "BaseFragment"       # Find Android base fragment
ast-index class "UIViewController"   # Find iOS view controller subclass
ast-index class "Codable"            # Find Swift protocol
```

Returns: Class/interface name, file path, line number, and inheritance info.

### Usage Search

**`usages`** - Find all places where a symbol is used. Critical for understanding impact of changes and refactoring.

```bash
ast-index usages "PaymentRepository" # Find all usages of repository
ast-index usages "onClick"           # Find all click handler usages
ast-index usages "UserModel"         # Find model usages across codebase
```

Returns: File path, line number, and context line for each usage. Performance: ~8ms for indexed symbols.

### Implementation Search

**`implementations`** - Find all classes that extend or implement a given class/interface/protocol. Essential for understanding inheritance hierarchies.

```bash
ast-index implementations "BasePresenter"  # Find all presenter implementations
ast-index implementations "Repository"     # Find repository implementations
ast-index implementations "Codable"        # Find Swift protocol conformances
```

Returns: List of implementing classes with file paths and line numbers.

### Class Hierarchy

**`hierarchy`** - Display complete class hierarchy tree showing both parents (superclasses) and children (subclasses).

```bash
ast-index hierarchy "BaseFragment"   # Show fragment inheritance tree
ast-index hierarchy "BaseViewModel"  # Show view model hierarchy
```

Returns: Tree structure showing inheritance relationships in both directions.

### Caller Search

**`callers`** - Find all places that call a specific function. Uses grep-based search for flexibility.

```bash
ast-index callers "onClick"          # Find all onClick calls
ast-index callers "viewDidLoad"      # Find iOS lifecycle calls
ast-index callers "fetchUser"        # Find API call sites
```

Returns: File path, line number, and context for each call site. Performance: ~1s (grep-based).

### Call Tree (v3.6.2)

**`call-tree`** - Show complete call hierarchy going UP (who calls the callers). Essential for understanding how a function is reached.

```bash
ast-index call-tree "processPayment" --depth 3 --limit 10
```

Output:
```
Call tree for 'processPayment':
  processPayment
    ← handlePayment (PaymentPresenter.kt:45)
      ← onPayButtonClick (CheckoutFragment.kt:112)
        ← onClick (CheckoutFragment.kt:89)
    ← retryPayment (PaymentRetryInteractor.kt:33)
```

Options:
- `--depth N` - Maximum depth of the tree (default: 3)
- `--limit N` - Maximum callers per level (default: 10)

Works across Kotlin, Java, Swift, Objective-C, and Perl.

### Import Analysis

**`imports`** - List all imports in a specific file. Useful for understanding file dependencies.

```bash
ast-index imports "path/to/File.kt"  # Show Kotlin file imports
ast-index imports "ViewController.swift"  # Show Swift file imports
```

Returns: List of import statements with line numbers. Performance: ~0.3ms.

### File Outline

**`outline`** - Show all symbols defined in a file (classes, functions, properties). Quick way to understand file structure.

```bash
ast-index outline "PaymentFragment.kt"    # Show fragment structure
ast-index outline "ViewModel.swift"       # Show Swift file structure
```

Returns: Hierarchical list of symbols with kinds and line numbers.

## Audit Commands

### TODO Search

**`todo`** - Find TODO, FIXME, and HACK comments across the codebase.

```bash
ast-index todo                       # Find all TODOs
ast-index todo "FIXME"               # Find only FIXMEs
ast-index todo "payment"             # Find TODOs mentioning payment
```

Returns: File path, line number, and comment text.

### Deprecated Search

**`deprecated`** - Find all deprecated items marked with @Deprecated or @available(*, deprecated).

```bash
ast-index deprecated                 # Find all deprecated items
ast-index deprecated "API"           # Find deprecated APIs
```

Returns: Deprecated symbols with file locations and deprecation messages.

### Suppress Annotations

**`suppress`** - Find @Suppress annotations to audit suppressed warnings.

```bash
ast-index suppress                   # Find all suppressions
ast-index suppress "UNCHECKED_CAST"  # Find specific suppression
```

Returns: Suppression annotations with reasons and locations.

### Extension Functions

**`extensions`** - Find extension functions for a specific type.

```bash
ast-index extensions "String"        # Find String extensions
ast-index extensions "View"          # Find View extensions
ast-index extensions "List"          # Find collection extensions
```

Returns: Extension function signatures with file locations.

### Deeplink Search

**`deeplinks`** - Find deeplink definitions and handlers in the codebase.

```bash
ast-index deeplinks                  # Find all deeplinks
ast-index deeplinks "payment"        # Find payment-related deeplinks
```

Returns: Deeplink patterns and handler locations.

### Changed Symbols

**`changed`** - Show symbols changed in current git diff. Useful for code review and impact analysis.

```bash
ast-index changed                    # Changes vs current branch
ast-index changed --base "main"      # Changes vs main branch
ast-index changed --base "HEAD~5"    # Changes in last 5 commits
```

Returns: Added, modified, and removed symbols with file locations.

### Public API

**`api`** - Show public API of a module. Lists all public classes, interfaces, and functions.

```bash
ast-index api "features/payments/api"     # Payment module API
ast-index api "core/network"              # Network module API
```

Returns: Public symbols organized by type.

## Index Management

**`init`** - Create empty index database without scanning files.

**`rebuild`** - Full reindex of the project. Run when project structure changes or index is corrupted.

```bash
ast-index rebuild                    # Full rebuild with dependencies
ast-index rebuild --no-deps          # Skip module dependency indexing
ast-index rebuild --no-ignore        # Include gitignored files (build/ directories)
```

The `--no-ignore` flag (v3.6.2) indexes files in gitignored directories like `build/`, useful for finding generated code like `BuildConfig.java`.

**`update`** - Incremental index update. Faster than rebuild, only processes changed files.

**`stats`** - Show index statistics (file count, symbol count, index size).

**`version`** - Show CLI version.

## Platform-Specific Commands

### Android/Kotlin/Java

For DI, Compose, Coroutines, and XML commands, consult: `references/android-commands.md`

- **DI Commands**: `provides`, `inject`, `annotations` - Find Dagger dependency injection points
- **Compose Commands**: `composables`, `previews` - Find Jetpack Compose functions
- **Coroutines Commands**: `suspend`, `flows` - Find coroutine and Flow usage
- **XML & Resource Commands**: `xml-usages`, `resource-usages` - Find layout and resource usage

### iOS/Swift/ObjC

For Storyboard, Assets, SwiftUI, and Concurrency commands, consult: `references/ios-commands.md`

- **Storyboard & XIB**: `storyboard-usages` - Find class usage in Interface Builder
- **Assets**: `asset-usages` - Find xcassets usage
- **SwiftUI**: `swiftui` - Find @State, @Binding, @Published properties
- **Swift Concurrency**: `async-funcs`, `main-actor` - Find async/await patterns
- **Combine**: `publishers` - Find Combine publishers

### Perl

For Perl-specific commands, consult: `references/perl-commands.md`

- **Exports**: `perl-exports` - Find @EXPORT and @EXPORT_OK definitions
- **Subroutines**: `perl-subs` - Find all subroutine definitions
- **POD**: `perl-pod` - Find POD documentation sections (=head1, =item, etc.)
- **Tests**: `perl-tests` - Find Test::More assertions (ok, is, like, etc.)
- **Imports**: `perl-imports` - Find use/require statements
- **Indexed Symbols**: `package`, `sub`, `use constant`, `our` variables
- **Inheritance**: `use base`, `use parent`, `@ISA` relationships
- **Modules**: Perl packages indexed as modules for `module` command

### Module Analysis

For module dependency analysis, consult: `references/module-commands.md`

- **Module Search**: `module` - Find modules by name
- **Dependencies**: `deps`, `dependents` - Analyze module dependencies
- **Unused Dependencies**: `unused-deps` - Find unused module dependencies

## Performance Reference

| Command | Time | Notes |
|---------|------|-------|
| search | ~10ms | Indexed FTS5 search |
| class | ~1ms | Direct index lookup |
| usages | ~8ms | Indexed reference search |
| imports | ~0.3ms | File-based lookup |
| deps/dependents | ~2ms | Module graph traversal |
| callers | ~1s | Grep-based search |
| call-tree | ~5-20s | Recursive grep + file read |
| todo | ~0.8s | Grep-based search |
| rebuild | ~25s | Full project indexing |
| update | ~1s | Incremental update |

## Workflow Recommendations

To search effectively in a codebase:

1. Run `ast-index rebuild` once in project root to build the index
2. Use `ast-index search` for quick universal search when exploring
3. Use `ast-index class` for precise class/interface lookup
4. Use `ast-index usages` to find all references before refactoring
5. Use `ast-index implementations` to understand inheritance
6. Use `ast-index changed --base main` before code review
7. Run `ast-index update` periodically to keep index fresh
8. Consult platform-specific references for specialized commands

## Additional Resources

### Reference Files

For detailed platform-specific commands, consult:

- **`references/android-commands.md`** - DI (Dagger), Compose, Coroutines, XML/Resource commands
- **`references/ios-commands.md`** - Storyboard, SwiftUI, Swift Concurrency, Combine commands
- **`references/perl-commands.md`** - Perl exports, subs, POD, tests, imports
- **`references/module-commands.md`** - Module search, dependencies, unused deps analysis
