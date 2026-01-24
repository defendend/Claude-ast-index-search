---
name: initialize-web
description: Initialize ast-index for TypeScript/JavaScript/React/Vue/Svelte project - configures .claude/settings.json, rules, and CLAUDE.md
---

# Initialize ast-index for Web Project

This command sets up ast-index integration for TypeScript/JavaScript web projects (React, Vue, Svelte, Angular, NestJS, etc.).

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
| Find class/component | `ast-index class "ComponentName"` | ~1ms |
| Find symbol | `ast-index symbol "SymbolName"` | ~1ms |
| Find usages | `ast-index usages "SymbolName"` | ~8ms |
| Find implementations | `ast-index implementations "Interface"` | ~5ms |
| Call hierarchy | `ast-index call-tree "function" --depth 3` | ~1s |
| Find callers | `ast-index callers "functionName"` | ~1s |
| Module deps | `ast-index deps "module-name"` | ~10ms |
| File outline | `ast-index outline "File.tsx"` | ~1ms |

## TypeScript/JavaScript-Specific Commands

| Task | Command |
|------|---------|
| Find React components | `ast-index class "Component"` |
| Find React hooks | `ast-index search "use" --kind function` |
| Find decorators | `ast-index search "@Controller"` |
| Find interfaces | `ast-index class "Props"` |
| Find types | `ast-index symbol "DTO"` |

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
ast-index search "Component"
```

## Output

After completion, inform user:
- settings.json has been configured with ast-index permissions
- Rules file created at .claude/rules/ast-index.md
- Skill documentation copied to .claude/skills/ast-index/
- Index has been built with X files and Y symbols
- Ready to use ast-index for code search

## Supported File Types

This initialization supports:
- `.ts`, `.tsx` - TypeScript (including React TSX)
- `.js`, `.jsx` - JavaScript (including React JSX)
- `.mjs`, `.cjs` - ES modules and CommonJS
- `.vue` - Vue Single File Components
- `.svelte` - Svelte components

## Framework Detection

After indexing, you can detect the framework:

```bash
# Check for React
ast-index search "useState" --kind function

# Check for Vue
ast-index search "defineComponent"

# Check for Svelte
ast-index search "export let"

# Check for NestJS
ast-index search "@Controller"

# Check for Angular
ast-index search "@Component"
```
