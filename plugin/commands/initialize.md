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

Inspect the current repo root and at least the first two directory levels. Use
targeted file searches or lightweight content checks; do not guess. Build a
**set of stacks**, not a single label.

Use these markers:

- Android/Kotlin/JVM: `settings.gradle*`, `build.gradle*`,
  `libs.versions.toml`, `pom.xml`
- Kotlin Multiplatform (KMP): `kotlin("multiplatform")`,
  `id("org.jetbrains.kotlin.multiplatform")`, `commonMain`, `androidMain`,
  `iosMain`, `iosArm64Main`, `iosSimulatorArm64Main`, `jsMain`, `wasmJsMain`
- iOS/Swift/ObjC: `Package.swift`, `*.xcodeproj`, `*.xcworkspace`, `Podfile`
- Web/TS/JS: `package.json`, `tsconfig.json`, `vite.config.*`, `next.config.*`,
  `nuxt.config.*`, `angular.json`
- Rust: `Cargo.toml`
- C#/.NET: `*.csproj`, `*.sln`, `Directory.Build.props`
- Ruby: `Gemfile`, `*.gemspec`
- Additional supported single-stack hints:
  - Python: `pyproject.toml`, `setup.py`, `setup.cfg`
  - Go: `go.mod`
  - Dart/Flutter: `pubspec.yaml`
  - PHP: `composer.json`
  - Scala: `build.sbt`

Interpretation rules:

- If Android/Kotlin markers and iOS markers coexist with multiplatform markers,
  treat the repo as **KMP**. Include both Android and iOS guidance plus the
  KMP note below.
- If multiple unrelated stacks are present, treat the repo as a
  **polyglot/monorepo** and keep every relevant stack.
- If detection is ambiguous, tell the user exactly which markers were found and
  ask one concise clarification question before writing files.

### 3. Choose the source command(s)

For each detected primary stack, use the matching existing command file as the
source of truth for stack-specific guidance:

- Android -> `plugin/commands/initialize-android.md`
- iOS -> `plugin/commands/initialize-ios.md`
- Web -> `plugin/commands/initialize-web.md`
- Rust -> `plugin/commands/initialize-rust.md`
- C# -> `plugin/commands/initialize-csharp.md`
- Ruby -> `plugin/commands/initialize-ruby.md`

Rules:

- If exactly one of those six stacks is detected, follow that command's flow
  exactly for the stack-specific parts. Do **not** ask the user to choose.
- If KMP or multiple primary stacks are detected, compose the union of the
  relevant stack-specific guidance from those files. Deduplicate shared setup,
  common rules, and repeated index-management text.
- If only Python, Go, Dart, PHP, or Scala are detected, use the common rules
  below plus a short language-specific note with 3-5 representative
  `ast-index` commands. Keep it concise and do not invent commands that do not
  exist.

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
