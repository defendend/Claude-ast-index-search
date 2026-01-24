---
name: initialize-rust
description: Initialize ast-index for Rust project - configures .claude/settings.json, rules, and CLAUDE.md
---

# Initialize ast-index for Rust Project

This command sets up ast-index integration for Rust projects (Cargo-based).

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
  }
}
```

**Important**: If `.claude/settings.json` already exists, MERGE the permissions (don't replace).

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
| Find struct/trait | `ast-index class "StructName"` | ~1ms |
| Find symbol | `ast-index symbol "SymbolName"` | ~1ms |
| Find usages | `ast-index usages "SymbolName"` | ~8ms |
| Find implementations | `ast-index implementations "Trait"` | ~5ms |
| Call hierarchy | `ast-index call-tree "function" --depth 3` | ~1s |
| Find callers | `ast-index callers "functionName"` | ~1s |
| Module deps | `ast-index deps "module-name"` | ~10ms |
| File outline | `ast-index outline "lib.rs"` | ~1ms |

## Rust-Specific Commands

| Task | Command |
|------|---------|
| Find structs | `ast-index class "User"` |
| Find traits | `ast-index class "Repository"` |
| Find impl blocks | `ast-index search "impl"` |
| Find macros | `ast-index search "macro_rules"` |
| Find derives | `ast-index search "#[derive"` |
| Find tests | `ast-index search "#[test]"` |

## Index Management

- `ast-index rebuild` — Full reindex (run once after clone)
- `ast-index update` — After git pull/merge
- `ast-index stats` — Show index statistics
```

### 4. Copy Skill Documentation to Project (CRITICAL)

**MANDATORY STEP - DO NOT SKIP!** Copy the ast-index skill documentation from the plugin to the project's `.claude/` directory.

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
ast-index rebuild
```

Show progress and report statistics when done.

### 6. Verify Setup

Run a quick search to verify everything works:

```bash
ast-index stats
ast-index search "fn"
```

## Output

After completion, inform user:
- settings.json has been configured with ast-index permissions
- Rules file created at .claude/rules/ast-index.md
- Skill documentation copied to .claude/skills/ast-index/
- Index has been built with X files and Y symbols
- Ready to use ast-index for code search

## Cargo Workspace Support

For Cargo workspaces with multiple crates:

```bash
# Index entire workspace from root
cd /path/to/workspace
ast-index rebuild

# Or index specific crate
cd /path/to/workspace/crate-name
ast-index rebuild
```

The indexer will automatically detect and index all `.rs` files in the project.
