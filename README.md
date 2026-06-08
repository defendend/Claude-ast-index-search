# ast-index v3.46.1

Fast code search CLI for 34 programming languages. Native Rust implementation.

## Supported Projects

| Platform | Languages | File Extensions |
|----------|-----------|-----------------|
| Android | Kotlin, Java | `.kt`, `.java` |
| iOS | Swift, Objective-C | `.swift`, `.m`, `.h` |
| Web/Frontend | TypeScript, JavaScript | `.ts`, `.tsx`, `.mts`, `.js`, `.jsx`, `.mjs`, `.cjs`, `.vue`, `.svelte` |
| Web/Frontend | CSS, SCSS, Less | `.css`, `.pcss`, `.postcss`, `.scss`, `.less` |
| Systems | Rust | `.rs` |
| Systems | Zig | `.zig`, `.zon` |
| Backend | C#, Python, Go, C++, Scala | `.cs`, `.py`, `.go`, `.cpp`, `.cc`, `.c`, `.hpp`, `.scala`, `.sc` |
| Backend | PHP | `.php`, `.phtml` |
| Scripting | Ruby, Perl | `.rb`, `.pm`, `.pl`, `.t` |
| Mobile | Dart/Flutter | `.dart` |
| Schema | Protocol Buffers, WSDL/XSD | `.proto`, `.wsdl`, `.xsd` |
| Enterprise | BSL (1C:Enterprise) | `.bsl`, `.os` |
| Scripting | Lua, Bash | `.lua`, `.sh`, `.bash`, `.zsh` |
| Functional | Elixir | `.ex`, `.exs` |
| Data | SQL, R | `.sql`, `.r`, `.R` |
| Scientific | Matlab | `.m` |
| JVM | Groovy | `.groovy`, `.gradle` |
| Functional | Common Lisp | `.lisp`, `.lsp`, `.cl`, `.asd` |
| Game | GDScript (Godot) | `.gd` |

Project type is auto-detected.

**New to ast-index? Start with the [User guide](USER_GUIDE.md).** It covers
project setup, index updates, git checkouts, worktrees, and AI agent usage.

**[Command setup guide](docs/setup-guide.md)** — install, commands, and usage examples.

## Installation

### Homebrew (macOS/Linux)

```bash
brew tap defendend/ast-index
brew install ast-index
```

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

## Quick Start

```bash
cd /path/to/project

# Build index
ast-index rebuild

# Search
ast-index search ViewModel
ast-index class BaseFragment
ast-index implementations Presenter
ast-index usages Repository
```

### Monorepo workflow

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

## AI Agent Integration

### Claude Code Plugin

```bash
# Option 1: via marketplace
claude plugin marketplace add defendend/Claude-ast-index-search
claude plugin install ast-index

# Option 2: if ast-index is already installed
ast-index install-claude-plugin
```

Restart Claude Code to activate. Update: `brew upgrade ast-index && claude plugin update ast-index`. Uninstall: `claude plugin uninstall ast-index`.

The Claude plugin ships `/initialize` as the default setup command. It
auto-detects project stack(s), including KMP and polyglot repos, then writes
`.claude/settings.json` and `.claude/rules/ast-index.md`. Use
`/initialize-android`, `/initialize-ios`, `/initialize-web`, `/initialize-rust`,
`/initialize-csharp`, or `/initialize-ruby` only as manual overrides.

See [`examples/.claude/rules/ast-index.md`](examples/.claude/rules/ast-index.md) for a template rules file that teaches the agent to prefer ast-index over grep, outline before reading large files, and pass the same instructions to subagents. Adapt before dropping into your project's `.claude/rules/`.

### Codex

```bash
cd /path/to/project
ast-index rebuild
ast-index install-codex-mcp
```

`install-codex-mcp` registers `ast-index-mcp` with Codex via
`codex mcp add`, sets `AST_INDEX_ROOT` to the current project, and sets
`AST_INDEX_BIN` to the current `ast-index` binary. It expects
`ast-index-mcp` next to `ast-index` or on `PATH`. Use
`ast-index install-codex-mcp --dry-run` to print the command and
`~/.codex/config.toml` fallback without changing Codex config.

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
setup is kept out of the Codex manifest because Codex uses skills, MCP config,
apps, and hooks as first-class components.

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

### MCP server (Cursor, Codex, Cline, Continue, OpenCode, Windsurf, …)

An MCP server that exposes ast-index tools to any MCP-compatible agent. Each
tool call spawns `ast-index <subcommand>`, parses the output, and returns a
compact TOON-inspired text blob (≈2-3× fewer tokens than pretty JSON). Agents
can opt into raw JSON per-call via `format: "json"` when they need structured
parsing.

Build:

```bash
cargo build --release -p ast-index-mcp
# Binary: target/release/ast-index-mcp
```

Exposed tools (20):

| Tool | Purpose |
|------|---------|
| `search` | Universal search across files, symbols, imports, content |
| `symbol` | Find symbols by exact name / glob / kind filter (precise alternative to `search`) |
| `class` | Find classes, interfaces, protocols, enums, structs by name or pattern |
| `outline` | Structural outline of a file (call before reading >500-line files) |
| `usages` | Every usage of a symbol (file:line + context) |
| `callers` | Direct callers of a function |
| `call_tree` | Recursive caller tree, configurable depth |
| `implementations` | Types that implement/extend an interface or parent |
| `hierarchy` | Full inheritance tree — superclasses + subclasses in one call |
| `refs` | Definitions + imports + usages in one shot |
| `imports` | Imports / includes of a source file |
| `api` | Public API of a module (refactoring & changelog prep) |
| `changed` | Symbols that changed since a base branch (code review) |
| `module` | Find modules matching a pattern |
| `deps` | Module dependencies |
| `dependents` | Reverse deps — who depends on this module |
| `find_file` | Locate files by name pattern |
| `stats` | Project type, counts, DB size, extra roots |
| `rebuild` | Full reindex (slow — prefer `update`) |
| `update` | Incremental reindex (fast) |

Setup instructions per agent: [`docs/mcp-setup.md`](docs/mcp-setup.md).

### Gemini CLI

```bash
gemini skills install https://github.com/defendend/Claude-ast-index-search.git --path plugin/skills/ast-index
```

### Generic Rule-Based Agents

Add to `.cursor/rules` or project-specific agent rules:

```markdown
Use `ast-index` CLI for fast code search. Run `ast-index rebuild` before first use.
Available commands: search, class, implementations, usages, callers, call-tree, deps, outline, deprecated.
```

---

## 💝 Support Development

