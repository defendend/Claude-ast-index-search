# ast-index v3.49.0

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

Use `ast-index update` after edits or branch switches. In monorepos with nested
project markers, add `--walk-up` or `AST_INDEX_WALK_UP=1` to reuse the root
index.

**Guides:** [User guide](USER_GUIDE.md) for everyday workflow;
[Command setup guide](docs/setup-guide.md) for install/options/examples.

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
ast-index changed [--base BRANCH]  # Changed symbols
ast-index todo [PATTERN]           # TODO/FIXME/HACK comments
ast-index deprecated [QUERY]       # Deprecated items
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
