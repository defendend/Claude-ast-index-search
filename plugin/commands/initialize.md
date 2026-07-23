---
name: initialize
description: Auto-detect project stack(s) and initialize ast-index for single-stack, KMP, and polyglot repos
---

# Auto-initialize ast-index

Use this command as the default initializer. Detect the current project's
stack(s), configure `.claude/settings.json`, create
`.claude/rules/ast-index.md`, build the index, and verify setup.

Keep `initialize-android`, `initialize-ios`, `initialize-web`,
`initialize-rust`, `initialize-csharp`, and `initialize-ruby` as manual
overrides only when the user explicitly wants one stack forced.

## Workflow

### 1. Check prerequisites

Verify `ast-index` is installed:

```bash
ast-index version
```

If it is not installed, tell the user to run:

```bash
brew tap defendend/ast-index
brew install ast-index
```

Stop there until the binary exists.

### 2. Detect stack(s) from actual repo markers

Run the built-in detector and parse its JSON output. Do not scan markers by
hand — `ast-index` already implements the same checks and adds
Kotlin Multiplatform (KMP) recognition.
Treat detection as a set of stacks, not a single label.

```bash
ast-index --format json detect-stacks
```

The command prints:

```json
{
  "stacks": [
    { "kind": "android", "label": "Android (Kotlin/Java/JVM)", "markers": ["build.gradle.kts"] },
    { "kind": "ios",     "label": "iOS (Swift/ObjC)",          "markers": ["Package.swift"] },
    { "kind": "kmp",     "label": "Kotlin Multiplatform",      "markers": ["composeApp/commonMain", "build.gradle.kts"] }
  ],
  "is_kmp": true,
  "is_polyglot": false
}
```

Interpretation rules:

- `is_kmp: true` → treat the repo as **KMP**. Include both Android and iOS
  guidance plus the KMP note below.
- `is_polyglot: true` → **polyglot/monorepo**. Include every detected stack's
  guidance.
- Otherwise → **single-stack**. Use the matching per-platform command file as
  the source of truth.
- `stacks` empty → tell the user "no known project markers in this directory"
  and ask whether to proceed against the current root anyway, or to point at
  another path.

If you need to double-check, the supported short `kind` ids are: `android`,
`ios`, `kmp`, `web`, `rust`, `csharp`, `ruby`, `python`, `go`, `dart`, `php`,
`scala`, `zig`, `cpp`, `perl`. Each `marker` is a real path relative to the
repo root, suitable for showing the user in the final summary.

### 3. Choose the source command(s)

Map detected `kind` values to existing per-platform command files as the
source of truth for stack-specific guidance:

| `kind`   | Source command                              |
|----------|---------------------------------------------|
| `android` | `plugin/commands/initialize-android.md`     |
| `ios`    | `plugin/commands/initialize-ios.md`          |
| `web`    | `plugin/commands/initialize-web.md`          |
| `rust`   | `plugin/commands/initialize-rust.md`         |
| `csharp` | `plugin/commands/initialize-csharp.md`       |
| `ruby`   | `plugin/commands/initialize-ruby.md`         |

Rules:

- Single-stack and one of those six → follow that command's flow exactly.
  Do **not** ask the user to choose.
- `is_kmp: true` → compose the union of `initialize-android.md` and
  `initialize-ios.md` guidance, plus the KMP note below. Deduplicate the shared
  setup, common rules, and repeated index-management text.
- `is_polyglot: true` → compose the union of every detected `kind`'s guidance.
- For `python`, `go`, `dart`, `php`, `scala`, `zig`, `cpp`, `perl`: use the
  common rules below plus a short language-specific note with 3-5
  representative `ast-index` commands. Keep it concise and do not invent
  commands that do not exist.

### 4. Create or merge `.claude/settings.json`

First ensure the directory exists:

```bash
mkdir -p .claude
```

Then create or merge into `.claude/settings.json`. If the file does not exist,
create it with this content:

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

Important:

- Merge keys into an existing `.claude/settings.json`; do not replace unrelated
  settings.
