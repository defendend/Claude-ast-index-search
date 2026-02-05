---
name: initialize-android
description: Initialize ast-index for Android/Kotlin/Java project - configures .claude/settings.json, rules, and CLAUDE.md
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
  "extraKnownMarketplaces": {
    "ast-index": {
      "source": {
        "source": "github",
        "repo": "defendend/Claude-ast-index-search"
      }
    }
  },
  "enabledPlugins": {
    "ast-index@ast-index": true
  },
  "permissions": {
    "allow": [
      "Bash(ya tool ast-index *)",
      "Bash(ast-index *)"
    ]
  }
}
```

**Important**: If `.claude/settings.json` already exists, MERGE the keys (don't replace the whole file).

### 3. Create .claude/rules/ast-index.md (CRITICAL)

Create the rules directory and ast-index rules file:

```bash
mkdir -p .claude/rules
```

Create file `.claude/rules/ast-index.md` with this content:

```markdown
# ast-index Rules

## Mandatory Search Rules

1. **ALWAYS use ast-index FIRST** for any code search task
2. **NEVER duplicate results** — if ast-index found usages/implementations, that IS the complete answer
3. **DO NOT run grep "for completeness"** after ast-index returns results
4. **Use grep/Search ONLY when:**
   - ast-index returns empty results
   - Searching for regex patterns (ast-index uses literal match)
   - Searching for string literals inside code (`"some text"`)
   - Searching in comments content

## Why ast-index

ast-index is 17-69x faster than grep (1-10ms vs 200ms-3s) and returns structured, accurate results.

## Command Reference

| Task | Command | Time |
|------|---------|------|
| Universal search | `ast-index search "query"` | ~10ms |
| Find class | `ast-index class "ClassName"` | ~1ms |
| Find usages | `ast-index usages "SymbolName"` | ~8ms |
| Find implementations | `ast-index implementations "Interface"` | ~5ms |
| Call hierarchy | `ast-index call-tree "function" --depth 3` | ~1s |
| Class hierarchy | `ast-index hierarchy "ClassName"` | ~5ms |
| Find callers | `ast-index callers "functionName"` | ~1s |
| Module deps | `ast-index deps "module-name"` | ~10ms |
| File outline | `ast-index outline "File.kt"` | ~1ms |

## Android-Specific Commands

| Task | Command |
|------|---------|
| Dagger provides | `ast-index provides "Type"` |
| Dagger inject | `ast-index inject "Type"` |
| Composables | `ast-index composables` |
| Suspend functions | `ast-index suspend` |
| Flows | `ast-index flows` |
| XML usages | `ast-index xml-usages "ViewClass"` |

## Index Management

- `ast-index rebuild` — Full reindex (run once after clone)
- `ast-index update` — After git pull/merge
- `ast-index stats` — Show index statistics
```

### 4. Build the Index

Run initial indexing:

```bash
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
- settings.json has been configured with ast-index permissions
- Rules file created at .claude/rules/ast-index.md
- Index has been built with X files and Y symbols
- Ready to use ast-index for code search
