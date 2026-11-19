---
name: ast-index-initialize
description: Detect the project stacks and initialize ast-index for Grok Build
---

# Initialize ast-index for Grok Build

Set up the current project so Grok uses `ast-index` for structural search.
Write `.grok/rules/ast-index.md` and a project allow rule. Do not write
`.claude/` files, do not add a marketplace, and do not enable the plugin.
Installing the plugin already enables it.

Run every step from the directory the user invoked this command in.

## 1. Check the CLI

```bash
ast-index version
```

If `ast-index` is not on `PATH`, stop. Tell the user to install it with
whichever method fits the machine. The README Installation section lists
Homebrew (macOS and Linux), Cargo, Winget (Windows), npm
(`npm install -g @ast-index/cli`), and a source build. Do not prescribe
Homebrew when another method is already the right one for this machine.
Stop until the binary exists.

## 2. See whether this CLI can detect stacks

```bash
ast-index help detect-stacks
```

If that command fails, or the help text does not describe `detect-stacks`,
the installed CLI is older than this plugin. Do not scan the tree yourself
to guess a stack.

Write only the common core from section 4 into `.grok/rules/ast-index.md`,
then merge the allow rule from section 5. Tell the user the CLI is older
than the plugin and should be upgraded the same way it was installed.
Run `ast-index rebuild` only when `ast-index help rebuild` succeeds, then
`ast-index stats` when that command exists. Skip the stack-specific search.
Report that stack sections were omitted because `detect-stacks` is missing.

If a later `ast-index` subcommand or flag is rejected as unknown, show
`ast-index version` and ask the user to upgrade. Do not invent flags.

## 3. Detect stacks

When `detect-stacks` exists:

```bash
ast-index --format json detect-stacks
```

Treat the result as a set of stacks, not a single label. The JSON has
`stacks` (each with `kind`, `label`, `markers`), `is_kmp`, `is_polyglot`,
and `scan_truncated`.

Supported `kind` values: `android`, `ios`, `kmp`, `web`, `rust`, `csharp`,
`ruby`, `python`, `go`, `dart`, `php`, `scala`, `zig`, `cpp`, `perl`.

- `stacks` empty: say that no known project markers were found. Ask whether
  to write the common core for this root anyway, or to point at another
  path. Do not create files until the user answers. If they choose this
  root, write the common core and the allow rule, then rebuild.
- One `kind` and not KMP: common core plus that kind's block.
- `is_kmp: true`: common core, the Android block, the iOS block, and the
  KMP note. Do not also add a separate `kmp` block.
- `is_polyglot: true`: common core plus one block per detected `kind`.
  Write shared rules once.
- `scan_truncated: true`: same rules, plus one sentence that the walk hit
  its limit and the stack list may be incomplete.
- The user names one stack ("initialize this as Rust"): write only that
  kind's block, even when detection found more.

A very large monorepo with several unrelated top-level apps is still
indexed at the current root. Mention that `.ast-index.yaml` `include`
paths can narrow a later rebuild. Do not create that file.

## 4. Write `.grok/rules/ast-index.md`

```bash
mkdir -p .grok/rules
```

Create the file, or replace an existing `ast-index.md` in that directory.
Leave every other file in `.grok/rules/` alone.

Start with this common core:

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
| One-shot context | `ast-index explore "how does X work"` |
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

If a subcommand or flag is rejected as unknown, run `ast-index version`
and ask to upgrade the CLI. Do not invent flags.
```

Append only the blocks for the stacks selected above.

### android

```markdown
## Android (Kotlin/Java/JVM)

- `ast-index search "ViewModel"`
- `ast-index class "Activity"`
- `ast-index usages "Repository"`
- `ast-index inject "Repository"`
- `ast-index composables`
- `ast-index xml-usages "ClassName"`
```

### ios

```markdown
## iOS (Swift/ObjC)

