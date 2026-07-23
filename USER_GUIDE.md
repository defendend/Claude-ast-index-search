# ast-index User Guide

This guide is for developers and AI coding agents that need fast, structural
code search inside an existing project.

`ast-index` is a native Rust CLI that builds a local SQLite + FTS5 index of
files, symbols, references, modules, and inheritance. After the first build,
most lookups run in milliseconds instead of repeatedly scanning the whole
repository.

## Installation

Install a ready-made binary with one of the public distribution channels below.

### Homebrew (macOS / Linux)

```bash
brew tap defendend/ast-index
brew install ast-index
```

You can also use the one-line tap form:

```bash
brew install defendend/ast-index/ast-index
```

### npm

Install globally:

```bash
npm install -g @ast-index/cli
```

Or run without a global install:

```bash
npx @ast-index/cli rebuild
npx @ast-index/cli search MyClass
```

### Winget (Windows)

```powershell
winget install --id defendend.ast-index
```

### GitHub Releases

Download the archive for your platform from
[GitHub Releases](https://github.com/defendend/Claude-ast-index-search/releases),
unpack it, and put the `ast-index` binary somewhere on your `PATH`.

### Verify Installation

```bash
ast-index version
ast-index help
```

## Quick Start

Install `ast-index`, then build an index from the project root:

```bash
cd /path/to/your/project
ast-index rebuild
ast-index stats
ast-index search "UserRepository"
```

The index is stored outside the repository in your user cache directory. It is
not committed and does not modify source files. To see the exact SQLite path:

```bash
ast-index db-path
```

After a successful `rebuild` or `update`, `ast-index` removes caches for other
projects only when they have not been touched for more than 14 days. Open
commands and a running `watch` hold an external lease, so an active index is
never selected by this cleanup. A removed cache is recreated by the next
`ast-index rebuild` in that project.

## Connect It To A Project

For a small or medium project, the default setup is enough:

```bash
cd /path/to/your/project
ast-index rebuild
```

For a large repository, add a project config so `rebuild`, `update`, and
`watch` all scan the same intended paths:

```yaml
# .ast-index.yaml
include:
  - app
  - packages/shared
exclude:
  - vendor
  - generated
```

Then rebuild once:

```bash
ast-index rebuild
```

Use `include` when you only want selected directories from a larger tree. Use
`exclude` for generated or vendored folders that should never enter the index.

## Keeping The Index Fresh

Use three commands for the index lifecycle:

```bash
ast-index rebuild  # full rebuild; use first time or after major tree changes
ast-index update   # incremental update; use after edits, pulls, rebases, checkouts
ast-index watch    # foreground watcher; update automatically on file changes
```

`update` walks the configured project roots, compares files with the database,
indexes new or changed supported source files, and removes deleted files from
the index. It honors `.gitignore`, built-in ignored directories, and
`.ast-index.yaml` `include` / `exclude` settings.

Change detection is timestamp-based. During `rebuild` and `update`,
`ast-index` stores each indexed file's relative path, filesystem modified time
(`mtime`), and size in SQLite. On the next `update`, it walks the current source
tree and compares each file's current `mtime` with the stored one:

- path is missing from the database: index it as a new file;
- current `mtime` is newer than stored `mtime`: re-parse and replace that file's
  symbols and references;
- path exists in the database but is no longer found on disk: delete it from the
  index;
- current `mtime` is the same or older: leave the existing index rows as-is.

`update` does not use `git diff` and does not hash file contents, so it also
works after ordinary file edits, generated file changes, branch checkouts, and
non-git workflows.

`watch` listens for source-file changes and runs the same incremental update
path after a short debounce. Run it in a long-lived terminal while you work:

```bash
cd /path/to/your/project
ast-index watch
```

Or start it in the background from your current shell:

```bash
ast-index watch &
```

Only one watcher is allowed per project. Stop a foreground watcher with
`Ctrl+C`. For a background watcher, use `jobs`, then `fg` + `Ctrl+C` or
`kill %<job-number>`.

## Git Pulls, Rebases, And Checkouts

After changing branches or pulling new code, run:

```bash
ast-index update
```

If `ast-index watch` is already running, file-system events usually keep the
index fresh automatically. A manual `update` is still a good habit after a large
checkout, rebase, or branch switch because it reconciles the full file list with
the database.

Use `rebuild` instead of `update` when:

- the project root or `.ast-index.yaml` changed significantly;
- generated or vendored folders were added or removed;
- the index looks inconsistent after a very large branch switch;
- you want a clean baseline before sharing results with an agent.

## Git Worktrees

Each git worktree has its own directory, and `ast-index` keys the default cache
by the canonical project-root path. That means each worktree gets its own index.

This is intentional: different worktrees can be on different branches, so
sharing one SQLite index between them would make results stale or incorrect.

Recommended workflow:

```bash
cd /path/to/project-main
ast-index rebuild

cd /path/to/project-feature-worktree
ast-index rebuild
```

After the first rebuild in each worktree, use `ast-index update` or
`ast-index watch` inside that worktree as usual.

## Running From Subdirectories

After an index exists, you can run search commands from subdirectories. Results
are scoped to the current subtree:

```bash
cd /path/to/project
ast-index rebuild
ast-index search "Payment"

cd /path/to/project/services/payments
ast-index search "Payment"  # searches only this subtree
```

In monorepos with nested project markers, read commands may stop at the nearest
nested project. If you intentionally built one parent index and want subprojects
to reuse it, opt in with:

```bash
ast-index --walk-up search "Payment"
# or
AST_INDEX_WALK_UP=1 ast-index search "Payment"
```

## AI Agent Instructions

Build the project index first:

```bash
cd /path/to/your/project
ast-index rebuild
```

Then copy this rule into your project's agent instructions file, for example
`AGENTS.md`, `CLAUDE.md`, `.cursor/rules`, or another project-level rules file
used by your agent:

````markdown
# ast-index Rules

All commands: `ast-index <command>`

## Keep Index Up To Date

After `git pull`, `git rebase`, `git checkout`, or `git switch`, run
`ast-index update`.

For active development, keep the watcher running:

```bash
ast-index watch
# or, from the current shell:
ast-index watch &
```

## Mandatory Search Rules

1. **ALWAYS use ast-index FIRST** for any code search task.
2. **NEVER duplicate results** — if ast-index found results, that is the complete answer.
3. **DO NOT run grep** after ast-index returns results.
4. Use Grep only when ast-index returns empty or for regex/string-literal search.

## Mandatory Read Rules

1. **ALWAYS run `ast-index outline <file>` BEFORE `Read`** for any file longer than 500 lines.
2. Use the outline to identify the specific symbol or range you need, then `Read` only that slice with `offset` / `limit`.
3. This rule is mandatory — do not bulk-read large files without an outline first.

## Rules For Subagents

When spawning any agent for code search, ALWAYS include these instructions in
the prompt. Many agent systems do not automatically pass project rules to
subagents.

```text
Use `ast-index` via Bash for code search before grep/Grep:
- search "query" — universal search
- file "Name" — find file
- usages "Name" — find all usages
- implementations "Name" — find implementations
- class "Name" — find definition
- callers "func" — find callers

Use Grep only if ast-index returns empty or when regex/string-literal search is required.

Before using the Read tool on any file longer than 500 lines, first run
`ast-index outline <file>` to get its structure, then Read only the targeted
slice via offset/limit. Never bulk-read large files.
```

## Commands

- **Search:** `search`, `file`, `symbol`, `class` — find files and symbols by name
- **Usages:** `usages`, `callers`, `call-tree`, `refs` — find where symbols are used
- **Hierarchy:** `implementations`, `hierarchy`, `extensions` — class hierarchy
- **Modules:** `module`, `deps`, `dependents`, `api` — module dependencies
- **Files:** `outline`, `imports`, `changed` — file analysis
- **iOS:** `storyboard-usages`, `asset-usages`, `asset-unused` — storyboard/asset search
- **Quality:** `todo`, `deprecated` — find TODOs and deprecated items
- **Index:** `rebuild`, `update`, `watch`, `stats` — index management

## Common Use Cases

- `ast-index usages "PaymentViewController"` — where is this class used?
- `ast-index implementations "PaymentProcessing"` — what implements this protocol?
- `ast-index callers "processPayment"` — what calls this function?
- `ast-index call-tree "processPayment" -d 3` — call hierarchy
- `ast-index deps "PaymentFeature"` — module dependencies
- `ast-index dependents "NetworkKit"` — what depends on this module?
- `ast-index changed` — what changed in my branch?
- `ast-index todo` — find all TODOs
````

## Optional Claude Code Local Automation

If you want Claude Code to ensure an index exists at session start, you can add
a local, uncommitted project setting:

```bash
mkdir -p .claude
printf ".claude/\n" >> .git/info/exclude
```

Example `.claude/settings.json`:

```json
{
  "permissions": {
    "allow": [
      "Bash(ast-index *)"
    ]
  },
  "hooks": {
    "SessionStart": [
      {
        "matcher": "startup",
        "hooks": [
          {
            "type": "command",
            "command": "ast-index stats >/dev/null 2>&1 || ast-index rebuild",
            "timeout": 120,
            "statusMessage": "Checking ast-index..."
          }
        ]
      }
    ]
  }
}
```

This is optional. If your team commits agent configuration, adapt the paths and
permissions to your own policy.

## Common Commands

```bash
ast-index search "Payment"              # broad search across files and symbols
ast-index file "PaymentView"            # find files by name
ast-index symbol "PaymentRepository"    # find a symbol
ast-index class "BaseController"        # find class-like definitions
ast-index usages "PaymentRepository"    # find references
ast-index refs "PaymentRepository"      # definitions + imports + usages
ast-index callers "processPayment"      # find call sites
ast-index implementations "Repository"  # find implementations
ast-index hierarchy "BaseController"    # inheritance tree
ast-index outline src/main.rs           # file structure
ast-index imports src/main.rs           # imports/includes
ast-index changed                       # symbols changed in VCS diff
ast-index map                           # compact project map
ast-index conventions                   # detected frameworks and patterns
```

Use JSON for scripts or agents:

```bash
ast-index --format json search "Payment"
```

## Advanced

Run SQL against the SQLite index:

```bash
ast-index query "
  SELECT s.name, s.kind, f.path, s.line
  FROM symbols s
  JOIN files f ON s.file_id = f.id
  WHERE s.name LIKE '%Controller%'
  ORDER BY f.path, s.line
"
```

Use structural search through ast-grep when `sg` is installed:

```bash
ast-index agrep 'if ($COND) { return $RET; }' --lang typescript
```

Add external source roots when a project depends on local sibling code:

```bash
ast-index add-root /path/to/shared-library
ast-index list-roots
ast-index update
```

## Troubleshooting

**Index not found.** Run `ast-index rebuild` from the intended project root.

**Results are stale.** Run `ast-index update`. If the tree changed heavily,
run `ast-index rebuild`.

**The agent cannot find the index.** Check `ast-index stats` manually in the
same directory where the agent runs commands.

**Search from a subproject ignores the parent index.** Use `--walk-up` or
`AST_INDEX_WALK_UP=1` when you intentionally want a parent index to win.

**The index includes too much.** Add `.ast-index.yaml` with `include` and
`exclude`, then run `ast-index rebuild`.
