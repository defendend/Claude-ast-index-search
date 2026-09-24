# ast-index v3.54.0

Structural, AST-aware code navigation CLI for large, multi-language
repositories. It builds a local SQLite index of symbols, references, imports,
modules, dependencies, and inheritance so humans and agents can move through
code by exact structure instead of grep-style text matches.

https://t.me/defendend_ai_dev

## What It Gives You

- Navigate classes, functions, files, imports, usages, callers, implementations,
  inheritance, modules, and dependency paths.
- Start broad with `explore`, then jump to exact definitions with `symbol`,
  `class`, `outline`, and `refs`.
- Keep the index current with `ast-index update` after the first `rebuild`.
- Give coding agents compact, parseable context instead of raw file dumps.
- Save ~40-50% of agent tokens on large repositories by returning structural
  slices of code instead of whole files.

**Languages:** Kotlin, Java, Swift, Objective-C, TypeScript, JavaScript, Vue,
Svelte, CSS, SCSS, Less, Rust, Zig, C#, Python, Go, C, C++, Scala, PHP, Ruby,
Perl, Dart, Protocol Buffers, WSDL, XSD, BSL (1C:Enterprise), Lua, Bash, Elixir,
SQL, R, Matlab, Groovy, Common Lisp, GDScript. Project type is auto-detected.

## How To

```bash
# Install
brew tap defendend/ast-index
brew install ast-index

# Build an index once per project
cd /path/to/project
ast-index rebuild

# Ask code questions
ast-index explore "payment flow"
ast-index search ViewModel
ast-index class BaseFragment
ast-index usages Repository
ast-index implementations Presenter
ast-index deps app
```

Use `ast-index update` after edits or branch switches. Hooks can queue a
trailing-debounced refresh without losing edits that arrive during an update:

```bash
ast-index update --background --debounce-ms 500
```

Index-reading commands wait (with a bounded timeout) for an already queued
generation, so they do not observe stale results. In monorepos with nested
project markers, add `--walk-up` or `AST_INDEX_WALK_UP=1` to reuse the root
index.