- `ast-index search "ViewController"`
- `ast-index swiftui`
- `ast-index storyboard-usages "ViewController"`
- `ast-index async-funcs`
- `ast-index outline "Path/To/File.swift"`
```

### web

```markdown
## Web (TypeScript/JavaScript)

- `ast-index search "Component"`
- `ast-index symbol "Props"`
- `ast-index outline "src/App.tsx"`
- `ast-index usages "useEffect"`
```

### rust

```markdown
## Rust

- `ast-index search "fn"`
- `ast-index symbol "StructName"`
- `ast-index usages "function_name"`
- `ast-index outline "src/lib.rs"`
```

### csharp

```markdown
## C# / .NET

- `ast-index search "class"`
- `ast-index class "Controller"`
- `ast-index usages "Service"`
- `ast-index outline "Path/To/File.cs"`
```

### ruby

```markdown
## Ruby

- `ast-index search "class"`
- `ast-index class "ApplicationRecord"`
- `ast-index usages "method_name"`
- `ast-index search "column_name" -t column`
```

### python

```markdown
## Python

- `ast-index search "def"`
- `ast-index class "ClassName"`
- `ast-index usages "function_name"`
- `ast-index outline "path/to/module.py"`
```

### go

```markdown
## Go

- `ast-index search "func"`
- `ast-index symbol "StructName"`
- `ast-index usages "MethodName"`
- `ast-index outline "path/to/file.go"`
```

### dart

```markdown
## Dart / Flutter

- `ast-index search "Widget"`
- `ast-index class "State"`
- `ast-index usages "build"`
- `ast-index outline "lib/main.dart"`
```

### php

```markdown
## PHP

- `ast-index search "class"`
- `ast-index class "Model"`
- `ast-index usages "methodName"`
- `ast-index outline "path/to/File.php"`
```

### scala

```markdown
## Scala

- `ast-index search "trait"`
- `ast-index class "Service"`
- `ast-index usages "method"`
- `ast-index outline "path/to/File.scala"`
```

### zig

```markdown
## Zig

- `ast-index symbol "functionName"`
- `ast-index usages "functionName"`
- `ast-index outline "src/main.zig"`
```

### cpp

```markdown
## C / C++

- `ast-index class "ClassName"`
- `ast-index usages "function_name"`
- `ast-index outline "path/to/file.cpp"`
- `ast-index search "JNI"`
```

### perl

```markdown
## Perl

- `ast-index perl-subs`
- `ast-index perl-exports`
- `ast-index perl-imports "Module::Name"`
- `ast-index usages "sub_name"`
```

### KMP note

Append this when `is_kmp` is true, after the Android and iOS blocks:

```markdown
## Kotlin Multiplatform Notes

- Treat `commonMain`, `commonTest`, and platform source sets (`androidMain`,
  `iosMain`, etc.) as first-class code, not support files.
- When explaining behavior, consider both Kotlin `expect`/`actual` edges and
  Swift/ObjC interop.
- Do not default to Android-only guidance in a KMP repo.
```

## 5. Allow `ast-index` in the project config

Merge into `.grok/config.toml`. Create the file when it is missing. Do not
remove or rewrite unrelated tables.

Add `Bash(ast-index *)` to `[permission] allow` when that entry is absent.
Keep every other allow entry.

```toml
[permission]
allow = ["Bash(ast-index *)"]
```

## 6. Build and check

From this root:

```bash
ast-index rebuild
ast-index stats
```

Then one search for a detected stack. For KMP or a polyglot repo, run one
query per major stack.

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
- Zig: `ast-index search "fn"`
- C / C++: `ast-index search "class"`
- Perl: `ast-index perl-subs`

## 7. Report

Tell the user:

- detected stacks and the markers that led there
- whether setup was single-stack, KMP, polyglot, empty, or a forced stack
- whether `scan_truncated` was set
- which files were created or merged
- index stats after `rebuild`
- any note about upgrading an older CLI, or about narrowing a monorepo
  later with `.ast-index.yaml`
