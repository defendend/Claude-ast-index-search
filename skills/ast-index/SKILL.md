---
name: ast-index
description: This skill should be used when the user asks to "find a class", "search for symbol", "find usages", "find implementations", "search codebase", "find file", "class hierarchy", "find callers", "module dependencies", "unused dependencies", or needs fast code search in Android/Kotlin/Java or iOS/Swift/ObjC projects. Also triggered by mentions of "ast-index" CLI tool.
---

# ast-index - Code Search for Mobile Projects

Fast native Rust CLI for structural code search in Android/Kotlin/Java and iOS/Swift/ObjC projects using SQLite + FTS5 index.

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

## Supported Projects

| Platform | Languages | Module System |
|----------|-----------|---------------|
| Android | Kotlin, Java | Gradle (build.gradle.kts) |
| iOS | Swift, Objective-C | SPM (Package.swift) |
| Mixed | All above | Both |

Project type is auto-detected by marker files.

## Core Commands

### Search Commands

To search for code elements, use these commands:

```bash
ast-index search "PaymentMethod"     # Universal search (files + symbols + modules)
ast-index file "Fragment.kt"         # Find files by name
ast-index symbol "PaymentInteractor" # Find symbols (classes, functions)
ast-index class "BaseFragment"       # Find class/interface/protocol
ast-index usages "Repository"        # Find usages (~8ms indexed)
ast-index implementations "Base"     # Find subclasses/implementors
ast-index hierarchy "BaseFragment"   # Class hierarchy tree
ast-index callers "onClick"          # Find function callers
ast-index imports "path/to/File.kt"  # File imports
```

### Audit Commands

To find code issues and patterns:

```bash
ast-index todo                       # Find TODO/FIXME/HACK
ast-index deprecated                 # Find @Deprecated items
ast-index suppress                   # Find @Suppress annotations
ast-index extensions "String"        # Find extension functions
ast-index deeplinks                  # Find deeplinks
ast-index changed --base "main"      # Changed symbols (git diff)
ast-index api "features/payments"    # Public API of module
```

### Index Management

To manage the search index:

```bash
ast-index init                       # Initialize empty database
ast-index rebuild                    # Full reindex
ast-index update                     # Incremental update
ast-index stats                      # Index statistics
ast-index version                    # Version info
ast-index outline "path/to/File.kt"  # Symbols in file
```

## Platform-Specific Commands

### Android/Kotlin/Java

For DI, Compose, Coroutines, and XML commands, consult: `references/android-commands.md`

- DI Commands: `provides`, `inject`, `annotations`
- Compose Commands: `composables`, `previews`
- Coroutines Commands: `suspend`, `flows`
- XML & Resource Commands: `xml-usages`, `resource-usages`

### iOS/Swift/ObjC

For Storyboard, Assets, SwiftUI, and Concurrency commands, consult: `references/ios-commands.md`

- Storyboard & XIB: `storyboard-usages`
- Assets: `asset-usages`
- SwiftUI: `swiftui`
- Swift Concurrency: `async-funcs`, `main-actor`
- Combine: `publishers`

### Module Analysis

For module dependency analysis, consult: `references/module-commands.md`

- Module Commands: `module`, `deps`, `dependents`, `unused-deps`

## Performance

| Command | Time |
|---------|------|
| search | ~10ms |
| class | ~1ms |
| usages | ~8ms (indexed) |
| imports | ~0.3ms |
| deps/dependents | ~2ms |

## Index Location

Database stored at: `~/.cache/ast-index/<project-hash>/index.db`

## Workflow Recommendations

To search effectively in a codebase:

1. Run `ast-index rebuild` once in project root to build the index
2. Use `ast-index search` for quick universal search
3. Use `ast-index class` for precise class lookup
4. Use `ast-index usages` to find all references to a symbol
5. Use `ast-index implementations` to find subclasses
6. Consult platform-specific references for specialized commands
