---
name: initialize-android
description: Initialize ast-index for Android/Kotlin/Java project - configures .claude/settings.json and CLAUDE.md
---

# Initialize ast-index for Android Project

This command sets up ast-index integration for an Android/Kotlin/Java project.

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
    ]
  },
  "rules": [
    "When searching for classes, symbols, files, or code patterns - use `ast-index` CLI for fast indexed search instead of grep/find/Glob",
    "For finding class definitions use `ast-index class \"ClassName\"`",
    "For finding symbol usages use `ast-index usages \"SymbolName\"`",
    "For finding implementations of interface/class use `ast-index implementations \"InterfaceName\"`",
    "For understanding call hierarchy use `ast-index call-tree \"functionName\" --depth 3`",
    "For exploring class inheritance use `ast-index hierarchy \"ClassName\"`",
    "For finding Dagger provides/inject points use `ast-index provides/inject \"ModuleName\"`",
    "For finding Compose functions use `ast-index composables`",
    "For module dependency analysis use `ast-index deps/dependents \"module-name\"`",
    "Run `ast-index update` periodically to keep index fresh after code changes"
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

Fast native CLI for structural code search in Android/Kotlin/Java projects.

### Quick Reference

| Task | Command |
|------|---------|
| Universal search | `ast-index search "query"` |
| Find class | `ast-index class "ClassName"` |
| Find usages | `ast-index usages "SymbolName"` |
| Find implementations | `ast-index implementations "Interface"` |
| Call hierarchy | `ast-index call-tree "function" --depth 3` |
| Class hierarchy | `ast-index hierarchy "ClassName"` |
| Find callers | `ast-index callers "functionName"` |
| Module deps | `ast-index deps "module-name"` |
| File outline | `ast-index outline "File.kt"` |

### Android-Specific Commands

| Task | Command |
|------|---------|
| Dagger provides | `ast-index provides "query"` |
| Dagger inject | `ast-index inject "query"` |
| Composables | `ast-index composables "query"` |
| Suspend functions | `ast-index suspend "query"` |
| Flows | `ast-index flows "query"` |
| XML usages | `ast-index xml-usages "layout_name"` |

### Index Management

```bash
ast-index rebuild    # Full reindex (run once after clone)
ast-index update     # Incremental update (run periodically)
ast-index stats      # Show index statistics
```

Performance: search ~10ms, usages ~8ms, class ~1ms (indexed queries).
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
ast-index search "Activity"
```

## Output

After completion, inform user:
- settings.json has been configured with ast-index rules
- CLAUDE.md has been updated with ast-index reference
- Index has been built with X files and Y symbols
- Ready to use ast-index for code search
