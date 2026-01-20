# ast-index v3.5.0 - Code Search for Mobile Projects

Fast native Rust CLI for code search in Android/Kotlin/Java and iOS/Swift/ObjC projects using SQLite + FTS5.

## Prerequisites

Install the CLI:
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

## Core Commands (Cross-Platform)

### Search

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

### Audit

```bash
ast-index todo                       # Find TODO/FIXME/HACK
ast-index deprecated                 # Find @Deprecated items
ast-index suppress                   # Find @Suppress annotations
ast-index extensions "String"        # Find extension functions
ast-index deeplinks                  # Find deeplinks
ast-index changed --base "main"      # Changed symbols (git diff)
ast-index api "features/payments"    # Public API of module
```

### File & Index

```bash
ast-index outline "path/to/File.kt"  # Symbols in file
ast-index init                       # Initialize index
ast-index rebuild                    # Full reindex
ast-index update                     # Incremental update
ast-index stats                      # Index statistics
ast-index version                    # Version info
```

## Platform-Specific Commands

**For Android/Kotlin/Java projects**, see: `references/android-commands.md`
- DI Commands (provides, inject, annotations)
- Compose Commands (composables, previews)
- Coroutines Commands (suspend, flows)
- XML & Resource Commands (xml-usages, resource-usages)

**For iOS/Swift/ObjC projects**, see: `references/ios-commands.md`
- Storyboard & XIB Commands (storyboard-usages)
- Asset Commands (asset-usages)
- SwiftUI Commands (swiftui)
- Swift Concurrency (async-funcs, main-actor)
- Combine Commands (publishers)

**For module analysis**, see: `references/module-commands.md`
- Module Commands (module, deps, dependents, unused-deps)

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