**Guides:** [User guide](https://github.com/defendend/Claude-ast-index-search/blob/main/USER_GUIDE.md)
for everyday workflow;
[command setup guide](https://github.com/defendend/Claude-ast-index-search/blob/main/docs/setup-guide.md)
for install/options/examples;
[CodeGraph comparison](https://github.com/defendend/Claude-ast-index-search/blob/main/docs/comparison.md)
for a dated, source-backed feature comparison.

## Performance

Benchmarks on large Android project (~29k files, ~300k symbols):

| Command | ast-index | grep | Speedup |
|---------|-----------|------|---------|
| imports | 0.3ms | 90ms | **260x** |
| dependents | 2ms | 100ms | **100x** |
| deps | 3ms | 90ms | **90x** |
| class | 1ms | 90ms | **90x** |
| search | 11ms | 280ms | **14x** |
| usages | 8ms | 90ms | **12x** |

## Installation

### Homebrew (macOS/Linux)

```bash
brew tap defendend/ast-index
brew install ast-index
```

### Cargo (crates.io)

Requires a Rust toolchain.

```bash
cargo install ast-index --locked
```

To build the unreleased default branch instead:

```bash
cargo install --locked --git https://github.com/defendend/Claude-ast-index-search ast-index
```

Both build from source. For prebuilt release binaries, use Homebrew, npm, or Winget.

### Winget (Windows)

```shell
winget install --id defendend.ast-index
```

### Migration from kotlin-index

If you have the old `kotlin-index` installed:

```bash
brew uninstall kotlin-index
brew untap defendend/kotlin-index
brew tap defendend/ast-index
brew install ast-index
```

### From source

```bash
git clone https://github.com/defendend/Claude-ast-index-search.git
cd Claude-ast-index-search
cargo build --release
# Binary: target/release/ast-index (~44 MB)
```

### Troubleshooting: Syntax errors on install

If `brew install ast-index` fails with merge conflict errors (`<<<<<<< HEAD`), reset your local tap:

```bash
cd /opt/homebrew/Library/Taps/defendend/homebrew-ast-index
git fetch origin
git reset --hard origin/main
brew install ast-index
```

## Monorepo Workflow

If your repo has subdirectories with their own VCS markers (git submodules,
subtrees, nested `Cargo.toml` / `settings.gradle`), read-commands normally
stop at the nearest marker — they won't reuse a parent-level index even
if one exists. Pass `--walk-up`, or set `AST_INDEX_WALK_UP=1`, to tell
the lookup to prefer any existing parent DB over nested markers:

```bash
# once, in the root
cd /monorepo && ast-index rebuild

# later, from any subproject — reuse the root index
AST_INDEX_WALK_UP=1 ast-index search ViewModel
# or per-call:
ast-index --walk-up search ViewModel
```

This is opt-in by design: silently preferring a far-away parent DB could
surface a stale or misconfigured index from an earlier accidental
`rebuild` higher up. With the flag you explicitly say "trust the parent".

## Worktrees And Intentional Cross-Root Workspaces

Independent git worktrees get independent indexes because their canonical
root paths differ. Rebuild once inside each worktree; do not attach worktrees
to one another.

For source trees that intentionally form one workspace, create the primary
index first, attach a named subtree, then index the attached files:

```bash
cd /path/to/application
ast-index rebuild
ast-index subtree add shared ../shared-library
ast-index update                 # or: ast-index rebuild
ast-index subtree list
```

Use `--subtree shared` to query only that attachment and `--local` to query
only the primary project. The legacy root commands remain compatibility
aliases; new automation should use `subtree add/remove/list`.

## AI Agent Integration

### Claude Code Plugin

```bash
# Option 1: via marketplace
claude plugin marketplace add defendend/Claude-ast-index-search
claude plugin install ast-index

# Option 2: if ast-index is already installed
ast-index install-claude-plugin
```

Restart Claude Code to activate.

Update: `brew upgrade ast-index && claude plugin update ast-index`.
Uninstall: `claude plugin uninstall ast-index`.

The Claude plugin ships `/initialize` as the default setup command. It
auto-detects project stack(s), including KMP and polyglot repos, then writes
`.claude/settings.json` and `.claude/rules/ast-index.md`. Use
`/initialize-android`, `/initialize-ios`, `/initialize-web`, `/initialize-rust`,
`/initialize-csharp`, or `/initialize-ruby` only as manual overrides.

See [`examples/.claude/rules/ast-index.md`](examples/.claude/rules/ast-index.md)
for a template rules file that teaches the agent to use ast-index for
structural navigation, outline before reading large files, and pass the same
instructions to subagents. Adapt before dropping into your project's
`.claude/rules/`.

### Codex Skill / Plugin

Codex can use the shared `ast-index` skill directly. For local development,
symlink or copy the skill directory into Codex's global skills directory:

```bash
mkdir -p ~/.codex/skills
ln -s /absolute/path/to/Claude-ast-index-search/plugin/skills/ast-index ~/.codex/skills/ast-index
```

This repository also includes a Codex plugin manifest at
[`plugin/.codex-plugin/plugin.json`](plugin/.codex-plugin/plugin.json) and a
repo marketplace at [`.agents/plugins/marketplace.json`](.agents/plugins/marketplace.json)
for Codex builds that support plugin marketplaces.

If your Codex build supports plugin marketplaces, restart Codex in this repo
and install `ast-index` from the repo marketplace. For a remote marketplace,
add the repository:

```bash
codex plugin marketplace add defendend/Claude-ast-index-search
```

The Codex package exposes the same `ast-index` skill. Command-style project
setup is kept out of the Codex manifest because Codex uses skills and local
project configuration as first-class components.

### Cursor Skill / Plugin

Cursor can use the shared skill directly:

```bash
mkdir -p ~/.cursor/skills
ln -s /absolute/path/to/Claude-ast-index-search/plugin/skills/ast-index ~/.cursor/skills/ast-index
```

This repository also includes a Cursor plugin manifest at
[`plugin/.cursor-plugin/plugin.json`](plugin/.cursor-plugin/plugin.json) and a
multi-plugin marketplace at [`.cursor-plugin/marketplace.json`](.cursor-plugin/marketplace.json).

For local Cursor testing:

```bash
mkdir -p ~/.cursor/plugins/local
ln -s /absolute/path/to/Claude-ast-index-search/plugin ~/.cursor/plugins/local/ast-index
```

Reload Cursor after creating the symlink. The Cursor plugin package exposes the
shared `ast-index` skill, a project rule in `plugin/rules/`, and a Cursor-specific
`initialize-ast-index` command that writes `.cursor/rules/ast-index.mdc`.

### Gemini CLI

```bash
gemini skills install https://github.com/defendend/Claude-ast-index-search.git --path plugin/skills/ast-index
```

## 💝 Support Development

[![Support on Boosty](https://img.shields.io/badge/Support%20on-Boosty-FF5722?style=for-the-badge&logo=star)](https://boosty.to/ast_index/donate)

---

## Commands (47+)

Run `ast-index rebuild` once per project, then use `ast-index update` to keep
the index fresh.

```bash
ast-index explore <QUERY...>       # One-shot context: ranked source + neighbours + tests (--rwr for graph)
ast-index search <QUERY>           # Universal structural search
ast-index file <PATTERN>           # Find files
ast-index symbol <NAME>            # Find symbols
ast-index class <NAME>             # Find classes/interfaces
ast-index outline <FILE>           # Symbols in file
ast-index imports <FILE>           # Imports in file
ast-index refs <SYMBOL>            # Definitions + imports + usages
ast-index usages <SYMBOL>          # Symbol usages
ast-index callers <FUNCTION>       # Function call sites
ast-index implementations <PARENT> # Find implementations
ast-index hierarchy <CLASS>        # Class hierarchy tree
ast-index changed [--base BRANCH]  # Branch-level changed files (A/M/D/R)
ast-index hotspots [--collect]     # Rank files by Git history (churn, fixes, authors)
ast-index todo [PATTERN]           # TODO/FIXME/HACK comments
ast-index deprecated [QUERY]       # Deprecated items
```

### Paginated JSON schema v2

Limited search commands now report completeness explicitly. `symbol`, `class`,
`implementations`, `usages`, and `callers` use this shape:

```json
{
  "schema_version": 2,
  "items": [],
  "pagination": {
    "total": 0,
    "returned": 0,
    "truncated": false,
    "limit": 50
  }
}
```

`search` and `refs` keep their named result arrays and provide one pagination
object per array under `pagination`. Consumers migrating from bare arrays must
read `items` for single-result-set commands and must check `truncated` before
treating a response as complete. This is limit-based pagination, not a cursor:
rerun with a larger `--limit` when more results are required.

The cache-independent `changed` command retains its separate JSON schema v1;
its schema version did not change with this pagination migration.

### Changed files on the current branch

`changed` asks the detected version-control repository for the files changed from
`merge-base(base, HEAD)` to `HEAD`. Without `--base`, Git resolves
`origin/HEAD`, then tries `origin/main`, `origin/master`, `main`, `master`, and
`trunk`; other supported backends select their conventional mainline. It reads
version-control state directly, so it works without an
ast-index database and does not require `rebuild` or `update`. Results are
scoped to the current working directory, while paths remain
repository-relative. Staged and unstaged working-tree edits are not included.

```bash
# Compact text summary
ast-index changed

# Stable schema v1; the VCS timeout defaults to 30000 ms
ast-index --format json changed --base origin/main --timeout-ms 30000

# Print the detected root, scope, exact VCS argv, and timing to stderr
ast-index changed --verbose
```

Text output uses `A` (added), `M` (modified), `D` (deleted), and `R` (renamed):

```text
Changed files against origin/main (3):
  M  README.md
  R  docs/old-guide.md -> docs/setup-guide.md
  M  docs/generated\nname.md
```

Control characters and backslashes in text paths are escaped, so every change
stays on one output line. Use JSON instead of parsing this human-readable
summary in scripts.

JSON output preserves rename metadata:

```json
{
  "schema_version": 1,
  "vcs": "git",
  "base": "origin/main",
  "head": "HEAD",
  "scope": null,
  "changes": [
    { "status": "M", "path": "README.md" },
    { "status": "R", "path": "docs/setup-guide.md", "old_path": "docs/old-guide.md" }
  ]
}
```

At the repository root, `scope` is `null`; from a nested working directory it
is that repository-relative directory path.

This is a fast file summary for branch review, not a changed-symbol report and
not a replacement for your version-control system's diff command when patch
hunks are needed.

### Git hotspots

Two files can look equally good to copy from and have very different histories:
one absorbed 28 commits in six months with 54% of them bugfixes, the other was
written once and left alone for 86 days. `hotspots` puts that history next to
the code.

Per file, collected from the Git log: commit count, churn (added + deleted
lines, plus churn relative to the file's current size), bugfix share, distinct
author count, age of the first commit, and time since the last change.

Thresholds are **percentiles within this repository**, not constants. 28
commits is a lot for a small library and unremarkable in a monorepo, so
`churn:high` means p90+ *here*. Raw numbers are always printed next to the
label.

```bash
# Collect history into the index, then show the top 20
ast-index hotspots --collect

# Report from already-collected data (no Git subprocess at all)
ast-index hotspots --limit 50 --sort fixes

# Narrow to a directory; percentiles still come from the whole repository
ast-index hotspots --path src/parsers --min-commits 5

# Discard the cursor and rescan the full history
ast-index hotspots --collect --full

# Stable paginated schema v2
ast-index --format json hotspots --limit 10
```

Collection is **never implicit**: `rebuild` and `update` do not run it, so
indexing cost is unchanged for everyone who does not ask for this. The first
`--collect` walks the whole history; every later one reads only what changed.
History is stored per commit, so a branch switch, rebase, force-push or reset
subtracts the commits `HEAD` no longer reaches and adds the new ones, and the
numbers equal a fresh full collection at that `HEAD`. Line counts come from the
working tree: an uncommitted edit counts once a later `--collect` recomputes
that file (a commit or checkout changes it) or on `--full`.

Signals live in their own tables, keyed by project-relative path, so they
also cover files the parsers never look at (fixtures, configs, migrations).
They depend on the repository, not on the index, so `rebuild` carries them
into the new index and the next `--collect` stays incremental. It leaves them
behind, says so, and the next `--collect` reads the history again only when
they were collected by an older version or from another working tree or
scope, or cannot be read back intact.

```text
Git hotspots — 289 file(s) with history, 370 commit(s) analyzed, HEAD 50db069dbb, sorted by score:
  Labels are percentiles within this repository (high = p90+, elevated = p75+).
  src/indexer.rs
    score 90 · commits 70 (p98) · fixes 17/70 = 24% (p72) · churn +8558/-3259 (p99, 2.2x file)
    authors 6 (p99) · age 244d · last change 0d ago · 5298 lines
    churn:high rewritten-often authors:many veteran
```

Labels: `churn:high` / `churn:elevated`, `rewritten-often` (high churn per
current line, only for files of 10+ lines), `fixes:high` / `fixes:elevated`
(only for files with at least four commits, so 1-of-1 is never "100% bugs"),
`authors:many`, `veteran`. `score` is the mean of the commit, churn and
bugfix-ratio percentiles.

Bugfix detection is a heuristic over commit subjects: leading tracker keys
(`[ABC-123]`, `ABC-123:`, `#42`, `[ABC-1][ABC-2]`) are stripped first, then
English and Russian bugfix vocabulary is matched on word boundaries — so
`prefix` is not a fix, `Исправить падение` is, and so is `[HOTFIX][ABC-123]`
(only keys are stripped, not tags like `[HOTFIX]` or `[FIX]`). Merge commits
are excluded. Renames follow the file: history recorded under the old path
moves onto the new one.

`--sort` accepts `score` (default), `commits`, `churn`, `relative-churn`,
`fixes`, `authors`, `recent`. `fixes` orders by the bugfix share discounted
for thin history — the lower bound of its 95% Wilson interval, so 11 fixes in
17 commits rank above 2 in 2 — and lists files with fewer than four commits
after the rest. `--subtree` is rejected: the signals describe the project's own
working tree.

### Module analysis

```bash
ast-index module <PATTERN>         # Find modules
ast-index deps <MODULE>            # Module dependencies
ast-index dependents <MODULE>      # Dependent modules
ast-index unused-deps <MODULE>     # Find unused dependencies (v3.2: +transitive, XML, resources)
ast-index api <MODULE>             # Public API of module
```

#### module-route — dependency path between two modules

Show how module A reaches module B through the dependency graph:

```bash
# Shortest path (default)
ast-index module-route --from core.utils --to features.payments.api

# All simple paths, filtered to api edges only
ast-index module-route --from app --to core.db --all --via-kind api

# JSON output — machine-readable, no ANSI
ast-index module-route --from app --to core.db --format json

# Mermaid diagram (paste into any markdown renderer)
ast-index module-route --from app --to core.db --format mermaid

# Graphviz DOT
ast-index module-route --from app --to core.db --format dot

# Gradle-style module names work too
ast-index module-route --from :app --to :core:utils
```

Options:
- `--all` — return all simple paths instead of the single shortest
- `--via-kind <api|implementation|all>` — filter traversal to one edge kind (default: `all`)
- `--max-paths <N>` — cap on returned paths when `--all` is set (default: 50)
- `--max-depth <N>` — cap on path length in hops (default: 20)
- `--timeout-ms <N>` — wall-clock guard in milliseconds (default: 5000)

### XML & Resource analysis

```bash
ast-index xml-usages <CLASS>       # Find class usages in XML layouts
ast-index resource-usages <RES>    # Find resource usages (@drawable/ic_name, R.string.x)
ast-index resource-usages --unused --module <MODULE>  # Find unused resources
```

### iOS-specific commands

```bash
ast-index storyboard-usages <CLASS>  # Class usages in storyboards/xibs
ast-index asset-usages [ASSET]       # iOS asset usages (xcassets)
ast-index asset-usages --unused --module <MODULE>  # Find unused assets
ast-index swiftui [QUERY]            # @State/@Binding/@Published props
ast-index async-funcs [QUERY]        # Swift async functions
ast-index publishers [QUERY]         # Combine publishers
ast-index main-actor [QUERY]         # @MainActor usages
```

### Perl-specific commands

```bash
ast-index perl-exports [QUERY]       # Find @EXPORT/@EXPORT_OK
ast-index perl-subs [QUERY]          # Find subroutines
ast-index perl-pod [QUERY]           # Find POD documentation (=head1, =item, etc.)
ast-index perl-tests [QUERY]         # Find Test::More assertions (ok, is, like, etc.)
ast-index perl-imports [QUERY]       # Find use/require statements
```

### Index management

```bash
ast-index init                     # Initialize DB
ast-index rebuild [--type TYPE]    # Full reindex
ast-index update                   # Incremental update
ast-index stats                    # Index statistics
ast-index version                  # Version info
```

## Language-Specific Features

### TypeScript/JavaScript (new in v3.9)

Supported elements:
- Classes, interfaces, type aliases, enums
- Class methods (constructor, getters/setters, static, async)
- Class fields/properties, private `#members`, abstract methods
- Functions (regular, arrow, async)
- React components and hooks (`useXxx`)
- Vue SFC (`<script>` extraction)
- Svelte components
- Decorators (@Controller, @Injectable, etc.)
- Namespaces, constants, imports/exports

```bash
ast-index class "Component"        # Find React/Vue components
ast-index search "use"             # Find React hooks
ast-index search "@Controller"     # Find NestJS controllers
ast-index class "Props"            # Find prop interfaces
```

### Rust (new in v3.9)

Supported elements:
- Structs, enums, traits
- Impl blocks (`impl Trait for Type`)
- Functions, macros (`macro_rules!`)
- Type aliases, constants, statics
- Modules, use statements
- Derive attributes

```bash
ast-index class "Service"          # Find structs
ast-index class "Repository"       # Find traits
ast-index search "impl"            # Find impl blocks
ast-index search "macro_rules"     # Find macros
```

### Ruby (new in v3.9)

Supported elements:
- Classes, modules
- Methods (def, def self.)
- RSpec DSL (describe, it, let)
- Rails patterns (has_many, validates, scope, callbacks)
- Require statements, include/extend

```bash
ast-index class "Controller"       # Find controllers
ast-index search "has_many"        # Find associations
ast-index search "describe"        # Find RSpec tests
ast-index search "scope"           # Find scopes
```

### C# (new in v3.9)

Supported elements:
- Classes, interfaces, structs, records
- Enums, delegates, events
- Methods, properties, fields
- ASP.NET attributes (@ApiController, @HttpGet, etc.)
- Unity attributes (@SerializeField)
- Namespaces, using statements

```bash
ast-index class "Controller"       # Find ASP.NET controllers
ast-index class "IRepository"      # Find interfaces
ast-index search "[HttpGet]"       # Find API endpoints
ast-index search "MonoBehaviour"   # Find Unity scripts
```

### Dart/Flutter (new in v3.10)

Supported elements:
- Classes with Dart 3 modifiers (abstract, sealed, final, base, interface, mixin class)
- Mixins, extensions, extension types
- Enhanced enums with implements/with
- Functions, constructors, factory constructors
- Getters/setters, typedefs, properties
- Imports/exports

```bash
ast-index class "Widget"           # Find widget classes
ast-index class "Provider"         # Find providers
ast-index search "mixin"           # Find mixins
ast-index implementations "State"  # Find State implementations
ast-index outline "main.dart"      # Show file structure
ast-index imports "app.dart"       # Show imports
```

### Python

```bash
ast-index class "ClassName"        # Find Python classes
ast-index symbol "function"        # Find functions
ast-index outline "file.py"        # Show file structure
ast-index imports "file.py"        # Show imports
```

### Go

```bash
ast-index class "StructName"       # Find structs/interfaces
ast-index symbol "FuncName"        # Find functions
ast-index outline "file.go"        # Show file structure
ast-index imports "file.go"        # Show imports
```

## Configuration File

Create `.ast-index.yaml` in your project root to configure ast-index:

```yaml
# Additional directories to index
roots:
  - "../shared-lib"
  - "../common-modules"

# Directories to exclude from indexing
exclude:
  - "vendor"
  - "build"
  - "node_modules"

# Include files ignored by .gitignore
no_ignore: false
```

All fields are optional. CLI flags override config file values.

### Examples

**Monorepo with shared libraries:**
```yaml
roots:
  - "../core"
  - "../network"
```

**Project with generated code to skip:**
```yaml
exclude:
  - "generated"
  - "proto/gen"
```

## Changelog

### Unreleased

- **`search`, `callers` and `call-tree` skip files that cannot match** — the
  index now keeps each file's distinct words (`file_words`, about +7% index
  size), and the grep-based commands no longer open a file whose words lack
  the searched literal while it is unchanged since indexing; edited and new
  files are still searched. On a 40k-file Rails monorepo `search` went from
  0.67 to 0.33 s, `callers merge` from 0.60 to 0.26 s, `call-tree --depth 3`
  from 1.13 to 0.66 s, with the same results. An index built by an older
  version is searched in full until its next `rebuild`.
- **`callers` checks for definitions cheaply and in parallel** — the "is this
  line a definition?" test ran a capturing regex on one thread for every call
  line; a capture-free pre-check now runs on the search threads.
- **`update` finds changes 3–4× faster** — the project root was canonicalized
  once per file (a thread and a `realpath` each), and files were stat'ed and
  `node_modules` packages walked one by one; now once per walk and in
  parallel. A no-op `update` on a 40k-file project: 2.1 → 0.6 s; 20 changed
  files: 1.8 → 0.4 s.
- **`rebuild` is about 40% faster** — signatures looked up their source line
  by scanning the file from the top for every symbol (quadratic in file
  length) and copied the whole line before truncating it; Ruby and TypeScript
  files were parsed twice (symbols, then references); and parse threads sat
  idle while each chunk was written. Lines are now indexed once per file,
  signatures capped before copying, each file parsed once, and writing
  overlaps parsing in the same order, so the index is byte-for-byte the same:
  9.3 → 5.5 s on a 40k-file monorepo.
- **`graph build` 2.5× faster** — references are resolved per file in parallel
  and import matches memoized, with an identical graph: 4.2 → 1.7 s.
- **`hotspots --collect` reads history 5× faster** — `git log` windows run in
  parallel (up to 8), and a window of old bulk-import commits no longer
  overflows the 16 MB output cap and gets re-read at half size again and
  again: a full collection of a 25k-commit history took 50 s, now 10 s, with
  the same stored history.
- **The MCP server answers tool calls concurrently** — several calls sent at
  once used to wait for each other; responses may now arrive out of order,
  matched by `id` as JSON-RPC allows.
- **Minified JavaScript and CSS are left out** — `.js` / `.mjs` / `.cjs` /
  `.css` files named `*.min.*` or `*-min.*`, or whose first 64 KiB is minifier
  output (lines of 1000+ bytes on average, 100+ of them outside string
  literals), are no longer indexed, read by the grep-based commands (`search`
  contents, `callers`, `call-tree`, `todo`, …) or parsed by `outline` /
  `imports`, which now say the file was skipped. A one-line stylesheet gave
  each of its thousands of selectors the whole file as a signature: on a Rails
  monorepo with a 589 KB `app.min.css`, `outline` of that file took 4.6 GB and
  `rebuild` peaked at 4–7 GB; now 11 MB and 0.21 GB. `callers` / `call-tree`
  no longer answer from bundles (`__webpack_require__`). Source with a few long
  strings, SVG paths or data URIs is unaffected, and TypeScript, JSX, SCSS and
  `.d.ts` are never judged. `update` drops minified files an older index kept,
  without a rebuild. `AST_INDEX_SKIP_MINIFIED=0` turns the filter off.
- **`rebuild` keeps the collected Git history** — the per-commit store, the
  per-file signals and the collection cursor are copied into the new index
  instead of being dropped, so `hotspots` reports right after a rebuild and the
  next `--collect` stays incremental: on a 25k-commit monorepo 0.2 s instead of
  a full rescan of a minute or more, for about 0.4 s added to the rebuild.
  History collected by an older version, from another working tree or scope,
  or that cannot be read back intact is left behind with a note, and the next
  `--collect` reads it again. `--sub-projects` / `--include` rebuilds lost it
  too and now keep it.
- **snake_case calls are references in every language** — the generic
  reference extractor did not allow `_` in a called name, so `usages`, `refs`
  and the graph missed `update_profile(user)` and `self._compute()` in Python,
  Rust, Go, C/C++, PHP, Lua, TypeScript and every other language using it
  (`usages open_db` on a Rust codebase went from 0 to 50 hits). Reserved words
  (`sizeof (x)`, `#if defined(X)`, Go `func (r *T)`, Rust `pub(crate)`,
  Python `None`) are no longer recorded as references. In Ruby, a lowercase
  `name(` inside a comment, string or heredoc is no longer a reference.
- **BSL and stylesheets use their own reference extractors again** — the
  trait default sent every language to the generic ASCII extractor, so BSL
  (1C:Enterprise / OneScript) files had no Cyrillic references at all and
  CSS, SCSS and Less recorded kebab-case fragments. BSL now records Cyrillic
  calls, modules before `.` and types after `Новый`, with the source line as
  context; stylesheets record none.
- **One test-path rule** for `explore`, the symbol graph and `--exclude-tests`
  (`graph top`, `hotspots`, `search --rank`): adds `test_*.py`, `conftest.py`
  and `FooTest`/`FooTests`/`FooSpec` files in JVM/Swift/C#/PHP/C++, and stops
  treating Ruby namespaces under `app/`/`lib/` (e.g. `app/jobs/tests/`) and
  PascalCase JS component folders (`components/Test/`) as tests.
- **First open of a project no longer slows down quadratically with the
  cache** — the cache-migration scan now lists the lease directory once
  instead of once per cached project (a first `rebuild` beside 5000 cached
  projects dropped from about 70 s to about 1.4 s).
- **Cache GC removes lease lock files of caches that no longer exist** —
  `.leases` kept a `{key}.lock` and `{key}.publish.lock` for every cache ever
  opened, so it grew without bound. The cleanup after `rebuild` / `update` now
  deletes them whatever their age, and only under the cache-layout lock after
  locking both files itself, so a lock another process holds or is about to
  take is never split across two files. Every command that resolves the index
  gets faster on a cache that had accumulated them.
- **Symbol ranges for every tree-sitter language** — `end_line` used to be
  filled only for Ruby and TypeScript/JavaScript, so `call-tree`,
  `explore --rwr` and `graph` fell back to "the last definition above the
  line" everywhere else. Python, Go, Rust, Java, Kotlin, Swift, C#, C/C++, PHP,
  Scala, Dart, Lua, Elixir, Zig, Objective-C, Groovy, Bash, SQL, R, MATLAB,
  GDScript, Common Lisp, BSL and Protobuf now store the last line of every
  definition, with classes, modules and namespaces enclosing their members.
  The fallback had blamed the wrong definition for 3.6% of references in a Rust
  codebase, 11% in a Go checkout and 18–42% in C++; module-level code no longer
  gets an invented caller. Decorators and annotations stay outside the
  definition they decorate, since they run in the enclosing scope. The graph
  treats Rust `impl` blocks and Swift/Objective-C extensions as the namespace of
  the type they extend, so `Type::new(…)` and `Self::helper(…)` resolve. CSS,
  SCSS, Less and the regex-based parsers still report no ranges. Run
  `ast-index rebuild` to fill ranges in an existing index.
- **Rails schema tables and columns** — `db/schema.rb` (indexed even when
  gitignored) yields `table` and `column` symbols (`users.email`;
  `search email -t column`, `outline db/schema.rb`). `graph build` matches
  tables to models by Active Record's rules — `self.table_name`, single-table
  inheritance, nested models, `table_name_prefix` / `isolate_namespace`, the
  pluralized class name — reports tables without a model and models without a
  table, and resolves column readers and attribute methods called inside a
  model (`email`, `self.email`, `email?`, `saved_change_to_email?`) as scoped
  edges to the column (`graph dependents users.email`).
- **Ruby calls without parentheses are references** — `usages` and the graph
  now see `recv.name`, `name arg`, snake_case `name(...)` and bare `name` calls
  that are not local variables; core collection and string methods called
  without parentheses are skipped. RSpec `let` / `subject` helpers resolve
  within their spec file. On a Rails monorepo resolved graph edges grew from
  66k to 186k at +29% index size.
- **Compound Ruby constants are indexed** — `Billing::Import = Container.injector`
  and CamelCase assignments (`Types = Dry.Types()`) become constants, named
  with their enclosing scopes like classes. In the graph a constant defined in
  an inner scope shadows an outer module of the same name, so
  `include Import[...]` inside `module Billing` no longer links every such
  class to an unrelated top-level `module Import`.
- **Production code never resolves into test trees** — a spec helper that
  reopens a class to stub a method is no longer the target of production calls.
- **One definition of third-party code** — only `node_modules` is left out of
  the graph; a project's `vendor/` directory and its own `.d.ts` files are
  graph nodes, while search ranking still demotes every `.d.ts`.
- **`hotspots --collect` survives branch switches, rebases and resets** —
  history is kept per commit, so moving HEAD subtracts the commits it no longer
  reaches and adds new ones, reusing diffs already read. A switch on a
  25k-commit monorepo takes under a second instead of a ~1-minute rescan, and
  the numbers equal a fresh full collection. The git tables are ~40% smaller.
  The first `--collect` after upgrading recollects once.
- **Hotspot scores no longer collapse into ties** — `hotspots --sort score` and
  `search --rank hotspots|risky|proven` order by unrounded percentiles
  (`score_exact` in JSON).
- **`--exclude-tests` for `hotspots` and ranked search** — `hotspots
  --exclude-tests` and `search --rank … --exclude-tests` (MCP: `exclude_tests`)
  hide spec/test files; percentiles still rank every file.
- **`rewritten-often` only for files of 10+ lines** — relative churn is
  computed only for such files, so one-line bundles, fixtures and gutted files
  no longer read as "3000x file".
- **Namespaced definitions lead `search`** — `search MergeService` had no exact
  hit for `class A::B::MergeService`, and bm25 put the shorter
  `describe "A::B::MergeService"` spec above the class. Names whose last `::`
  segment equals the query (case-sensitive) now rank right below exact names;
  statements such as `include A::B::MergeService` do not count. `search --rank`
  uses the same tier (`exact_last_segment`).
- **Owning-symbol lookups stay within the file's root** — with an attached
  subtree holding a file under the same relative path, `call-tree` and
  `explore --rwr` could attribute a call to a method of the other root.
- **Drop the redundant `idx_symbols_file` index** — it is the leftmost prefix
  of `idx_symbols_file_line_end`; new databases no longer create it and
  existing ones drop it on open. `restore` accepts backups with and without it;
  a backup made by this version cannot be restored by older releases.
- **MCP `search` keeps its explore fallback** — a multi-word query without
  literal matches now renders `fallback: explore — <reason>` followed by
  source, ranked symbols, graph neighbours and tests instead of an empty
  `Files:` heading.
- **`callers` and `call-tree` find bare Ruby predicate and bang calls** —
  `if next_page? && …` and `save!` without receiver or parentheses were
  missed; definitions, `#name?` in documentation and `!=`/`!~` are still
  skipped.
- **Tests no longer write to the user's index cache** — `.cargo/config.toml`
  points `AST_INDEX_CACHE_DIR` at `target/ast-index-cache` for everything cargo
  starts; an explicitly set variable still wins.
- **Name a wrapped default export after what it wraps** —
  `export default injectIntl(Header)`, `memo(Button)` or
  `connect(mapState)(Page)` was indexed under the wrapper's name, so every file
  applying `injectIntl` claimed a definition of it. Such an export is now
  indexed as `default(Header)`, like `export default Header`. A wrapped inline
  function or class is indexed like an anonymous default export, under the
  module name. A call that builds a value from its configuration
  (`createRouter({ … })`) keeps the callee's name.
- **Name an `index` default export after its package, not its build
  directory** — `node_modules/pkg/dist/index.d.ts` was named `dist`; the name
  now comes from the nearest directory above build and source directories.
- **`imports` lists every TypeScript/JavaScript import** — files were read
  with a line pattern meant for Kotlin, so a multi-line `import {` printed as a
  bare `{`, re-exports were not listed and a barrel `index.ts` reported
  "No imports found". Imports and re-exports are now read from the syntax
  tree, one declaration per line. Import symbols in the index stay limited to
  project-local specifiers.
- **`search --type` no longer takes a minute on a large index** — with a kind
  filter the bundled SQLite drove the query from the index on `kind`, scanning
  every symbol of that kind and re-running the full-text match for each one:
  `search Job --type class` took about a minute on a 320k-symbol index. The
  full-text match now leads and the kind filters its hits, about a second, with
  the same results.
- **MCP exposes the symbol graph, Git hotspots and ranked search** — seven new
  tools: `graph_dependents` (direct dependents, or the transitive blast radius
  with `depth` ≥ 2), `graph_dependencies`, `graph_path`, `graph_metrics` (the
  most central symbols, or metrics of given ones), `graph_cycles`,
  `graph_build` and `hotspots`; `search` takes `rank` (`proven`, `hotspots`,
  `risky`, `central`). Descriptions tell the agent when to reach for each one —
  before changing a symbol, when choosing which match to copy — and what has to
  be collected first; graph queries accept `refresh: true` to build a missing or
  stale graph in place. History collection stays a CLI command, since its first
  run outlasts common MCP client timeouts, and the tools name it when history is
  missing. Output is compact text: a ranked search prints each file's history
  and each symbol's graph numbers once, about a quarter of the JSON size.
- **`search --rank` — re-rank results by history and structure** — four
  presets re-order the Files and Symbols sections of a search: `proven` (calm,
  old, idle and actually used — safe to copy), `hotspots` (the file's hotspot
  score), `risky` (transitive dependents × hotspot score) and `central`
  (PageRank). Every result carries its dossier: the score and its terms, raw
  history and graph numbers with repository percentiles and labels, and the
  relevance position it came from. Relevance stays in charge: exact name
  matches stay above partial ones, the pool is the head of the plain order, and
  the preset only weighs in inside a tier. History is labelled as per-file,
  third-party code is never scored and goes last, and a preset whose data is
  missing is not applied — the output names `hotspots --collect` or
  `graph build` instead of ranking by zeros. Formulas were chosen by
  backtesting next-year bugfixes on a 40k-file monorepo.
- **`graph` — a symbol dependency graph** — `graph build` resolves indexed
  references into symbol-to-symbol edges and stores them with per-edge
  resolution confidence (`local`, `scoped`, `import`, `unique`, `ambiguous`).
  Queries on top of it: `dependents`, `dependencies`, `impact` (transitive
  dependents), `path`, `cycles`, `top` and `metrics` (fan-in, fan-out,
  PageRank). Ruby references resolve through namespaces, lexical nesting,
  constant receivers, inheritance and mixins; JavaScript/TypeScript references
  through the module's own imports. Metrics count resolved edges only and
  report ambiguous ones separately, so a name defined in hundreds of places
  does not inflate a symbol's centrality. The graph is opt-in: `rebuild` and
  `update` never build it, and queries flag a stale graph after the index
  changes and accept `--refresh`.
- **`call-tree` prints the same tree on every run** — which callers made it
  under `--limit` depended on the files the parallel scan happened to reach
  first and on hash-map iteration order, so repeating a query could print a
  different tree. Call lines are now taken in path order and attributed in
  that order; files are still searched in parallel. Definition lines and files
  outside `--in-file` no longer use up a function's match budget, so every
  Sidekiq worker defining `perform` no longer crowds out the calls, and
  `--in-file` no longer prints an empty tree while matching calls exist. The
  repository is walked once per command rather than once per level: deep
  trees got faster, while a single frequent name at `--depth 1` now pays for
  the full walk up front.
- **`call-tree` stops expanding callers nothing can call** — a call inside an
  RSpec block or a Rails DSL call is owned by a symbol named after that block
  (`it "does nothing"`, `let(:fields)`, `attributes :id`). Such an owner is
  still printed as a caller, but its name never occurs in code as a call, so
  looking up its callers cost a scan of the whole repository and never found
  any. Only callers whose name is an identifier — `Foo::Bar`, `save!`,
  `valid?` and `name=` included — are expanded now.
- **`call-tree` scans once per level** — every function in the tree took a
  scan of the whole repository to find its callers, so a level with nine
  callers meant nine scans. Since calls resolve to the symbol that really
  contains them, levels are wider and `call-tree` had slowed down several
  times over. The functions a level needs are now looked up together in one
  scan that still gives each of them its own match budget, so the printed tree
  is the same and a level costs about one scan.
- **Calls behind a keyword are no longer taken for definitions** — `callers`
  and `call-tree` skip definition lines by reading `Type name(` as a
  declaration, and a keyword in the type position fooled it: `return foo(`,
  `await foo(`, `new Foo(`, `if foo(`, `export default connect(` and the like
  were dropped as if they declared `foo`. Such lines now count as calls; real
  typed declarations are still skipped.
- **Index anonymous `export default` functions and classes** —
  `export default () => {}`, `export default function () {}` and
  `export default class {}` produced no symbol, so the function could not be
  found and every call inside it had no owner in `call-tree` and
  `explore --rwr`. Such a value is now indexed with its range under the name it
  is imported by: the file name (`hooks/useMap.js` → `useMap`), or the
  directory for an `index` file (`Button/index.jsx` → `Button`). A function is
  classified like a declaration of that name, so a PascalCase component indexes
  as a class; `outline` shows the same name. Named default exports are
  unchanged.
- **Rank project code above dependencies in `search`** — within a ranking
  tier, hits in `node_modules` or `.d.ts` files were ordered against project
  hits by path alone, so `node_modules/…` came ahead of `spec/` or `system/`.
  Project hits now lead within each tier. The tier still decides first, so a
  library's exact `useState` stays above a project's partial `useStateModal`.
  A project-owned `vendor/` directory still counts as project code; `explore`
  shares the same definition.
- **Rank symbol searches by relevance** — FTS symbol queries used to come back
  in whatever order the engine produced, or sorted by name length, so a search
  for `ApplicationService` listed every shorter sibling and never the class
  itself. Matches are now ordered by exact name first (case-sensitive, then
  case-insensitive), then by `bm25()` weighted towards the symbol name over its
  signature, then by name length and finally by path and line, so repeated runs
  return the same order. The candidate pool `explore` builds keeps its previous
  unranked order — ranking it narrowed the pool to same-named symbols and made
  its own scoring worse.
- **Attribute a call site to the function that really contains it** —
  `call-tree` and `explore --rwr` used to blame the nearest definition line
  above a reference, so a module-level call landed on the last method of the
  file and an `include` or a constant could be reported as the caller. Both
  now ask the index which symbol's line range encloses the reference and take
  the innermost one. Languages whose parsers report no range keep the previous
  behaviour.
- **`hotspots` — per-file Git history signals** — a new command ranks files by
  what their history says about them: commit count, churn (absolute and
  relative to the file's current size), bugfix share, distinct authors, age
  and time since the last change. Thresholds are percentiles *within the
  repository being indexed*, not absolute constants, so "high churn" means
  something in a 300-file library and in a 30 000-file monorepo alike; raw
  numbers are printed next to every label. Text and `--format json` output,
  paginated JSON schema v2.
- **Collection is opt-in** — `rebuild` and `update` never walk the log;
  `hotspots --collect` does.

### 3.54.0

- **Tuist projects get a module graph** — targets declared in `Project.swift`
  (`.target(...)` or any project helper such as `.spmSwiftFolderTarget(name: .Foo, ...)`)
  are indexed as modules with their `dependencies:` (`.target`, `.external`,
  `.project`, `.product`), so `module`, `deps`, `dependents`, `module-route`
  and `unused-deps` work on Tuist workspaces. Previously such a workspace had
  no module dependencies at all.
- **SwiftPM modules point at their sources and have dependencies** — targets
  resolve to `Sources/<Target>` / `Tests/<Target>` (or `path:` / `sources:`)
  instead of `<package>/<Target>`, `let targets = [...]` declarations are read,
  and target dependencies are indexed. Swift modules are named after the
  target — the `import` name — and only fall back to a package-qualified name
  when two manifests declare the same target. Manifests are parsed with
  tree-sitter.
- **`unused-deps` understands Swift imports** — an `import Dep` in any file of
  the module marks the dependency as used. Swift `import` declarations are now
  indexed.
- **`api` lists Swift public API** — declarations marked `public` / `open`.
- **Swift local variables are no longer indexed as properties** — `let` / `var`
  inside function bodies, closures and accessors are skipped, matching Kotlin.
- **Module-qualified supertypes resolve** — `class A : ru.pkg.Base()`,
  `extension Foo: Module.Proto` are recorded with the simple parent name, so
  `hierarchy` and `implementations` find them (previously the parent became
  `ru` / `Module`).
- **`inject` finds constructor injection** — parameters of
  `@Inject constructor(...)` and Java `@Inject` constructors, plus fields whose
  annotation is on its own line.
- **`--limit` with a filter no longer under-reports** — `flows`, `suspend`,
  `deprecated`, `suppress`, `deeplinks` applied the query after the limit was
  spent (e.g. `flows --limit 5` could return 1). `suspend` reports the function
  name instead of an extension receiver.
- **`provides` is an order of magnitude faster** — scans only files that
  declare `@Provides` / `@Binds`.
- **`unused-symbols --module` accepts a module name** — `features.surge.impl`
  or `:core:utils`, not just a path prefix.
- **`rebuild --type modules|deps|files` keeps the rest of the index** — a
  partial rebuild used to publish an index without files and symbols.

### 3.53.0

- **`search` no longer returns nothing for a multi-word query** — when a
  query with two or more terms has no literal match, `search` now hands it to
  the `explore` ranking engine and prints relevance-ranked symbols with their
  source, honouring `--module` / `--in-file`. Text output is labelled
  `No literal matches …`; JSON stays a single document and carries
  `"fallback": "explore"` plus a `reason`. Single-term queries keep exact
  literal semantics.
- **`callers` walks attached subtrees** — call sites inside a subtree added
  with `subtree add` are found and shown as `[name] /abs/path`, matching
  `usages` and `refs`. `--subtree NAME` restricts to that subtree, `--local`
  to the primary root. Previously both flags were accepted and ignored.
- **`explore` accepts scope** — `--module` / `--in-file` narrow the candidate
  set before ranking.
- **Agent guidance points intent queries at `explore`** — the skill and the
  MCP `search` description now say to use `explore` for a question or a
  description and `search` for a known identifier.

### 3.52.0

- **Index C++ functions that return a pointer or a reference** — declarations
  such as `Grid* GetGrid()`, `Player& FindPlayer()`, and
  `const Grid* const GetEmptyPhaseShift()` are now indexed instead of being
  skipped.
- **Install from crates.io** — the installation guide now documents
  `cargo install ast-index --locked` as the released channel, with the Git
  build kept as the way to install the unreleased default branch.

### 3.51.0

- **Report truncated search results instead of hiding them** — `search`,
  `symbol`, `class`, `implementations`, `refs`, `usages`, and `callers` now
  print `showing N of M` with a `--limit` hint, and emit paginated JSON
  schema v2 with `total`, `returned`, `truncated`, and `limit`.
- **Keep the index fresh without blocking edits** — `update --background
  --debounce-ms <ms>` queues a coordinated, trailing-debounced generation and
  returns immediately; index-reading commands wait for a queued generation
  instead of answering from a stale index.
- **Scope watcher detection to the project** — the new `watch-status` command
  reports whether this project has an active watcher, so a watcher in one
  repository no longer suppresses updates in another. Session-start and
  post-edit hooks and the generated Git hooks use it, and the session-start
  hook now runs asynchronously with a visible status message.
- **Index Kotlin files containing `suspend { }` lambdas** — a suspend-lambda
  syntax error no longer drops the enclosing interface, nested classes, and
  later declarations from the index, and local `val` bindings are no longer
  published as properties.
- **Count Kotlin references accurately** — references in a symbol's own
  declaring file are kept when external references exist, and matches inside
  string literals, comments, and KDoc are excluded while executable string
  interpolation is still indexed.
- **Resolve kebab-case Gradle type-safe project accessors** — `deps`,
  `dependents`, `unused-deps`, and `module-route` now link
  `projects.core.designIcon` to `:core:design-icon`, and report ambiguity
  instead of picking an arbitrary module.
- **Rebuild on Windows** — the staged index is synced through a
  write-capable handle, fixing `failed to sync index file … (os error 5)`.
  Concurrent runs are also recognized as ordinary lock contention instead of
  failing with `failed to acquire index publication lock … (os error 33)`.
- **Detect nested project stacks** — `detect-stacks` performs a bounded
  recursive marker scan and recognizes standard nested Kotlin Multiplatform
  layouts such as `composeApp/src/commonMain`.
- **Document worktrees and cross-root workspaces** — independent git
  worktrees keep independent indexes; intentional workspaces are attached
  with `rebuild`, then `subtree add`, then `update`.
- **Compare ast-index and CodeGraph** — `docs/comparison.md` adds a dated,
  source-backed feature comparison with pinned snapshots of both projects.

### 3.50.0

- **Review branch changes quickly without building an index** — use `changed`
  from the CLI or MCP to read cache-independent branch changes with
  added, modified, deleted, and renamed files, rename metadata,
  working-directory scope, a bounded VCS timeout, Git base auto-detection, and
  stable JSON schema v1.
- **Migrate `changed` consumers to the file-level contract** — text output now
  prints an A/M/D/R file summary instead of regex-derived declaration
  pseudo-symbols. Scripts should request `--format json` and read
  `changes[].status`, `changes[].path`, and `changes[].old_path` for renames.
  Library callers can use the deprecated Rust compatibility wrappers while
  migrating to the new API.

See [CHANGELOG.md](CHANGELOG.md) for earlier releases.