[![Support on Boosty](https://img.shields.io/badge/Support%20on-Boosty-FF5722?style=for-the-badge&logo=star)](https://boosty.to/ast_index/donate)

---

## Commands (46+)

### Grep-based (no index required)

```bash
ast-index todo [PATTERN]           # TODO/FIXME/HACK comments
ast-index callers <FUNCTION>       # Function call sites
ast-index provides <TYPE>          # @Provides/@Binds for type
ast-index suspend [QUERY]          # Suspend functions
ast-index composables [QUERY]      # @Composable functions
ast-index deprecated [QUERY]       # @Deprecated items
ast-index suppress [QUERY]         # @Suppress annotations
ast-index inject <TYPE>            # @Inject points
ast-index annotations <ANN>        # Classes with annotation
ast-index deeplinks [QUERY]        # Deeplinks
ast-index extensions <TYPE>        # Extension functions
ast-index flows [QUERY]            # Flow/StateFlow/SharedFlow
ast-index previews [QUERY]         # @Preview functions
ast-index usages <SYMBOL>          # Symbol usages (falls back to grep)
```

### Index-based (requires rebuild)

```bash
ast-index search <QUERY>           # Universal search
ast-index file <PATTERN>           # Find files
ast-index symbol <NAME>            # Find symbols
ast-index class <NAME>             # Find classes/interfaces
ast-index symbol <NAME>            # Find any symbol by name
ast-index implementations <PARENT> # Find implementations
ast-index hierarchy <CLASS>        # Class hierarchy tree
ast-index usages <SYMBOL>          # Symbol usages (indexed, ~8ms)
```

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

### File analysis

```bash
ast-index outline <FILE>           # Symbols in file
ast-index imports <FILE>           # Imports in file
ast-index changed [--base BRANCH]  # Changed symbols (git diff)
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

## Performance

Benchmarks on large Android project (~29k files, ~300k symbols):

| Command | Rust | grep | Speedup |
|---------|------|------|---------|
| imports | 0.3ms | 90ms | **260x** |
| dependents | 2ms | 100ms | **100x** |
| deps | 3ms | 90ms | **90x** |
| class | 1ms | 90ms | **90x** |
| search | 11ms | 280ms | **14x** |
| usages | 8ms | 90ms | **12x** |

### Size Comparison

| Metric | Rust | Python |
|--------|------|--------|
| Binary | ~44 MB | ~273 MB (venv) |
| DB size | 180 MB | ~100 MB |
| Symbols | 299,393 | 264,023 |
| Refs | 900,079 | 438,208 |

## Architecture

- **grep-searcher** — ripgrep internals for fast searching
- **SQLite + FTS5** — full-text search index
- **rayon** — parallel file parsing
- **ignore** — gitignore-aware directory traversal

### Database Schema

```sql
files (id, path, mtime, size)
symbols (id, file_id, name, kind, line, signature)
symbols_fts (name, signature)  -- FTS5
inheritance (child_id, parent_name, kind)
modules (id, name, path)
module_deps (module_id, dep_module_id, dep_kind)
refs (id, file_id, name, line, context)
xml_usages (id, module_id, file_path, line, class_name, usage_type, element_id)
resources (id, module_id, type, name, file_path, line)
resource_usages (id, resource_id, usage_file, usage_line, usage_type)
transitive_deps (id, module_id, dependency_id, depth, path)
storyboard_usages (id, module_id, file_path, line, class_name, usage_type, storyboard_id)
ios_assets (id, module_id, type, name, file_path)
ios_asset_usages (id, asset_id, usage_file, usage_line, usage_type)
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

Legacy `project_type:` keys are ignored for backward compatibility, so old
configs keep working unchanged.

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

### 3.47.0
- **Named workspace subtrees (#31)** — attach sibling project trees to the current index under short names. `ast-index subtree add <name> <path>` stores the path verbatim (relative or absolute) and indexes its files alongside the primary root. `ast-index subtree remove <name>` detaches by name. `ast-index subtree list` prints `name | original_path | canonical_path` (or JSON via `--format json`). The legacy `add-root` / `remove-root` / `list-roots` commands still work; they auto-generate a subtree name from the path basename for the same effect.
- **`[name]` prefix in text output for subtree-owned files** — `search`, `symbol`, `class`, `usages`, `refs`, `implementations`, `outline`, and `file` print `[name] /abs/path` when the result lives in a named subtree. Primary-project paths stay unprefixed. `--format json` keeps the raw absolute path in the `path` field so downstream tooling doesn't have to parse the prefix.
- **Global `--subtree <name>` and `--local` filters** — restrict search results to one subtree or to the primary project only. The two flags are mutually exclusive and exit with a clear error when combined. Note: the filter is applied after `--limit`, so the visible result count may be lower than the limit when a subtree matches few rows.
- **Soft warning at 500k candidate files** (env `AST_INDEX_WARN_FILES`) — walker prints a one-shot stderr warning when crossing the threshold, then keeps going. Hard abort cap stays at 2,000,000 (`AST_INDEX_MAX_FILES`).
- **Auto-migration of pre-3.47 `extra_roots` metadata** — the old JSON-encoded list of extra roots is rewritten into the new `subtrees` table on the first `open_db`, deriving each subtree's name from its path basename and deleting the legacy row. Idempotent: subsequent opens find no legacy row and exit early.

### 3.46.1
- **Preserve the previous index when `rebuild` aborts** — pre-3.46.1, `rebuild` ran `delete_db` before the walker, so any abort (file-count cap hit, IO error, parser panic that propagates) left the user with an empty database and no working index. Now `cmd_rebuild` (both the regular and `--sub-projects` branches) atomically renames the live DB aside via a new `RebuildSwap` RAII guard. On success the guard's `commit()` removes the swap; on any error the destructor restores the swap back into place. The next `rebuild` also cleans up any leftover swap from a previous crashed run before it starts.

### 3.46.0
- **Index Android resources in `--sub-projects` mode even without layout XML** — sub-projects with `res/values/`, `res/drawable/`, `res/colors/` etc. but no `res/layout|menu|navigation/` now correctly run through `index_resources`. The guard `any_android && !all_xml_files.is_empty()` is split, and `any_android` is also promoted from collected `res_files` so a monorepo sub-project whose `build.gradle` lives one level deeper still indexes resources.
- **Narrow the `/res/` walker filter to canonical Android subdirs** — `res/values`, `res/layout`, `res/drawable`, `res/menu`, `res/navigation`, `res/mipmap`, `res/anim`, `res/animator`, `res/color`, `res/font`, `res/interpolator`, `res/raw`, `res/transition`, `res/xml` (plus their `-<qualifier>` variants). A non-Android `assets/res/dataset/labels.xml` no longer leaks into resource indexing, and `res/values-pl/layout_attrs.xml` no longer gets misclassified as a layout file.
- **Add `safe_canonicalize` to survive dead FUSE mounts during rebuild** — `Path::canonicalize` now runs on a side thread with a 5s timeout (`AST_INDEX_CANONICALIZE_TIMEOUT_MS`) and falls back to the raw path with a stderr warning if the filesystem stops responding (stale VFS mounts, unreachable network paths). The migration loop inside `db::get_db_path` also stopped canonicalizing foreign `project_root` strings stored in old cache databases, which previously could pin rebuild forever. Set `AST_INDEX_NO_CANONICALIZE=1` to skip canonicalize entirely.
- **Cap memory peaks during rebuild** — three independent guards stop a single big-blob file or an accidental monorepo-root rebuild from blowing past dozens of GB:
  - Symbol signatures are truncated to 500 chars at the DB insert chokepoint, so minified bundles no longer multiply `N_symbols × line_length` into the heap.
  - File-size cap (default 1 MB, env `AST_INDEX_MAX_FILE_SIZE`) — oversized files are recorded for change tracking but never read into memory.
  - Walker file-count cap (default 2,000,000, env `AST_INDEX_MAX_FILES`) — exits with an actionable message pointing at `rebuild --force`, `rebuild --force --remember`, and `rebuild --max-files N`. `--remember` persists the opt-in for the project via a `bypass_size_check=1` metadata flag.
- **More verbose checkpoints in `rebuild`** — every potentially blocking syscall (db path resolution, `.ast-index.yaml` load, `find_sub_projects`) now prints its own `[verbose]` line before running, so any future "hangs after X" report points at the exact failing step.

### 3.45.0
- **Add `ast-index detect-stacks` for reliable multi-stack detection** — emits the full list of stacks present at the project root with their marker files, plus `is_kmp` and `is_polyglot` flags; Kotlin Multiplatform is recognised by requiring both a `kotlin("multiplatform")` plugin reference and a `commonMain`/`androidMain`/`iosMain`-style source set, so an Android-only Kotlin app no longer trips KMP detection (#44).
- **Smart `/initialize` now drives off the new detector** — instead of hand-scanning markers, the command parses `ast-index --format json detect-stacks` output and composes per-stack rules for single-stack, KMP, and polyglot/monorepo repos in one go (#44).
- **Ship event-driven hooks in the Claude plugin** — `plugin/hooks/hooks.json` adds a `PostToolUse` (Edit/Write/MultiEdit/NotebookEdit) debounced incremental update and a `SessionStart` refresh; both hooks no-op when an `ast-index watch` daemon is running and when no index exists, so they never block agent sessions (#45).
- **Add `ast-index install-git-hooks`** — installs idempotent `post-checkout`, `post-merge`, and `post-rewrite` hooks under `.git/hooks/` that re-run `ast-index update` in the background after each git operation; supports `--dry-run` and `--force`, and uses a comment marker so re-runs detect previously installed scripts and skip them instead of complaining (#45).

### 3.44.2
- **Allow colliding relative paths across extra roots without crashing rebuild** — files are now keyed by both owning root and relative path, so `rebuild`/`update` no longer fail with `UNIQUE constraint failed: files.path` when the primary root and an extra root both contain files like `Workspace.swift` or `Geko/Config.swift`; path resolution now also preserves the correct absolute file for each root

### 3.44.1
- **Index C++ enum values and relax symbol CLI compatibility** — C++ enumerators like `kAntifraud` are now indexed as constants, so `symbol kAntifraud`, `symbol AcceptanceOperationInitiator::kAntifraud`, `symbol ::kAntifraud`, and wildcard forms such as `symbol AcceptanceOperationInitiator::*` work without forcing users to switch to `-p`

### 3.44.0
- **Add namespace-aware C++ symbol search without breaking bare lookups** — `symbol Client` still finds all matching classes, while `symbol arcanum::Client`, `symbol -p '::Client'`, and `symbol -p 'foo::bar*'` now narrow by fully-qualified names and print qualified results to disambiguate duplicate class names across namespaces
- **Expand `module-route` diagnostics and graph output** — `module-route` now returns clearer empty/truncated reasons, better timeout handling, self-edge handling, `json`/`mermaid`/`dot` output, and stronger regression coverage for large fanout graphs
- **Harden rebuild/update management paths** — `query` now blocks mutation SQL while still allowing `SELECT`, `WITH`, and `EXPLAIN`; `update` now preserves extra roots, respects `include` allow-lists, and keeps root-level files intact in sub-project rebuild mode
- **Improve TypeScript/Vue and general parser coverage** — better extraction for Vue Composition API / Pinia / script-setup patterns, broader parser/test coverage across multiple languages, and more reliable file-oriented commands
- **Make safe rebuild optimizations the default and add benchmarks** — full rebuilds now always bulk-load into a thin schema before creating indexes/FTS, use a slightly larger SQLite cache during rebuild, and ship baseline Criterion benches for rebuild, update, module graph, and Android XML/resource phases

### 3.43.2
- **Preserve root-level files in sub-project rebuilds** — experimental fast rebuild no longer drops files and module markers that live directly under the selected root when it switches large monorepos into sub-project mode

### 3.43.1
- **Propagate `--experimental-fast-rebuild` mode to `update` and `watch`** — once a project is rebuilt with the experimental fast pipeline, subsequent incremental updates and file watching now automatically reuse the same mode from index metadata instead of falling back to the default slower path

### 3.43.0
- **Add `--experimental-fast-rebuild` for large monorepos** — opt-in rebuild pipeline that leaves the default `rebuild` behavior unchanged while speeding up big trees via deferred index/FTS creation, monorepo sub-project shortcuts, and faster dependency/XML/resource indexing. On `taxi/mobile/go-client-android`, local rebuild time dropped from about 62s to about 16s

### 3.42.0
- **Dart: swap to `tree-sitter-dart` 0.2.0 (nielsenko) with full Dart 3 support (#40)** — the previous `tree-sitter-dart-orchard 0.3.2` grammar pre-dates Dart 3 and required `ERROR`-node recovery heuristics to handle class modifiers (`sealed`, `base`, `final`, `interface`, `mixin class`), extension types, records, switch expressions, and pattern matching. Once multiple Dart 3 features stacked in one file the recovery collapsed and the file returned **0 symbols**. Switching to the natively Dart-3-aware nielsenko grammar lets the parser walk the AST normally; the rewrite also closes five long-standing extraction gaps that the orchard codepath silently swallowed (class-level instance fields, top-level `external` variables, `operator` overloads, top-level setters misclassified as `Function`, and old C-form typedefs that were recording the *return type* as the typedef name). On a 96 505-file mixed Dart 2/3 codebase symbol count rises by +22% (+268 038 symbols); the previously black-hole Dart 3 files now index cleanly. Two upstream-grammar quirks are papered over with documented workarounds (the `typedef Foo = Future<void> Function(...)` mis-parse and the `library_export` shape that lacks an `import_specification` wrapper). Thanks to @f1xgun for the change
- **Set rayon worker stack to 32 MB to prevent stack-overflow crashes on deep AST recursion (#39)** — `ast-index rebuild` was crashing with `thread '<unknown>' has overflowed its stack` (exit 134) on large Flutter codebases that include the Dart SDK test corpus (deeply nested generics, pathological recursion fixtures). Tree-sitter parsers recurse on each AST node, and the Rust default worker stack (~2 MB on macOS) is too small for the recursive descent these files require. Each `rayon::ThreadPoolBuilder` in the indexer now sets `.stack_size(32 * 1024 * 1024)` (32 MB), which is language-agnostic — benefits every tree-sitter grammar that can recurse deeply, not just Dart. After the patch a previously-crashing 162 049-file Flutter project rebuilds cleanly. Stack pages are demand-committed by the OS, so typical files cost nothing extra. Thanks to @f1xgun for the fix

### 3.41.0
- **Codex/Cursor skill installation docs** — documented direct skill installs through `~/.codex/skills/ast-index` and `~/.cursor/skills/ast-index`, while keeping plugin marketplace metadata as an optional packaging surface.
- **Codex and Cursor plugin packaging** — added first-class Codex and Cursor manifests, repo-local marketplaces, a Cursor-specific project rule/initializer command, and a validation script so agent packaging stays in sync with release version bumps. The existing Claude plugin payload remains unchanged.
- **Add `module-route` command** — find transitive dependency path(s) between two modules. Supports shortest (BFS) and all-paths (iterative DFS) modes, four output formats (`text`, `json`, `mermaid`, `dot`), edge-kind filtering (`--via-kind api|implementation|all`), configurable depth and path caps, and a wall-clock timeout guard. Accepts Gradle-style module names (`:core:utils`, `core/utils`, `core.utils` all resolve to the same module)
- **Fix `module-route --all` returning "no path" on large graphs when a direct edge exists** — DFS iterated each frame's edges in alphabetical order, so on a `:app` with many siblings it could blow the 2000ms timeout exploring decoy subtrees before reaching an alphabetically late target. Now each frame's edges are reordered so any edge pointing directly at `to` is processed first; the 1-hop direct path is recorded before any deep recursion. When the search still ends empty due to timeout or `max_paths`, the rendered message says so (`truncated_timeout` / `truncated_max_paths`) instead of the misleading "No dependency path"
- **Surface `module-route --all` search progress and a suggested `--timeout-ms`** — DFS now tracks `nodes_visited`, `edges_explored`, `max_depth_reached`, and `elapsed_ms`; the `truncated` text/JSON output includes these and a doubled-and-clamped `suggested_timeout_ms` so the user can tell whether the search ran out of time vs. exhausted the graph and knows the next value to try. Default `--timeout-ms` raised from 2000 to 5000 to give real Android-monorepo graphs more headroom out of the box
- **Bidirectional pruning for `module-route --all` on large graphs** — before DFS, run a reverse BFS from `to` to compute `dist_to[node]` (min hops from each node to `to`). DFS skips any child not in `dist_to` (cannot reach the target) and any child where `current_depth + dist_to[child] > max_depth` (cannot reach within budget). On a 1000-module Gradle project with `:app` fanning out to ~150 direct deps, this turns "DFS hits 5s timeout, returns nothing" into "answer in <100 ms" because 90%+ of decoy subtrees are pruned at the edge level and never recursed into. Adds `db::get_incoming_edges_dedup` for the reverse traversal

### 3.40.4
- **`update` now honours `.ast-index.yaml` (`include` / `exclude`)** — previously only `rebuild` loaded the project config. On a monorepo with `include: [adfox, yabs/adfox]`, `update` would crawl the entire repository (no scope), hang indefinitely on Arcadia-sized trees, and silently pull files outside the configured scope into the DB — making `search` return results from `crypta/`, `sim/`, etc. that were never in `include`. `cmd_update` now loads the config and replaces the primary walker root with the listed sub-paths; paths in the DB stay anchored to the outer root (matching what `rebuild` writes), and `exclude` patterns are applied to every walked entry. `cmd_watch` got the same treatment so file-system watchers stay scoped too. Regression test feeds a seeded DB with two `include` paths and asserts that `crypta/` / `sim/` decoys never enter the index.
- **Bump default thread count for `update` from `min(cpu, 8)` to `32`** — incremental updates on monorepos benefit from much higher parallelism than the cautious rebuild default (the walker has already paged inodes into the FS cache, parsing is purely CPU-bound). Override via `AST_INDEX_THREADS=N` if you want a different pool size

### 3.40.3
- **Fix Gradle `project(":path")` undercount in Forma-style `deps(...)` blocks** — the wrapper-anchored regex `\b(\w+)\s*\(\s*project\s*\(` only matched the *first* `project(...)` per `deps(` block, silently dropping the rest. On real Forma projects this masked the majority of internal edges (e.g. `dependents` returning 0 when the truth was hundreds). The fix scopes a project-only fallback to `<name>dependencies = wrapper(...) [+ wrapper(...)]*` assignment blocks (paren-balanced scan + line-comment strip), so phantom edges from `project("...")` in comments, string literals, or unrelated code cannot leak in. A per-file `(module_id, dep_id)` HashSet keeps the wrapper-anchored regex's real `dep_kind` (api/compileOnly/...) when both fire on the same edge. Regression tests cover (1) `deps(externals) + deps(project, project, project)` chains and (2) decoy `project(...)` text inside comments and Kotlin string literals

### 3.40.2
- **Fix Windows release build broken by tree-sitter-scss 1.0.0** — the published crate hardcodes `-Wno-unused-parameter` in `build.rs`, which MSVC `cl.exe` rejects with `error D8021: invalid numeric argument`, killing the entire Windows + matrix build (v3.40.0 and v3.40.1 release artifacts never published). Upstream master has the platform-conditional fix from 2024-04-26 but never cut a 1.0.1; pinned via `[patch.crates-io]` to that commit
- **Bump GitHub Actions to non-deprecated versions** — `actions/checkout@v4 → v6`, `actions/setup-node@v4 → v6`, `actions/upload-artifact@v4 → v7`, `actions/download-artifact@v4 → v7`. Silences Node.js 20 deprecation warnings (Node.js 20 is removed from runners on September 16th, 2026)

### 3.40.1
- **Parse custom Gradle DSL `<wrapper>(project(":path"))` dependencies (#33)** — previously the indexer only recognised `api`/`implementation`/`compileOnly`/`testImplementation` as dependency configurations. Custom DSLs that wrap `project(...)` under a different identifier (e.g. Forma's `deps(project(":foo"))`, `kapt(project(...))`, `classpath(project(":buildSrc"))`) were silently ignored. The Gradle parser now accepts any `<word>(project(":path"))` form, so `ast-index deps` / `dependents` / `module` correctly cover Android projects on custom DSLs. Thanks to @AndVl1 for the fix

### 3.40.0
- **CSS / SCSS / PCSS / Less language support** — tree-sitter based parsers for `.css`, `.pcss`, `.postcss` (via the CSS grammar), `.scss`, and `.less`. Indexed: class selectors (`.foo`), id selectors (`#bar`), CSS custom properties (`--var`), SCSS variables (`$primary`), Less variables (`@brand`), `@mixin`/`@function`/`%placeholder` (SCSS), `.mixin()` definitions (Less), `@keyframes`, and `@import`/`@use`/`@forward` paths. `ast-index file` walks the new extensions automatically. FTS5 tokenization treats `$`, `@`, `--`, `%` as separators, so `search primary` finds `$primary` and `search brand` finds `@brand`
- **Fix #32: `hierarchy` silently truncates at 50** — `ast-index hierarchy "BaseQueryService"` returned only the first 50 children alphabetically (e.g. A → E), losing 60% of real subclasses on a hierarchy of 125 with no warning. The command now accepts `--limit <N>` (default 200), reports the total count alongside the displayed slice (`Children (50 of 125 shown)`), and prints a yellow `Truncated.` hint with the exact `--limit` value needed to see all results

### 3.39.0
- **Zig language support** — tree-sitter based parser with `.zig` and `.zon` extensions, `ProjectType::Zig` auto-detection via `build.zig` / `build.zig.zon`, integration test covering fn/struct/field/test-block symbol extraction
- **MCP server expanded to 20 tools** — added `symbol`, `class`, `hierarchy`, `imports`, `api`, `changed`, `module`, `deps`, `dependents`, `call_tree` on top of the original 10. Covers precise symbol lookup, file context, module-level navigation, and code-review workflows without requiring multiple `ast-index` command shells. See [`docs/mcp-setup.md`](docs/mcp-setup.md)
- **`--walk-up` / `AST_INDEX_WALK_UP` opt-in for monorepos** — when enabled, read-commands prefer any existing parent-directory index over nested project/VCS markers. Useful when subprojects carry their own `.git` / `Cargo.toml` / `settings.gradle` but share a root-level index. Default off — safe from accidentally-broad parent indexes (#30)
- **Fix #25: SQL project detection** — folders containing `.sql` files now report `ProjectType::Sql` instead of `Unknown`, matching the README's advertised SQL support
- **Gated `Time:` / `Total time:` output behind `--verbose`** — `rebuild` and `update` no longer print timing lines by default, keeping agent output clean. Pass `--verbose` to see per-phase timing
- **Database schema now documented** — see [`docs/db-schema.md`](docs/db-schema.md) for the full ER diagram, design decisions (adjacency-list vs materialized-path, why `refs.name` is TEXT not FK), and common query patterns
- **Smoke-test + benchmark tooling** — `scripts/smoke.sh` runs six end-to-end CLI scenarios against the release binary (including a perf-budget scenario); `scripts/check-pr.sh` chains build → tests → smoke → bench compile-check for pre-PR validation. See [`docs/smoke-testing.md`](docs/smoke-testing.md) and [`docs/benchmarks.md`](docs/benchmarks.md)
- **Property-based parser tests** — `tests/parser_proptest.rs` exercises 5 languages × 3 properties (no-panic / determinism / line-bounds) with 64 random cases each, using `proptest = "1"`
- **60+ new integration tests** — across `tests/files_command_tests.rs`, `tests/indexer_detection_tests.rs`, `tests/management_query_tests.rs`, `tests/zig_tests.rs`, plus inline unit tests covering MCP argv dispatch (20), format shapers (15), and root-lookup logic (15)
- **Contributor guide** — `CLAUDE.md` at the repo root and seven focused rules under `.claude/rules/` (architecture, commands, parsers, commits, testing, release, verify) give AI coding agents a consistent spec for the codebase. Three agent profiles under `.claude/agents/` (bug-fix, research, review) encode reusable workflows

### 3.38.1
- **Fix ambiguous paths in search output under extra roots** — when `add-root` pointed outside the primary project, `search`/`symbol`/`class`/`implementations`/`refs`/`hierarchy`/`usages` printed only the stored relative path (e.g. `src/main/java/.../BClass.java`) with no indication of which root owned it. Agents defaulted to the primary project and failed to open the file. Now, when any extra root is configured, index-backed results are resolved to absolute paths by probing each root on disk (primary first, then extras). Single-root output is unchanged

### 3.38.0
- **Fix `update` wiping files under extra roots** — `update_directory_incremental` walked only the primary root, marking all extra-root files as deleted on every run. Now walks primary + every `extra_root` from metadata, computing relative paths per-root to match `rebuild`'s storage scheme. On a 111k-file project this was deleting 85k files per `update`

### 3.37.1
- **Sub-projects mode now indexes modules, deps, XML layouts, resources, storyboards, and assets** — previously `cmd_rebuild_sub_projects` only indexed source files, silently skipping module detection, dependency graph, Android XML/resources, and iOS storyboards/assets. Now all collected build files, layout files, and asset dirs from sub-project walks are processed after the main loop, matching the behavior of single-project rebuild
- **`ya.make` recognized in project type detection** — directories with `ya.make` (and no other build system markers) are now detected as C++ projects instead of Unknown, improving type-specific behavior in monorepo sub-projects

### 3.37.0
- **`outline` now uses tree-sitter for all languages** — Perl, Python, Go, C++, Kotlin previously used regex fallback with inaccurate results. Now routes through the same tree-sitter parsers used for indexing, with a generic fallback via `FileType::from_extension` covering ~30 languages. `outline` still works without a database, parsing the file on the fly (~5-50ms per file, e.g. 1806-line Kotlin file with 1298 symbols → 47ms)
- **Python `import X as Y` now indexed** — `refs <module>` previously missed `import sqlalchemy as sa` while finding `from sqlalchemy import orm`. Tree-sitter query extended to emit both the original module name and the alias as `Import` symbols
- **`ya.make` build system support**:
  - `ya.make` files recognized as module markers during the single directory walk
  - Module keys use the path relative to the outer repository root so that `PEERDIR(...)` entries — which are repo-root-relative — can be matched by literal lookup
  - `PEERDIR(...)` parser handles both single- and multi-line blocks with whitespace-separated paths, emits entries with `dep_kind=peerdir`
- **Python dependency parsing from `pyproject.toml` / `setup.py`**:
  - `[project] dependencies = [...]` (PEP 621)
  - `[tool.poetry.dependencies]` (skips `python` and external packages, matches only internal modules)
  - `install_requires=[...]` in `setup.py`/`setup.cfg`
  - Strips PEP 508 version specifiers, extras and markers to get just the package name
  - Verified on real Python project: 126 modules / 267 deps detected
- **Dep indexing no longer gated on Android** — the `Indexing module dependencies...` step used to run only when the project was detected as Android, silently skipping Java/Python/ya.make monorepos. Now runs whenever there are any modules in the index, regardless of build system
- **`include` config now always routes through the scoped path** — previously an `include` allow-list only honored if auto-switch to sub-projects mode triggered (≥2 sub-projects AND ≥65k files). Small projects with `include` set fell through to the main branch which walked the entire root, silently ignoring the filter. Now `include` always forces scoped walking of only the listed directories, with a clear `Honoring include config (N paths)` message
- **Nested `include` paths now work literally** — previously `include: [smart_devices/tools/burn_data]` would be expanded to the top-level `smart_devices/` directory (because `find_sub_projects` only matched immediate subdirs by name). A 300-path config ended up indexing the entire tree because each entry matched the outer dir. Now each include entry is taken as-is and becomes its own scoped root — `cmd_rebuild_sub_projects` walks exactly the listed paths, no wider. Sub-project display uses relative paths for nested entries
- **`.h` file routing** — `detect_h_file_objc` promoted to `pub` so `outline` can use the same ObjC/C++ auto-detection that indexing uses

### 3.36.2
- Release build check

### 3.36.1
- `watch` command: per-project flock to prevent duplicate watchers

### 3.36.0
- **Monorepo exclude/include support**:
  - `exclude` config now works in sub-projects mode — previously was silently ignored when auto-switching to sub-projects
  - `exclude` now uses full gitignore syntax (`*`, `**`, `?`, path-based patterns like `proto/gen`)
  - Extra roots now indexed with `exclude` filter (was missing)
  - New `include` allow-list in `.ast-index.yaml` — only index matching directories, skip everything else. Ideal for large monorepos where you need a handful of dirs
  - New CLI flags: `--include`, `--exclude`, `--path` for `rebuild` command

### 3.35.0
- **BSL fixes** (issue #19 by @colegero):
  - **Module indexing** — 1C modules now extracted from directory paths (`CommonModules/X/` → `X`, `Documents/Y/` → `Документ.Y`, etc.), fixes `Modules: 0` on 1C configurations. Supports 35+ 1C metadata collections.
  - **`outline` for BSL files** — routes `.bsl`/`.os` to tree-sitter parser (was falling back to Kotlin regex → no symbols found)
  - **Query planner optimization** — added composite index `idx_refs_name_file_line` + early materialization in `find_references` via subquery. On large BSL databases (12M+ refs) `usages`/`callers` went from 30s timeout to <10ms (benchmarked ~76x faster on 28k refs)

### 3.34.0
- **Swift improvements** (contributed by @kolyuchiy):
  - Tree-sitter based detection of SwiftUI property wrappers (`@Environment`, `@AppStorage`, `@Bindable`, `@Observable`) — replaces regex approach
  - Tree-sitter based async function detection with multi-line signatures
  - Language-aware reference extraction — separate keyword sets for Swift/Kotlin/Java, less noise
  - Correct inheritance semantics: struct/enum/actor/protocol parents marked as `implements`, not `extends`
  - Extension conformances now tracked
  - `.h` file content sniffing — auto-route to ObjC parser when ObjC markers found
- **SQL injection fix in iOS commands** — parameterized queries in `storyboard-usages`, `asset-usages`, `asset-unused` (contributed by @kolyuchiy)
- **Scoped implementations fix** — `find_implementations_scoped` uses SQL filtering instead of post-query in-memory filter, no more result loss (contributed by @vadimvolk)
- Ruby parser: language-aware reference extraction via `extract_refs_for_lang`

### 3.33.2
- **Java record support** — `record` declarations indexed as classes with inheritance, record components as properties + synthetic accessor methods, dedup when accessor is explicitly overridden (contributed by @viktoraseev)

### 3.33.1
- Internal release (no user-facing changes)

### 3.33.0
- **Fix DB lookup after VFS remount** — canonicalize project path before hashing, so index survives arc remount
- **Auto-migrate** old DBs created with raw paths to normalized paths

### 3.32.0
- **npm distribution** — `npx @ast-index/cli` now works on all platforms (darwin arm64/x64, linux x64/arm64, win32 x64) via scoped optional dependencies (contributed by @SiereSoft)

### 3.31.0
- **GDScript (Godot) support** — 30th language: class_name, class, func, signal, enum, const, var, @export var, @onready var, extends hierarchy
- **Fix BSL cross-compilation** — added `.std("c11")` to build.rs for tree-sitter-bsl C code compilation

### 3.30.0
- **TS/Vue: callers for await/return** — `await func()`, `return obj.func()` patterns now detected by callers command
- **TS: Vue Composition API outline** — `ref()`, `computed()`, `reactive()`, `defineProps()`, `defineStore()` variables appear in outline
- **Ruby: bang/question methods in usages** — `save!`, `valid?` methods now tracked in references
- **Ruby: Alba serializer & Dry::Initializer DSL** — `attribute`, `attributes`, `one`, `many`, `option`, `param` parsed as properties
- **Glob patterns for class/symbol** — `--pattern "*Mailer"` for class and symbol commands
- **Comma-separated OR queries** — `search "email,mail"` searches both terms with deduplication
- **--type filter for search** — `search query --type class`
- **--in-file/--module filters for hierarchy** — filter children by file or module path
- **Fix --in-file matching** — uses contains match instead of suffix match

### 3.29.1
- **Fix IX build** — replaced rusqlite `bundled-full` with `bundled` to remove `time` crate dependency that failed in IX sandbox

### 3.29.0
- **Upgraded Dart grammar** — switched to tree-sitter-dart-orchard 0.3.2, native Dart 3 support (sealed/base/final/interface classes, extension types, records, patterns)
- **Fix implementations false positives** — removed `LIKE '%name%'` pattern that returned 6000+ false matches instead of ~180 real ones
- **Expanded grep commands coverage** — added 50 file extensions to `ALL_SOURCE_EXTENSIONS` for todo/deprecated/annotations/callers/search commands (Dart, Lua, Elixir, Shell, SQL, R, BSL, Common Lisp, and more)

### 3.28.0
- **Common Lisp support** — 29th language, defun/defmacro/defgeneric/defmethod/defclass/defstruct/defvar/defparameter/defconstant/defpackage parsing (contributed by @svetlyak40wt)

### 3.27.0
- **Matlab support** — 28th language, classdef/function/properties/enumeration/events parsing with Matlab vs ObjC `.m` file auto-detection

### 3.26.2
- **Fix project root detection** — `rebuild` now uses CWD instead of searching upward, fixing wrong root in monorepos

### 3.26.1
- **Windows support** — `winget install defendend.ast-index` now available (contributed by @kulemeevag)
- **Gemini CLI support** — added skill installation instructions
- **MIT license** — added LICENSE file
- **Release automation** — winget auto-update in GitHub Actions release workflow (contributed by @kulemeevag)

### 3.26.0
- **Ruby callers/call-tree support** — `rb` added to scanned extensions, Ruby-specific call patterns (`.method` without parens, `:method_name` symbol refs, `method.chain`), bang/question method handling (`authenticate_user!`, `valid?`) (contributed by @melnik0v)
- **Ruby parser improvements** — show `include`/`extend`/`prepend` in outline, `validate` (without `s`), all ActiveRecord callbacks (`after_commit`, `around_*`), multi-arg `attr_reader`/`attr_writer`/`attr_accessor`, Rails DSL (`enum`, `delegate`, `has_one_attached`, `encrypts`, `store_accessor`), `RSpec.describe` with receiver, `shared_examples`/`shared_context` (contributed by @melnik0v)
- **Vue/Svelte outline support** — `outline` command now works for `.vue` and `.svelte` files with correct line numbers, Vue 3 Composition API (`ref`, `reactive`, `computed`, `defineProps`, `defineEmits`), lifecycle hooks, `export default` detection (contributed by @melnik0v)
- **TypeScript/JS callers expansion** — `ts`, `tsx`, `mts`, `js`, `jsx`, `vue`, `svelte` added to `callers` and `todo` command extensions

### 3.25.1
- **Configuration file support** — create `.ast-index.yaml` in project root to set `project_type`, `roots`, `exclude`, `no_ignore` (CLI flags override config values)

### 3.25.0
- **Fix BSL parser ABI** — regenerate parser.c with ABI 15 for tree-sitter 0.26 compatibility (BSL tests were silently failing since v3.24.0)
- **Fix BSL keyword priority** — identifier token lowered to `prec(-1)` so keywords like `Процедура`/`Procedure` are recognized correctly
- **Ruby nested scope tracking** — qualified names for nested class/module definitions (e.g. `Event::CreateService`, `Api::V2::UsersController`) (contributed by @melnik0v)
- Remove local config files and mobile-tools from repo
- 462 total tests

### 3.24.0
- **BSL parser: all 7 issues fixed** — complete overhaul of 1C:Enterprise BSL parser per official 8.3.27 documentation
  - `SymbolKind::Procedure` — procedures and functions now distinguished
  - Compilation directives (`&НаКлиенте`, `&AtServer`, etc.) indexed as `Annotation`
  - `Export`/`Экспорт` keyword captured in signature
  - Extension annotations (`&Перед`, `&После`, `&Вместо`, `&ИзменениеИКонтроль`) indexed
  - `extract_refs` — full Cyrillic support via `\p{Cyrillic}` regex
  - `strip_comments` — BSL uses `//` only, no `/* */`
  - `Асинх`/`Async` modifier — grammar.js rewritten from scratch, parser.c regenerated with `tree-sitter generate`
- **tree-sitter-bsl grammar rewrite** — new grammar.js covering all BSL 8.3.27 constructs: procedures, functions, variables, regions, annotations, preprocessor directives, async/await, goto, handler statements
- **52 BSL keywords** in ref filter (26 Russian + 26 English), per official reserved words list
- 16 BSL tests, 457 total tests

### 3.23.0
- **6 new languages** — Lua (`.lua`), Elixir (`.ex`, `.exs`), Bash (`.sh`, `.bash`, `.zsh`), SQL (`.sql`), Groovy (`.groovy`, `.gradle`), R (`.r`, `.R`); all with full tree-sitter AST parsing
- 23 languages supported, 447 tests

### 3.22.1
- **`--project-type` flag** — force project type in `rebuild` when auto-detection is wrong (e.g., `ast-index rebuild --project-type dart`)

### 3.22.0
- **BSL (1C:Enterprise) support** — full tree-sitter parser for BSL/OneScript: procedures, functions, variables, regions; file extensions `.bsl`, `.os`
- **BSL project detection** — detects 1C projects by `Configuration.mdo`, `Configuration.xml`, `ConfigDumpInfo.xml`, `packagedef`, or `.bsl`/`.os` files
- **Project type detection for all languages** — added C# (`.sln`, `.csproj`), C++ (`CMakeLists.txt`), Dart/Flutter (`pubspec.yaml`), PHP (`composer.json`), Ruby (`Gemfile`, `.gemspec`), Scala (`build.sbt`)
- **`--project-type` flag** — force project type in `rebuild` when auto-detection is wrong (e.g., `ast-index rebuild --project-type dart`)

### 3.21.1
- **Fix: Windows home directory indexing** — `find_project_root()` now stops at `$HOME` boundary, preventing indexing of entire user directory when stale DB exists above project
- **Flutter/Dart project detection** — added `pubspec.yaml` as project root marker
- **Expanded project markers** — added VCS (`.git`, `.arc/HEAD`), Rust (`Cargo.toml`), Node.js (`package.json`), Go (`go.mod`), Python (`pyproject.toml`, `setup.py`), C# (`*.sln`) root detection

### 3.21.0
- **PHP support** — full tree-sitter parser for PHP: namespaces, classes (extends/implements), interfaces, traits, enums, functions, methods, constants, properties, `use` imports, trait `use`; file extensions `.php`, `.phtml`

### 3.20.0
- **`.d.ts` indexing from `node_modules`** — Frontend projects automatically index TypeScript type declarations from dependencies; resolves pnpm symlinks safely (no `follow_links` on FUSE mounts)
- **Tree-sitter ambient declarations** — `declare function/class/interface/type/enum/const/namespace` in `.d.ts` files now parsed correctly via tree-sitter queries
- **`search` includes refs** — `search` command now searches the `refs` table, finding library-only symbols (e.g. `useToaster` from `@gravity-ui/uikit`) even when they have no local definition

### 3.19.0
- **`query` command** — execute raw SQL against the index DB with JSON output; enables complex joins, aggregation, and negative queries in a single call (`SELECT`, `WITH`, `EXPLAIN` only — mutations blocked)
- **`db-path` command** — print SQLite database path for direct access from Python, JS, or any language with SQLite support
- **`schema` command** — show all tables with columns and row counts in JSON
- **`agrep` command** — structural code search via ast-grep (`sg`); AST pattern matching with `$NAME`/`$$$` metavariables and `--lang` filter

### 3.18.2
- **Fix `composables` returning 0 results** — `@Composable` and `fun` are typically on separate lines in Kotlin; rewritten to two-phase approach (find files, then multi-line scan) instead of single-line grep callback
- **Fix `previews` returning 0 results** — same multi-line issue as `composables`

### 3.18.1
- **Tree-sitter outline for all languages** — `outline` command now delegates to tree-sitter for Java, TypeScript/JavaScript, Swift, Ruby, Rust, Scala, C#, Proto, ObjC (previously only Dart used tree-sitter, others fell through to Kotlin regex)
- **Module dependencies for extra roots** — `rebuild` now merges module files from extra roots and checks them for build system markers; Maven (`pom.xml`) triggers dependency indexing
- **Fix call-tree nested generics** — regex now handles `Map<String, List<Integer>>` correctly instead of breaking on inner `>`
- **`inject` supports @Autowired** — `inject` command searches for both `@Inject` and `@Autowired` annotations (Spring DI)
- **Partial matching in `implementations`** — `implementations "Service"` now finds implementations of `UserService`, `PaymentService`, etc. via contains matching with relevance ranking
- **Overlap validation for `add-root`** — warns when adding a root inside or parent of project root; use `--force` to override

### 3.18.0
- **Dedicated Java parser** — Java files now use `tree-sitter-java` instead of being routed through the Kotlin parser; indexes classes, interfaces, enums, methods, constructors, fields, and Spring annotations (`@RestController`, `@Service`, `@GetMapping`, etc.)
- **Maven module support** — `pom.xml` files are recognized as module descriptors; `<artifactId>` extracted as module name, `<dependency>` entries matched against local modules
- **Improved call-tree for Java** — regex patterns now detect Java-style method definitions (`void methodName(`, `String methodName(`), `this.method()` and `super.method()` call patterns
- **Updated skill documentation** — added Java/Spring examples, Maven support notes, removed incorrect wildcard syntax

### 3.17.5
- **No marker files** — removed `.ast-index-root` marker; project root detected via existing index DB in cache (zero files in project directory)

### 3.17.4
- **Directory-scoped search** — when running from a subdirectory, results are automatically limited to that subtree

### 3.17.3
- **`--threads` / `-j` flag for rebuild** — control parallel threads (e.g. `-j 32` for network filesystems where I/O is the bottleneck)

### 3.17.2
- **Fix FUSE hang on auto-detection** — `quick_file_count` no longer stat-s `.gitignore`/`.arcignore` per directory, which caused hangs on FUSE-mounted repos

### 3.17.1
- **`--verbose` flag for rebuild** — detailed timing logs for every step (walk, parse, DB write, lock, modules, deps) to diagnose performance issues
- **Removed `init` command** — `rebuild` creates DB from scratch, `init` was redundant
- **SQLite concurrent safety** — `busy_timeout = 5000ms` prevents "database locked" errors; file lock prevents concurrent rebuilds on same project

### 3.17.0
- **Auto sub-projects mode** — `rebuild` automatically switches to sub-projects indexing when directory has 65K+ source files and 2+ sub-project directories
- **`--sub-projects` flag** — explicit sub-projects mode for large monorepos, indexes each subdirectory separately into a single shared DB
- **Extended VCS support** — respects `.gitignore` and `.arcignore` in monorepos without `.git` directory

### 3.16.3
- **FTS5 prefix search fix** — `search` no longer crashes on queries like `SlowUpstream`; prefix `*` operator now correctly placed outside FTS5 quotes
- **Extended VCS support** — `rebuild`/`search`/`grep` now respect `.gitignore` and `.arcignore` in non-git monorepos, preventing hangs on large codebases
- **Fuzzy search fix** — `--fuzzy` flag now returns all matching results (exact + prefix + contains) instead of early-returning on exact match only

### 3.16.0
- **`restore` command** — restore index from a `.db` file: `ast-index restore /path/to/index.db`

### 3.15.0
- **TypeScript class members** — index class methods (constructor, getters/setters, static, async), fields/properties, private `#members`, and abstract methods; object literal methods correctly excluded

### 3.14.0
- **`map` command** — compact project overview: top directories by size with symbol kind counts; `--module` for detailed drill-down with classes and inheritance
- **`conventions` command** — auto-detect architecture patterns, frameworks, and naming conventions from indexed codebase
- **`refs` command** documented in skill

### 3.13.4
- **Android indexing performance** — eliminate 4 redundant filesystem walks during `rebuild`; XML layout files, resource files collected in the main walk, code file usages queried from DB

### 3.13.3
- **iOS indexing performance** — eliminate 3 redundant filesystem walks during `rebuild`; storyboard/xib files and .xcassets directories are now collected in the main walk, swift file asset usages queried from DB instead of a 4th walk

### 3.13.2
- **Fix `rebuild` losing extra roots** — `add-root` paths are now preserved across `rebuild` (previously deleted with DB)

### 3.13.1
- **Fix plugin skill discovery** — added `"skills"` field to `plugin.json`, fixing "Unknown skill: ast-index" error when invoking `/ast-index`

### 3.13.0
- **Scala language support** — tree-sitter parser for class, case class, object, trait, enum (Scala 3), def, val/var, type alias, given
- **Bazel project detection** — `WORKSPACE`, `WORKSPACE.bazel`, `MODULE.bazel` as project root markers
- **4x faster rebuild on non-Android/iOS projects** — skip XML layouts, storyboards, iOS assets, CocoaPods phases when no platform markers present (309s → 83s on 83k files)
- **Git default branch detection** — correctly parse `origin/trunk`, `origin/develop` from symbolic-ref, not just main/master

### 3.12.0
- **Tree-sitter AST parsing for 12 languages** — replaced regex-based parsers with tree-sitter for Kotlin, Java, Swift, ObjC, Python, Go, Rust, Ruby, C#, C++, Dart, Proto, and TypeScript. Parsing is now based on real ASTs instead of regex heuristics — more accurate symbol extraction, correct handling of nested constructs, and fewer false positives
- **Grouped `--help` output** — commands organized into 8 logical categories (Index Management, Search & Navigation, Module Commands, Code Patterns, Android, iOS, Perl, Project Configuration) instead of a flat alphabetical list
- **Updated project description** — "Fast code search for multi-language projects"

### 3.11.2
- **Fix `watch` command on large projects** — switched from kqueue to FSEvents (macOS) / inotify (Linux), fixes "Too many open files" error

### 3.11.1
- **Fix `changed` command** — auto-detect default git branch (`origin/main` or `origin/master`)
- **Fix `api` command** — accept module names with dots (e.g. `module.name` → `module/name`)
- **Updated skill docs** — added `--format json`, `unused-symbols`, `watch`, multi-root commands

### 3.11.0
- **10x faster `unused-deps`** — replaced filesystem scanning (WalkDir + read_to_string) with index-based SQL queries to `refs` table. `core` module (225 deps) now completes in ~6s instead of 60s+ timeout
- **Fixed transitive dependency logic** — correctly checks `transitive_deps` table (api chain reachability) instead of re-scanning symbols
- **Multi-VCS support for `changed`** — auto-detects VCS, auto-selects base branch (`trunk` for arc, `origin/main` for git), normalizes `origin/` prefix
- **Removed skill copying from initialize commands** — `/initialize-*` no longer copies skill files to project directory

### 3.10.4
- **2.6x faster indexing on large projects** — fix Dart parser allocating lines vector per class declaration

### 3.10.2
- **Fix `changed` command** — use `merge-base` instead of direct diff to show only current branch changes
- **Multi-VCS support** — auto-detect arc vs git, use correct VCS commands

### 3.10.1
- **Fix indexing hangs on large monorepos** — disable symlink following, add max depth limit
- **Expanded excluded directories** — added `bazel-out`, `bazel-bin`, `buck-out`, `out`, `.metals`, `.dart_tool` and more
- **Better progress reporting** — output after every chunk instead of every 4th
- **GitHub Actions release workflow** — automated builds for darwin-arm64, darwin-x86_64, linux-x86_64, windows-x86_64

### 3.10.0
- **Dart/Flutter support** — index and search Dart/Flutter codebases
  - Classes with Dart 3 modifiers: `abstract`, `sealed`, `final`, `base`, `interface`, `mixin class`
  - Mixins: `mixin Foo on Bar`
  - Extensions and extension types (Dart 3.3)
  - Enhanced enums with `with`/`implements`
  - Functions, constructors, factory constructors
  - Getters/setters, typedefs, properties
  - Imports/exports
  - Multiline class declarations
  - File types: `.dart`
- **20 new tests** — comprehensive test coverage for Dart parser

### 3.9.3
- **Simplified plugin installation** — `install-claude-plugin` now calls `claude plugin marketplace add` and `claude plugin install` instead of manual file copying
- **Updated README** — plugin install instructions now use official `claude plugin` CLI commands

### 3.9.2
- **Fix OOM crashes on large projects** (70K+ files)
  - Batched indexing: parse and write to DB in chunks of 500 files instead of loading everything into memory
  - Limited rayon thread pool to max 8 threads to cap peak memory
  - Skip files > 1 MB (minified/generated code)
  - Skip lines > 2000 chars in ref parser
  - Truncate ref context to 500 chars (was unbounded — minified JS lines caused 12 GB+ databases)
  - Reduced SQLite cache from 64 MB to 8 MB
- **Hardcoded directory exclusions** — always skip `node_modules`, `__pycache__`, `build`, `dist`, `target`, `vendor`, `.gradle`, `Pods`, `DerivedData`, `.next`, `.nuxt`, `.venv`, `.cache` etc. regardless of `.gitignore`
- **New project type detection** — Frontend (`package.json`), Python (`pyproject.toml`), Go (`go.mod`), Rust (`Cargo.toml`)
- **LazyLock regex** — all 146 regex compilations cached via `std::sync::LazyLock` (was recompiling per file)

### 3.9.1
- **Performance fix** — grep-based commands now use early termination
  - Commands like `deeplinks`, `todo`, `callers` etc. stop scanning after finding `limit` results
  - Up to 100-1000x faster on large codebases (29k files: 4-35s → 10-50ms)

### 3.9.0
- **TypeScript/JavaScript support** — index and search web projects
  - React: components, hooks (useXxx), JSX/TSX
  - Vue: SFC script extraction, defineComponent
  - Svelte: component props extraction
  - NestJS/Angular: decorators (@Controller, @Injectable, @Component)
  - Node.js: ES modules, CommonJS
  - File types: `.ts`, `.tsx`, `.mts`, `.js`, `.jsx`, `.mjs`, `.cjs`, `.vue`, `.svelte`
- **Rust support** — index and search Rust codebases
  - Structs, enums, traits, impl blocks
  - Functions, macros, type aliases
  - Derive attributes tracking
  - File types: `.rs`
- **Ruby support** — index and search Ruby/Rails codebases
  - Classes, modules, methods
  - RSpec DSL (describe, it, let, context)
  - Rails: associations, validations, scopes, callbacks
  - File types: `.rb`
- **C# support** — index and search .NET projects
  - Classes, interfaces, structs, records
  - ASP.NET: controllers, HTTP attributes
  - Unity: MonoBehaviour, SerializeField
  - File types: `.cs`
- **Explore agent** — deep code investigation with confirmations
- **Review agent** — change analysis with impact assessment
- **63 tests** — comprehensive test coverage for all parsers

### 3.8.5
- **Documentation** — added troubleshooting section for brew install merge conflict errors

### 3.8.2
- **Plugin improvements**
  - Added C++, Protocol Buffers, and WSDL/XSD reference documentation
  - Added "Critical Rules" section to SKILL.md for better Claude integration
  - Initialize commands now copy skill documentation to project `.claude/` directory
  - Updated plugin description to include all supported languages

### 3.8.1
- **search command fix** — `-l/--limit` parameter now correctly limits file results
- **Content search** — `search` command now also searches file contents (not just filenames and symbols)

### 3.8.0
- **Python support** — index and search Python codebases
  - Index: `class`, `def`, `async def`, decorators
  - Imports: `import module`, `from module import name`
  - File types: `.py`
  - `outline` and `imports` commands work with Python files
- **Go support** — index and search Go codebases
  - Index: `package`, `type struct`, `type interface`, `func`, methods with receivers
  - Imports: single imports and import blocks
  - File types: `.go`
  - `outline` and `imports` commands work with Go files
- **Performance** — `deeplinks` command 200x faster (optimized pattern)

### 3.7.0
- **call-tree command** — show complete call hierarchy going UP (who calls the callers)
  - `ast-index call-tree "functionName" --depth 3 --limit 10`
  - Works across Kotlin, Java, Swift, Objective-C, and Perl
- **--no-ignore flag** — index gitignored directories like `build/`
  - `ast-index rebuild --no-ignore`
  - Useful for finding generated code like `BuildConfig.java`

### 3.6.0
- **Perl support** — index and search Perl codebases
  - Index: `package`, `sub`, `use constant`, `our` variables
  - Inheritance: `use base`, `use parent`, `@ISA`
  - File types: `.pm`, `.pl`, `.t`, `.pod`
  - New commands: `perl-exports`, `perl-subs`, `perl-pod`, `perl-tests`, `perl-imports`
  - Grep commands now search Perl files: `todo`, `callers`, `deprecated`, `annotations`
  - `imports` command now parses Perl `use`/`require` statements
  - Perl packages indexed as modules for `module` command
  - Project detection: `Makefile.PL`, `Build.PL`, `cpanfile`

### 3.5.0
- **Renamed to ast-index** — project renamed from `kotlin-index`
  - New CLI command: `ast-index` (was `kotlin-index`)
  - New Homebrew tap: `defendend/ast-index` (was `defendend/kotlin-index`)
  - New repo: `Claude-ast-index-search` (was `Claude-index-search-android-studio`)

### 3.4.1
- **Fix grep-based commands for iOS** — 6 commands now work with Swift/ObjC:
  - `todo` — search in .swift/.m/.h files
  - `callers` — support Swift function call patterns
  - `deprecated` — support `@available(*, deprecated)` syntax
  - `annotations` — search in Swift/ObjC files (@objc, @IBAction, etc.)
  - `deeplinks` — add iOS patterns (openURL, CFBundleURLSchemes, NSUserActivity)
  - `extensions` — support Swift `extension Type` syntax

### 3.4.0
- **iOS storyboard/xib analysis** — `storyboard-usages` command to find class usages in storyboards and xibs
- **iOS assets support** — index and search xcassets (images, colors), `asset-usages` command with `--unused` flag
- **SwiftUI commands** — `swiftui` command to find @State, @Binding, @Published, @ObservedObject properties
- **Swift concurrency** — `async-funcs` for async functions, `main-actor` for @MainActor usages
- **Combine support** — `publishers` command to find PassthroughSubject, CurrentValueSubject, AnyPublisher
- **CocoaPods/Carthage** — detect and index dependencies from Podfile and Cartfile

### 3.3.0
- **iOS/Swift/ObjC support** — auto-detect project type and index Swift/ObjC files
- Swift: class, struct, enum, protocol, actor, extension, func, init, var/let, typealias
- ObjC: @interface, @protocol, @implementation, methods, @property, typedef, categories
- SPM module detection from Package.swift (.target, .testTarget, .binaryTarget)
- Inheritance and protocol conformance tracking for Swift/ObjC

### 3.2.0
- Add `xml-usages` command — find class usages in XML layouts
- Add `resource-usages` command — find resource usages (drawable, string, color, etc.)
- Add `resource-usages --unused` — find unused resources in a module
- Update `unused-deps` with transitive dependency checking (via api deps)
- Update `unused-deps` with XML layout usage checking
- Update `unused-deps` with resource usage checking
- New flags: `--no-transitive`, `--no-xml`, `--no-resources`, `--strict`
- Index XML layouts (5K+ usages in large Android projects)
- Index resources (63K+ resources, 15K+ usages)
- Build transitive dependency cache (11K+ entries)

### 3.1.0
- Add `unused-deps` command — find unused module dependencies
- Module dependencies now indexed by default (use `--no-deps` to skip)

### 3.0.0 (Rust)
- **Major release** — complete Rust rewrite, replacing Python version
- 26 of 33 commands faster than Python
- Top speedups: imports (260x), dependents (100x), deps/class (90x)
- Full index with 900K+ references
- Fixed `hierarchy` multiline class declarations
- Fixed `provides` Java support and suffix matching

### Python versions (1.0.0 - 2.5.2)

> Legacy Python code archived in `legacy-python-mcp/` folder

#### 2.5.2
- Project-specific databases: Each project now has its own index database

#### 2.5.1
- Use ripgrep for 10-15x faster grep-based searches

#### 2.5.0
- Add `composables`, `previews`, `suspend`, `flows` commands

#### 2.4.1
- Fix `callers`, `outline`, `api` commands

#### 2.4.0
- Add `todo`, `deprecated`, `suppress`, `extensions`, `api`, `deeplinks` commands

#### 2.3.0
- Add `callers`, `imports`, `provides`, `inject` commands

#### 2.2.0
- Add `hierarchy`, `annotations`, `changed` commands

#### 2.1.0
- Fix `class` command, add `update` command

#### 2.0.0
- pip package, CLI with typer + rich, Skill for Claude Code, MCP server

#### 1.2.0
- Java support (tree-sitter-java), Find Usages, Find Implementations

#### 1.1.0
- Incremental indexing, better module detection

#### 1.0.0
- Initial release: File/symbol/module search, MCP server

## License

MIT
