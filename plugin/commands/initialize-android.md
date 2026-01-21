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
  "contextProtocol": {
    "rules": [
      "ALWAYS use `ast-index` FIRST for any code search. Only use grep/Search as fallback if ast-index returns no results or for patterns ast-index doesn't support (regex, string literals in code)",
      "NEVER duplicate ast-index results with grep/Search - if ast-index found usages, that's the complete answer",
      "For class/interface lookup: `ast-index class \"Name\"` (~1ms)",
      "For finding usages: `ast-index usages \"Symbol\"` (~8ms) - returns ALL usages, no grep needed",
      "For implementations: `ast-index implementations \"Interface\"`",
      "For call hierarchy: `ast-index call-tree \"function\" --depth 3`",
      "For inheritance: `ast-index hierarchy \"Class\"`",
      "For Dagger: `ast-index provides/inject \"Type\"`",
      "For Compose: `ast-index composables`",
      "For modules: `ast-index deps/dependents \"module\"`",
      "For universal search (files + symbols + content): `ast-index search \"query\"`",
      "grep/Search ONLY for: regex patterns, string literals, comments, or when ast-index returns empty",
      "Run `ast-index update` after git pull/merge to refresh index"
    ]
  }
}
```

**Important**: If `.claude/settings.json` already exists, MERGE the `contextProtocol.rules` array (don't replace). Check for duplicates before adding.

### 3. Update .claude/CLAUDE.md

If `.claude/CLAUDE.md` doesn't exist, create it:

```bash
touch .claude/CLAUDE.md
```

Then append this section at the end of the file:

```markdown

## ast-index - Code Search Tool

**ALWAYS use ast-index FIRST for code search. Do NOT duplicate results with grep/Search.**

Fast native CLI for structural code search in Android/Kotlin/Java projects.

### Quick Reference

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

### Android-Specific Commands

| Task | Command |
|------|---------|
| Dagger provides | `ast-index provides "Type"` |
| Dagger inject | `ast-index inject "Type"` |
| Composables | `ast-index composables` |
| Suspend functions | `ast-index suspend` |
| Flows | `ast-index flows` |
| XML usages | `ast-index xml-usages "ViewClass"` |

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

### 4. Copy Skill Documentation to Project (CRITICAL)

**MANDATORY STEP - DO NOT SKIP!** Copy the ast-index skill documentation from the plugin to the project's `.claude/` directory. Without this step, project-level Claude will NOT have access to ast-index documentation.

Execute these commands:

```bash
mkdir -p .claude/skills/ast-index/references
cp "${CLAUDE_PLUGIN_ROOT}/skills/ast-index/SKILL.md" .claude/skills/ast-index/
cp "${CLAUDE_PLUGIN_ROOT}/skills/ast-index/references/"*.md .claude/skills/ast-index/references/
```

After executing, verify the files were copied:

```bash
ls -la .claude/skills/ast-index/
ls -la .claude/skills/ast-index/references/
```

You MUST see SKILL.md and multiple .md files in references/. If not, the copy failed and must be retried.

### 5. Build the Index

Run initial indexing:

```bash
cd <project-root>
ast-index rebuild
```

Show progress and report statistics when done.

### 6. Verify Setup

Run a quick search to verify everything works:

```bash
ast-index stats
ast-index search "Activity"
```

## Output

After completion, inform user:
- settings.json has been configured with ast-index rules
- CLAUDE.md has been updated with ast-index reference
- Skill documentation copied to .claude/skills/ast-index/
- Index has been built with X files and Y symbols
- Ready to use ast-index for code search
