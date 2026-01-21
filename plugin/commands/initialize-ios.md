---
name: initialize-ios
description: Initialize ast-index for iOS/Swift/ObjC project - configures .claude/settings.json and CLAUDE.md
---

# Initialize ast-index for iOS Project

This command sets up ast-index integration for an iOS/Swift/Objective-C project.

## Steps to Execute

### 1. Check Prerequisites

Verify ast-index is installed:

```bash
ast-index version
```

If not installed, inform user to run:
```bash
brew tap defendend/ast-index
brew install ast-index
```

### 2. Create/Update .claude/settings.json

First, ensure the directory exists:

```bash
mkdir -p .claude
```

Then create or merge into `.claude/settings.json`. If file doesn't exist, create it with this content:

```json
{
  "permissions": {
    "allow": [
      "Bash(ast-index *)"
    ],
    "deny": [
      "Bash(find:*)",
      "Bash(find *)"
    ]
  },
  "rules": [
    "ALWAYS use `ast-index` FIRST for any code search. Only use grep/Search as fallback if ast-index returns no results or for patterns ast-index doesn't support (regex, string literals in code)",
    "NEVER duplicate ast-index results with grep/Search - if ast-index found usages, that's the complete answer",
    "For class/protocol lookup: `ast-index class \"Name\"` (~1ms)",
    "For finding usages: `ast-index usages \"Symbol\"` (~8ms) - returns ALL usages, no grep needed",
    "For protocol conformances: `ast-index implementations \"Protocol\"`",
    "For call hierarchy: `ast-index call-tree \"function\" --depth 3`",
    "For inheritance: `ast-index hierarchy \"Class\"`",
    "For SwiftUI: `ast-index swiftui`",
    "For async functions: `ast-index async-funcs`",
    "For @MainActor: `ast-index main-actor`",
    "For modules: `ast-index deps/dependents \"module\"`",
    "For universal search (files + symbols + content): `ast-index search \"query\"`",
    "grep/Search ONLY for: regex patterns, string literals, comments, or when ast-index returns empty",
    "Run `ast-index update` after git pull/merge to refresh index"
  ]
}
```

**Important**: If `.claude/settings.json` already exists, MERGE the rules array (don't replace). Check for duplicates before adding.

### 3. Update .claude/CLAUDE.md

If `.claude/CLAUDE.md` doesn't exist, create it:

```bash
touch .claude/CLAUDE.md
```

Then append this section at the end of the file:

```markdown

## ast-index - Code Search Tool

**ALWAYS use ast-index FIRST for code search. Do NOT duplicate results with grep/Search.**

Fast native CLI for structural code search in iOS/Swift/Objective-C projects.

### Quick Reference

| Task | Command | Time |
|------|---------|------|
| Universal search | `ast-index search "query"` | ~10ms |
| Find class/protocol | `ast-index class "ClassName"` | ~1ms |
| Find usages | `ast-index usages "SymbolName"` | ~8ms |
| Find conformances | `ast-index implementations "Protocol"` | ~5ms |
| Call hierarchy | `ast-index call-tree "function" --depth 3` | ~1s |
| Class hierarchy | `ast-index hierarchy "ClassName"` | ~5ms |
| Find callers | `ast-index callers "functionName"` | ~1s |
| Module deps | `ast-index deps "ModuleName"` | ~10ms |
| File outline | `ast-index outline "File.swift"` | ~1ms |

### iOS-Specific Commands

| Task | Command |
|------|---------|
| SwiftUI views | `ast-index swiftui` |
| Async functions | `ast-index async-funcs` |
| @MainActor | `ast-index main-actor` |
| Combine publishers | `ast-index publishers` |
| Storyboard usages | `ast-index storyboard-usages "Class"` |
| Asset usages | `ast-index asset-usages "name"` |

### When to use grep/Search instead

- Regex patterns (ast-index uses literal match)
- String literals inside code (`"some text"`)
- Comments content
- When ast-index returns empty results

### Index Management

```bash
ast-index rebuild    # Full reindex (run once after clone)
ast-index update     # After git pull/merge
ast-index stats      # Show index statistics
```
```

### 4. Build the Index

Run initial indexing:

```bash
cd <project-root>
ast-index rebuild
```

Show progress and report statistics when done.

### 5. Verify Setup

Run a quick search to verify everything works:

```bash
ast-index stats
ast-index search "ViewController"
```

## Output

After completion, inform user:
- settings.json has been configured with ast-index rules
- CLAUDE.md has been updated with ast-index reference
- Index has been built with X files and Y symbols
- Ready to use ast-index for code search
