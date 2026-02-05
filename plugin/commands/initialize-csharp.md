---
name: initialize-csharp
description: Initialize ast-index for C#/.NET project - configures .claude/settings.json, rules, and CLAUDE.md
---

# Initialize ast-index for C# Project

This command sets up ast-index integration for C#/.NET projects (ASP.NET, Unity, WPF, etc.).

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
| Find class/interface | `ast-index class "ClassName"` | ~1ms |
| Find symbol | `ast-index symbol "SymbolName"` | ~1ms |
| Find usages | `ast-index usages "SymbolName"` | ~8ms |
| Find implementations | `ast-index implementations "IInterface"` | ~5ms |
| Call hierarchy | `ast-index call-tree "Method" --depth 3` | ~1s |
| Find callers | `ast-index callers "MethodName"` | ~1s |
| File outline | `ast-index outline "Service.cs"` | ~1ms |

## C#-Specific Commands

| Task | Command |
|------|---------|
| Find controllers | `ast-index class "Controller"` |
| Find services | `ast-index class "Service"` |
| Find repositories | `ast-index class "Repository"` |
| Find API endpoints | `ast-index search "[HttpGet]"` |
| Find attributes | `ast-index search "[Authorize]"` |
| Find records | `ast-index search "record"` |

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
ast-index search "class"
```

## Output

After completion, inform user:
- settings.json has been configured with ast-index permissions
- Rules file created at .claude/rules/ast-index.md
- Index has been built with X files and Y symbols
- Ready to use ast-index for code search

## Project Type Detection

Detect the .NET project type:

```bash
# ASP.NET Core Web API
ast-index search "[ApiController]"
ast-index search "ControllerBase"

# ASP.NET MVC
ast-index search "Controller"
ast-index search "ActionResult"

# Unity
ast-index search "MonoBehaviour"
ast-index search "ScriptableObject"

# WPF/MAUI
ast-index search "Window"
ast-index search "UserControl"

# Console/Library
ast-index search "Main"
```
