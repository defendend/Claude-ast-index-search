# ast-index Setup Guide

Fast code search CLI for AI coding agents and developers. Single binary with
no external runtime dependencies.

## Install

### macOS / Linux (Homebrew)

```bash
brew tap defendend/ast-index
brew install ast-index
```

### Cargo (pending first crates.io release)

After the first ast-index version is published on crates.io:

```bash
cargo install ast-index --locked
```

Until that rollout, use Homebrew or a release archive.

### Manual

Download binary from [GitHub Releases](https://github.com/defendend/Claude-ast-index-search/releases) and add to PATH.

## Quick Start

```bash
# Build the index (run once, from project root)
ast-index rebuild

# Update index after changes (incremental, fast)
ast-index update
```

## Core Commands

### Search

```bash
# Universal search — finds files, symbols, and references
ast-index search "UserRepository"

# Find symbols (classes, functions, interfaces)
ast-index symbol "UserRepository"

# Find files by name
ast-index file "UserRepo"

# Find usages of a symbol
ast-index usages "UserRepository"

# Cross-references — definitions, imports, usages in one call
ast-index refs "UserRepository"
```

### Navigation

```bash
# Class hierarchy (parents and children)
ast-index hierarchy "BaseViewModel"

# Find all implementations of an interface
ast-index implementations "Repository"

# Show symbols in a file
ast-index outline src/main/UserRepository.kt

# Show imports in a file
ast-index imports src/main/UserRepository.kt
```

### Modules & Dependencies

```bash
# List modules
ast-index module ""

# Show dependencies of a module
ast-index deps "app"

# Find who depends on a module
ast-index dependents "core"

# Find unused dependencies
ast-index unused-deps "app"

# Show public API of a module
ast-index api "core"
```

### Project Overview

```bash
# Compact project map — key types per directory
ast-index map

# Detect conventions (architecture, frameworks, naming)
ast-index conventions

# Index statistics
ast-index stats
```

### Code Patterns

```bash
# Find TODO/FIXME comments
ast-index todo

# Find callers of a function
ast-index callers "fetchUser"

# Call hierarchy tree
ast-index call-tree "fetchUser"

# Find classes with annotation
ast-index annotations "RestController"

# Find deprecated items
ast-index deprecated
```

### Changed files on a branch

```bash
# Auto-detect the branch base
ast-index changed

# Compare merge-base(origin/develop, HEAD) to HEAD
ast-index changed --base origin/develop

# Structured output; VCS timeout defaults to 30000 ms
ast-index --format json changed --base origin/develop --timeout-ms 30000

# Print the detected root, scope, exact VCS argv, and timing to stderr
ast-index changed --verbose
```

`changed` is a cache-independent branch summary: it does not read the
ast-index database, so no `rebuild` or `update` is required. The current
working directory defines the scope; returned paths remain relative to the
repository root. Staged and unstaged working-tree edits are not included.
Without `--base`, Git resolves `origin/HEAD`, then tries `origin/main`,
`origin/master`, `main`, `master`, and `trunk`. Other supported version-control
backends select their conventional mainline automatically.

Text output reports added, modified, deleted, and renamed files:

```text
Changed files against origin/develop (5):
  A  docs/new-guide.md
  M  README.md
  D  docs/obsolete.md
  R  docs/old-name.md -> docs/new-name.md
  M  docs/generated\nname.md
```

Control characters and backslashes in text paths are escaped, keeping each
change on one output line. Scripts should consume JSON rather than parse this
human-readable summary.

JSON uses schema v1 and keeps the old path for renames:

```json
{
  "schema_version": 1,
  "vcs": "git",
  "base": "origin/develop",
  "head": "HEAD",
  "scope": null,
  "changes": [
    { "status": "M", "path": "README.md" },
    { "status": "R", "path": "docs/new-name.md", "old_path": "docs/old-name.md" }
  ]
}
```

At repository root `scope` is `null`; in a nested working directory it is the
repository-relative directory path.

Use your version-control system's diff command when you need patch hunks.
`changed` reports files and statuses, not changed declarations or symbols.

### Structural Search (ast-grep)

```bash
# Find pattern in code using ast-grep metavariables
ast-index agrep "fetchUser($$$)" --lang kotlin
ast-index agrep "if ($COND) { return $VAL; }" --lang typescript
```

### Watch Mode

```bash
# Auto-update index on file changes
ast-index watch
```

### Unused Code

```bash
# Find potentially unused symbols
ast-index unused-symbols

# Find unused module dependencies
ast-index unused-deps "app"
```

## Platform-Specific Commands

### Android / Kotlin

```bash
# Find XML layout usages of a class
ast-index xml-usages "MyAdapter"

# Find resource usages (drawables, strings, etc.)
ast-index resource-usages "ic_launcher"

# Find @Composable functions
ast-index composables

# Find suspend functions
ast-index suspend

# Find Flow/StateFlow/SharedFlow
ast-index flows

# Find @Inject points (Dagger/Hilt)
ast-index inject

# Find @Provides/@Binds (Dagger)
ast-index provides

# Find deeplinks
ast-index deeplinks

# Find @Preview functions
ast-index previews
```

### iOS / Swift

```bash
# Find class usages in storyboards/xibs
ast-index storyboard-usages "MyViewController"

# Find iOS asset usages (xcassets)
ast-index asset-usages "AppIcon"

# Find SwiftUI views and state properties
ast-index swiftui

# Find async functions
ast-index async-funcs

# Find Combine publishers
ast-index publishers

# Find @MainActor annotations
ast-index main-actor
```

### Perl

```bash
# Find exported functions (@EXPORT)
ast-index perl-exports

# Find subroutines
ast-index perl-subs

# Find POD documentation
ast-index perl-pod

# Find test assertions
ast-index perl-tests

# Find use/require statements
ast-index perl-imports
```

## Multi-Root Projects

Independent worktrees should each have their own index. Use a named subtree
only when separate directories intentionally form one logical workspace.

```bash
# 1. Create the primary index; subtree commands require it.
ast-index rebuild

# 2. Attach a stable, human-readable name.
ast-index subtree add shared /path/to/shared-lib

# 3. Index the newly attached files and inspect the workspace.
ast-index update
ast-index subtree list

# Scope a query, or restrict it to the primary project.
ast-index --subtree shared search "Repository"
ast-index --local search "Repository"

# Detach by name, then drop its indexed files.
ast-index subtree remove shared
ast-index rebuild
```

A full `rebuild` after `subtree add` is also valid and preserves the named
attachment. The older `add-root`, `remove-root`, and `list-roots` forms remain
compatibility aliases; prefer `subtree add/remove/list` in new automation.

## JSON Output

Add `--format json` for structured output (useful for AI agents):

```bash
ast-index --format json search "UserRepository"
ast-index --format json symbol "fetchUser" --kind function
ast-index --format json refs "UserRepository"
ast-index --format json changed --base origin/main
```

Paginated search commands use schema v2. Most return
`{ schema_version, items, pagination }`, where `pagination` contains `total`,
`returned`, `truncated`, and `limit`. `search` and `refs` retain their named
arrays and expose pagination metadata for each array. Migrate consumers that
expect a bare array to read `items`, and check `truncated` before assuming the
response is complete. Increase `--limit` when necessary. `changed` remains on
its separate schema v1.

## Supported Languages

| Platform | Languages | Extensions |
|----------|-----------|------------|
| Android | Kotlin, Java | `.kt`, `.java` |
| iOS | Swift, Objective-C | `.swift`, `.m`, `.h` |
| Web | TypeScript, JavaScript | `.ts`, `.tsx`, `.mts`, `.js`, `.jsx`, `.vue`, `.svelte` |
| Systems | Rust, C/C++ | `.rs`, `.cpp`, `.cc`, `.c`, `.h`, `.hpp` |
| Backend | C#, Python, Go, Scala, PHP | `.cs`, `.py`, `.go`, `.scala`, `.php` |
| Scripting | Ruby, Perl | `.rb`, `.pm`, `.pl` |
| Mobile | Dart/Flutter | `.dart` |
| Schema | Protocol Buffers, WSDL/XSD | `.proto`, `.wsdl`, `.xsd` |
| Enterprise | BSL (1C:Enterprise) | `.bsl`, `.os` |

## Programmatic Access

```bash
# Execute raw SQL against the index
ast-index query "SELECT name, kind FROM symbols WHERE name LIKE '%User%' LIMIT 10"

# Get path to SQLite database (for direct access from Python, JS, etc.)
ast-index db-path

# Show database schema
ast-index schema
```

## Tips

- Run `ast-index rebuild` once, then use `ast-index update` for incremental updates
- Use `ast-index --format json` when integrating with AI agents
- For monorepos with 50k+ files, sub-projects mode activates automatically
- Use `ast-index query "SELECT ..."` for custom SQL queries against the index