- Keep the `ast-index` plugin enabled.
- Keep the `Bash(ast-index *)` permission.

### 5. Create `.claude/rules/ast-index.md`

Create the rules directory:

```bash
mkdir -p .claude/rules
```

If exactly one primary stack is detected, you may reuse the matching
`initialize-*` command's rule content directly.

If KMP, polyglot, or a secondary stack without a dedicated manual override is
detected, create `.claude/rules/ast-index.md` from:

1. The common core below, included exactly once.
2. The relevant stack-specific sections taken from the source command(s) above.
3. The KMP note below when applicable.
4. A compact fallback section for Python/Go/Dart/PHP/Scala when one of those
   stacks is present.

Use this common core verbatim:

```markdown
# ast-index Rules

## Mandatory Search Rules

1. **ALWAYS use ast-index FIRST** for any code search task
2. **NEVER duplicate results** - if ast-index found usages/implementations,
   that IS the complete answer
3. **DO NOT run grep "for completeness"** after ast-index returns results
4. **Use grep/Search ONLY when:**
   - ast-index returns empty results
   - searching for regex patterns (ast-index uses literal match)
   - searching for string literals inside code (`"some text"`)
   - searching in comments content

## Why ast-index

ast-index is much faster than grep on large repos and returns structured,
accurate results.

## Common Command Reference

| Task | Command |
|------|---------|
| Universal search | `ast-index search "query"` |
| Find type/class | `ast-index class "Name"` |
| Find symbol | `ast-index symbol "Name"` |
| Find usages | `ast-index usages "Name"` |
| Find implementations | `ast-index implementations "Interface"` |
| Call hierarchy | `ast-index call-tree "function" --depth 3` |
| Find callers | `ast-index callers "functionName"` |
| Module deps | `ast-index deps "module-name"` |
| File outline | `ast-index outline "path/to/file.ext"` |
| File imports | `ast-index imports "path/to/file.ext"` |

## Index Management

- `ast-index rebuild` - Full reindex (run once after clone)
- `ast-index update` - After git pull/merge
- `ast-index stats` - Show index statistics
```

When KMP is detected, add this section:

```markdown
## Kotlin Multiplatform Notes

- Treat `commonMain`, `commonTest`, and platform source sets (`androidMain`,
  `iosMain`, etc.) as first-class code, not support files.
- When explaining behavior, consider both Kotlin `expect`/`actual` edges and
  Swift/ObjC interop.
- Do not default to Android-only guidance in a KMP repo.
```

For Python/Go/Dart/PHP/Scala-only repos, add a short section that points the
agent at the generic commands that matter most for that language. Keep it short
and practical.

### 6. Build the index

Run the initial index build:

```bash
ast-index rebuild
```

Run it from the current repo root unless the user explicitly asked to
initialize a narrower subtree.

If the repo is clearly a very large monorepo with several unrelated top-level
apps, finish setup at the current root and mention that `.ast-index.yaml`
`include` paths may be helpful later. Do not invent that file unless the user
asked for scoped indexing.

### 7. Verify setup

Always run:

```bash
ast-index stats
```

Then run one or two quick searches that match the detected stack(s):

- Android: `ast-index search "Activity"` or `ast-index search "ViewModel"`
- iOS: `ast-index search "ViewController"`
- Web: `ast-index search "Component"`
- Rust: `ast-index search "fn"`
- C#: `ast-index search "class"`
- Ruby: `ast-index search "class"`
- Python: `ast-index search "def"`
- Go: `ast-index search "func"`
- Dart: `ast-index search "Widget"`
- PHP: `ast-index search "class"`
- Scala: `ast-index search "trait"`

For KMP or polyglot repos, verify at least one query per major detected stack.

### 8. Final output

Report:

- detected stack(s) and the markers that led to that conclusion
- whether setup was single-stack, KMP, or polyglot
- which files were created or merged
- index stats after `rebuild`
- any follow-up note about monorepo scoping or ambiguity that the user should
  know about
