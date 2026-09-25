---
name: ast-index
description: This skill should be used when the user asks to "find a class", "search for symbol", "find usages", "find implementations", "search codebase", "find file", "class hierarchy", "find callers", "module dependencies", "unused dependencies", "project map", "project conventions", "project structure", "what frameworks", "what architecture", "find Perl subs", "Perl exports", "find Python class", "Go struct", "Go interface", "find React component", "find TypeScript interface", "find Rust struct", "find Ruby class", "find C# controller", "find Dart class", "find Flutter widget", "find mixin", "find Scala trait", "find case class", "find object", "find PHP class", "find Laravel model", "find PHP trait", or needs fast code search in Android/Kotlin/Java, iOS/Swift/ObjC, Dart/Flutter, TypeScript/JavaScript, Rust, Ruby, C#, Scala, PHP, Perl, Python, Go, C++, or Protocol Buffers projects. Also triggered by mentions of "ast-index" CLI tool.
user-invocable: false
---

# ast-index - Code Search for Multi-Platform Projects

Fast native Rust CLI for structural code search in Android/Kotlin/Java, iOS/Swift/ObjC, Dart/Flutter, TypeScript/JavaScript, Rust, Ruby, C#, Scala, PHP, Perl, Python, Go, C++, and Proto projects using SQLite + FTS5 index.

## Critical Rules

**ALWAYS use ast-index FIRST for any code search task.** These rules are mandatory:

1. **ast-index is the PRIMARY search tool** — use it before grep, ripgrep, or Search tool
2. **Pick the command by what you know:**
   - You have an intent or a description ("how is auth handled", "processing update admin") → `ast-index explore "<query>"`. It ranks by relevance and prints the source.
   - You have an exact identifier (`UserService`, `parseConfig`) → `ast-index search` / `symbol` / `class`.
   - `search` itself falls back to `explore` ranking when a multi-word query has no literal match, so a wrong pick is not fatal — but `explore` is the right first call for questions.
3. **DO NOT duplicate results** — if ast-index found usages/implementations, that IS the complete answer
4. **DO NOT run grep "for completeness"** after ast-index returns results
5. **Use grep/Search ONLY when:**
   - ast-index returns empty results
   - Searching for regex patterns (ast-index uses literal match)
   - Searching for string literals inside code (`"some text"`) — `usages` and
     `refs` skip names inside strings and comments
   - Searching in comments content

**Why:** ast-index is 17-69x faster than grep (1-10ms vs 200ms-3s) and returns structured, accurate results.

## Prerequisites

Install the CLI before use:

```bash
brew tap defendend/ast-index
brew install ast-index
```

Initialize index in project root:

```bash
cd /path/to/project
ast-index rebuild
```

The index is stored at `~/Library/Caches/ast-index/<project-hash>/index.db` (macOS) or `~/.cache/ast-index/<project-hash>/index.db` (Linux). Rebuild builds a fresh index and swaps it in; named subtrees and the collected Git history (`hotspots --collect`) are carried over.

## Supported Projects

| Platform | Languages | Module System |
|----------|-----------|---------------|
| Android/Java | Kotlin, Java | Gradle (build.gradle.kts), Maven (pom.xml) |
| iOS | Swift, Objective-C | SPM (Package.swift) |
| Web | TypeScript, JavaScript, React, Vue, Svelte | package.json |
| Rust | Rust | Cargo.toml |
| Ruby | Ruby, Rails, RSpec | Gemfile |
| .NET | C#, ASP.NET, Unity | *.csproj |
| Dart/Flutter | Dart | pubspec.yaml |
| Scala | Scala | Bazel (WORKSPACE, BUILD) |
| PHP | PHP | composer.json |
| Perl | Perl | Makefile.PL, Build.PL |
| Python | Python | None (*.py files) |
| Go | Go | None (*.go files) |
| Proto | Protocol Buffers (proto2/proto3) | None (*.proto files) |
| WSDL | WSDL, XSD | None (*.wsdl, *.xsd files) |
| C/C++ | C, C++ (JNI, uservices) | None (*.cpp, *.h, *.hpp files) |
| Godot | GDScript | project.godot |
| Mixed | All above | All |

Project type is auto-detected by marker files (build.gradle.kts, Package.swift, Makefile.PL, etc.). Python, Go, Proto, WSDL, and C++ files are indexed alongside main project type.

Minified JavaScript/CSS is never indexed or searched: `.js`/`.mjs`/`.cjs`/`.css` files named `*.min.*` or `*-min.*`, or whose first 64 KiB is minifier output (lines averaging 1000+ bytes of code, not one long string). Grep-based commands skip them too, and `outline`/`imports` on such a file print `Skipped: minified file` instead of parsing it. `AST_INDEX_SKIP_MINIFIED=0` turns the filter off.

## Core Commands

### Explore (one-shot context)

**`explore`** - Rank the symbols most relevant to a query, show the best files
— an outline with line ranges for a type or module (the definitions inside it,
the chosen one marked `→`), the source (read fresh from disk) for a function —
list graph neighbours (callers/subclasses), and locate tests by path convention
— in a single call. Prefer this over a search + read loop when you want to
understand an area; read the slice an outline row points at next.
Language-agnostic and vendor-aware (`node_modules` `.d.ts` and cross-stack
matches are down-ranked, never deleted).

```bash
ast-index explore applicant merge MergeService   # bag of symbol/file names
ast-index explore "how does auth session work"   # natural-language question
ast-index explore PaymentService --rwr           # --rwr: re-rank via call/inheritance graph
ast-index explore Repository --max-files 8        # cap source files shown (default 6)
ast-index explore Session request --rwr --format json
```

- Default (Stage A) ranks by lexical match + multi-term corroboration. The
  words of a question are also read run together (`pdf to html service` finds
  `PdfToHtmlService`) and against file paths, so a CamelCase class is found
  from its words; the type a file is named after ranks above helpers, and
  statements (`has_many :x`, `scope`, `include`) and namespace-only modules
  rank below definitions. Question words (`how`, `does`, `work`) are ignored.
- `--rwr` (Stage B) builds a call/inheritance graph in memory and re-ranks by
  personalized PageRank, surfacing callers/subclasses in a "Graph neighbours"
  section. Slightly slower; best when you care about who-calls-what.

### Universal Search

**`search`** - Perform universal search across files, symbols, and modules simultaneously.

```bash
ast-index search "Payment"           # Finds files, classes, functions matching "Payment"
ast-index search "ViewModel"         # Returns files, symbols, modules in ranked order
ast-index search "Store" --fuzzy     # Fuzzy: exact → prefix → contains matching
ast-index search "Handler" --module "core/"  # Search within a module
ast-index search "UserService"       # Find Java/Spring services
ast-index search "@RestController"   # Find Spring REST controllers (annotation search)
ast-index search "@GetMapping"       # Find GET endpoint mappings
```

Symbols come in tiers: exact name, then a name whose last `::` or `.` segment
is the query (`Billing::Invoice` for `Invoice`, the schema column `users.email`
for `email`), then partial matches. Inside each
tier definitions come before imports and project code before `node_modules`;
among partial matches test symbols (test files, `test_*`, `TestX`) come last.

**`search --rank <preset>`** - Re-rank the Files and Symbols sections by the
file's Git history and the symbol's place in the dependency graph, with a
dossier next to every result explaining its position. Use it when the question
is not "where is X" but "which of these X":

```bash
ast-index search Service --fuzzy --module app/services/ --rank proven  # a settled example to copy
ast-index search Merge --rank risky                   # dangerous to touch: many dependents + unstable history
ast-index search Import --rank hotspots               # keeps being changed and fixed
ast-index search Import --rank hotspots --exclude-tests  # same, spec/test files left out
ast-index search Event --module app/models/ --rank central  # what the rest leans on (PageRank)
ast-index --format json search Merge --rank risky     # rank.applied / rank.missing + per-result dossier
```

| Preset | Score | Needs |
|--------|-------|-------|
| `proven` | mean(1 − hotspot score, maturity, used 1/0) × substance (0.5 for stubs) × lineage (0.5 when the base is no longer extended) | `hotspots --collect` + `graph build` |
| `hotspots` | file hotspot score (commits, churn, bugfix ratio pct) | `hotspots --collect` |
| `risky` | dependents pct × hotspot score | both |
| `central` | PageRank pct | `graph build` |

- Relevance stays in charge: the pool is the top 100 project symbols of the
  plain order, exact-name matches stay above partial ones, and inside a tier
  the key is `0.9 × score + 0.1 × 1/(1 + position/20)`. File path matches use
  where the match sits (stem, name, directory) as the relevance term.
- **History is per file** — every symbol in a file shares it ("file history"
  in the output). Graph numbers are per symbol; a file result borrows them from
  its strongest symbol.
- Missing data is never ranked as zeros: the preset is not applied, results
  keep plain order, and the output names the command to run
  (`rank.missing[].command` in JSON). A stale graph ranks with a warning.
- Third-party code (`node_modules`, `.d.ts`) is never scored and is listed
  after all project results.
- Formulas were picked by backtesting next-year bugfixes on a 40k-file
  monorepo, `proven` also by hand-judged "which one to copy" queries; the
  numbers are in USER_GUIDE.md ("Ranking search results"). Safe by the numbers
  is not the same as a good example: `proven` cannot tell which of two living
  styles the team prefers.

### File Search

**`file`** - Find files by name pattern.

```bash
ast-index file "Fragment.kt"         # Find files ending with Fragment.kt
ast-index file "ViewController"      # Find iOS view controllers
```

### Symbol Search

**`symbol`** - Find symbols (classes, interfaces, functions, properties) by name.

```bash
ast-index symbol "PaymentInteractor" # Find exact symbol
ast-index symbol "Presenter"         # Find all presenters
ast-index symbol "Store" --fuzzy     # Fuzzy: exact → prefix → contains matching
ast-index symbol "Mapper" --in-file "payments/" --limit 10  # Scoped search
ast-index symbol "@Service"          # Find all @Service annotations
```

A namespaced class is found by its short or its full name: `LedgerImporter` and
`Billing::LedgerImporter` both find `class Billing::LedgerImporter` (also in
`class`, `refs`, `hierarchy`, `implementations`). An exact short name wins
over namespaced ones. `usages Billing::LedgerImporter` lists references to
`LedgerImporter` on lines that spell out the full name; `usages LedgerImporter`
lists all of them.

### Class Search

**`class`** - Find class, interface, or protocol definitions.

```bash
ast-index class "BaseFragment"       # Find Android base fragment
ast-index class "UIViewController"   # Find iOS view controller subclass
ast-index class "Store" --fuzzy      # Find all classes containing "Store"
ast-index class "Repository" --module "features/payments"  # Filter by module
ast-index class "UserController"     # Find Java/Spring controller class
```

### Usage Search

**`usages`** - Find all places where a symbol is used. Critical for refactoring.

```bash
ast-index usages "PaymentRepository" # Find all usages of repository
ast-index usages "onClick"           # Find all click handler usages
ast-index usages "fetchData" --in-file "src/api/"  # Scoped to file path
ast-index usages "Repository" --module "features/auth" --limit 100
ast-index usages "parse_config"      # snake_case calls: Python, Rust, Go, C, PHP, ...
```

Usages (here and in `refs`) list production files first and test files after
them, each group by path and line; in JSON a test reference carries
`"test": true`.

Indexed usages are capitalized names and calls written `name(`, snake_case and
`_private` names included. Reserved words (`sizeof (x)`, `#if defined(X)`, Go's
`func (r *T)`, Python's `None`) are never recorded.

Performance: ~8ms for indexed symbols.

### Cross-References

**`refs`** - Show cross-references for a symbol: definitions, imports, and usages in one view.

```bash
ast-index refs "PaymentRepository"   # Definitions + imports + usages
ast-index refs "BaseFragment" --limit 10  # Limit results per section
```

### Implementation Search

**`implementations`** - Find all classes that extend or implement a given class/interface/protocol. Supports partial name matching with relevance ranking (exact → suffix → contains).

```bash
ast-index implementations "BasePresenter"  # Find all presenter implementations
ast-index implementations "Repository"     # Find repository implementations (exact match)
ast-index implementations "Service"        # Partial: finds UserService, PaymentService impls too
ast-index implementations "ViewModel" --module "features/"  # Scoped to module
```

### Class Hierarchy

**`hierarchy`** - Display complete class hierarchy tree.

```bash
ast-index hierarchy "BaseFragment"   # Show fragment inheritance tree
```

### Caller Search

**`callers`** - Find the lines that call a function, matched by name at query time (`name(`, `.name`, …). It prints call sites, not the function each one sits in — use `call-tree` for that.

```bash
ast-index callers "onClick"          # Find all onClick calls
ast-index callers "fetchUser"        # Find API call sites
```

A Ruby symbol naming the method counts as a call site (`before_save :name`,
`validate :name`, `delegate :name`, `map(&:name)`); `:name?`, `:name!` and
`:name=` name other methods and do not, and neither does a `::name` path
(`use super::name;`, `Billing::Name`) unless it is called (`Mod::name(`,
`Billing::Name.new`).

### Call Tree

**`call-tree`** - Show the call hierarchy going UP: the function each call sits in, then its callers. Works in every indexed language: the caller is the innermost definition whose line range encloses the call for the tree-sitter languages (Ruby, TypeScript/JavaScript, Python, Go, Rust, Java, Kotlin, Swift, C#, C/C++, PHP, …); the regex-based parsers without ranges (Perl, WSDL/XSD) fall back to the nearest definition above the call.

```bash
ast-index call-tree "processPayment" --depth 3 --limit 10
ast-index call-tree "getUsers"       # Java: finds callers of getUsers() method
```

Every caller is printed with its file, including same-named definitions of
other files (two `it "works"` blocks are two callers). Callers are looked up
by name, so each name is expanded once: a later caller of that name is marked
`(expanded above)`, and `(recursive)` marks only a definition already on its
own path — a real cycle. Imports and annotations (`use`, `include Mod`, Rails
callbacks) never own a call — the definition around them does — and the line
declaring the function itself is not a call of it.

### Symbol Dependency Graph

**`graph`** - A directed symbol-to-symbol graph built from the index: an edge
means "the definition containing a reference" -> "the definition that
reference names". Unlike `callers` / `call-tree` (text search at query time)
it answers from precomputed edges and says how sure it is about each target.

```bash
ast-index graph build                                 # Build/refresh the graph (explicit, ~seconds)
ast-index graph status                                # Built? Stale? Edge counts per confidence
ast-index graph dependents ApplicationService         # Who depends on it (incoming edges)
ast-index graph dependents "Billing::Invoice#total"   # A member of one class
ast-index graph dependents MergeService --exclude-tests  # Production dependents only (also on impact)
ast-index graph dependencies CheckoutController --members  # What a class and its methods use
ast-index graph impact PaymentGateway --depth 3       # Blast radius: dependents per depth, files
ast-index graph path OrdersController Invoice         # Shortest dependency path(s) between two symbols
ast-index graph cycles --path app/models              # Strongly connected components
ast-index graph top --kind class --exclude-tests      # Most central symbols (PageRank)
ast-index graph metrics Invoice Payment               # fan-in/out, dependents, PageRank per symbol
```

Every edge carries the level at which its target was resolved:
`local` (same file), `scoped` (explicit namespace `A::B::Name`, lexical
nesting, a constant receiver `Type.method`, or inheritance/mixins), `import`
(the source file imports the target's module), `unique` (only definition of
that name in the language), `ambiguous` (several candidates, or a call on a
receiver of unknown type — `candidates` says how many). Fan-in, fan-out,
dependents and PageRank count **resolved edges only**; ambiguous edges are
counted separately and listed with `--include-ambiguous` (on `impact` that
gives an upper bound next to the resolved-only number).

Rust paths resolve like the compiler reads them: a file is a module
(`src/db.rs` is `db`), `crate::` / `super::` / `self::` walk the module tree,
and `use` declarations (grouped, aliased, globbed, `pub use` re-exports) bind
names, so `db::open_db(...)` is a `scoped` edge to `src/db.rs` and a name
imported with `use` an `import` edge. Another workspace crate is reached by
its `Cargo.toml` library name; `std::` and dependency paths never resolve to
a same-named project definition.

A class symbol only owns its class-level references (superclass, mixins);
pass `--members` to `dependents` / `dependencies` / `impact` to cover the
definitions inside it. `path` always treats a class as itself plus its
members and may step from a class into a member (shown as `contains`),
because dispatch like `Service.call` -> `process` is not statically visible.

A bare name can match many definitions (`call`, or `Applicant` as a model,
TypeScript types and spec stubs): `dependents` / `dependencies` / `impact`
then merge their edges, say how many definitions matched, list at most
`--limit` of them and suggest `Outer::Name`, `Class#member`, `--in-file` or
`--kind`.

In a Rails app the tables of `db/schema.rb` are matched to models
(`self.table_name`, single-table inheritance, `table_name_prefix` /
`isolate_namespace`, nesting, the pluralized class name), and a column reader
or attribute method called inside the model (`status`, `self.status`,
`status?`, `saved_change_to_status?`) resolves as a `scoped` edge to the
column: `ast-index graph dependents applicants.first_name` (or
`applicants#first_name`). A call on another receiver (`applicant.first_name`)
never guesses a column, so most reads of a column are not its edges — the
answer says so; `ast-index usages first_name` lists every read. `graph build`
reports tables without a model and models without a table.

The graph is not rebuilt by `rebuild` / `update`. After an update changes the
index, queries print a stale warning (`"stale": true` in JSON); rerun
`graph build` or add `--refresh` to a query to rebuild first.

### File Analysis

**`imports`** - List all imports in a specific file.

```bash
ast-index imports "path/to/File.kt"  # Show Kotlin file imports
```

**`outline`** - Show all symbols defined in a file, parsed the way the index parses it (tree-sitter for most languages).

```bash
ast-index outline "PaymentFragment.kt"    # Show Kotlin fragment structure
ast-index outline "UserController.java"   # Show Java controller methods
ast-index outline "App.tsx"               # Show TypeScript/React components and functions
ast-index outline "handler.rs"            # Show Rust structs, impls, functions
ast-index outline --format json "app.rb"  # {schema_version: 1, file, symbols: [{name, kind, line, end_line}]}
```

Rows are in source order. A definition spanning several lines prints its range
(`:12-40 Invoice [class]`), so `Read` can take exactly that slice; a one-line
definition prints `:12`. `end_line` is `null` in JSON where the parser reports
no range.

In a schema dump (`db/schema.rb`) the columns are folded into their table's
row (`:96-149 orders [table] 33 columns`) so the outline stays small; `--full`
lists every column, and `symbol --type column --pattern 'users.*'` lists one
table's.

### Code Quality

**`todo`** - Find TODO/FIXME/HACK comments in code.

```bash
ast-index todo                           # Find all TODO comments
ast-index todo --limit 10                # Limit results
```

**`deprecated`** - Find @Deprecated annotations.

```bash
ast-index deprecated                     # Find deprecated items
```

**`unused-symbols`** - Find potentially unused exported symbols.

```bash
ast-index unused-symbols --module path/to/module   # In specific module
ast-index unused-symbols --export-only             # Only exported (public) symbols
```

### Git/Arc Integration

**`changed`** - Fast branch-level file summary from the detected Git/Arc
repository. It compares `merge-base(base, HEAD)` to `HEAD` and reports
repository-relative paths with `A` (added), `M` (modified), `D` (deleted), or
`R` (renamed) status. Without `--base`, Git resolves `origin/HEAD`, then tries
`origin/main`, `origin/master`, `main`, `master`, and `trunk`; Arc uses
`trunk`. For renames, JSON includes `old_path`.

```bash
ast-index changed                              # Auto-detect Git base; Arc uses trunk
ast-index changed --base main                  # Explicit base branch
ast-index --format json changed --base trunk   # Stable JSON schema v1
ast-index changed --timeout-ms 30000           # Bound VCS execution time
ast-index changed --verbose                    # Exact VCS argv and timing on stderr
```

Example text output:

```text
Changed files against main (5):
  A  src/new.rs
  M  src/lib.rs
  D  src/obsolete.rs
  R  src/old.rs -> src/current.rs
  M  src/generated\nname.rs
```

Text mode escapes control characters and backslashes so each result stays on
one line. Scripts should request JSON instead of parsing the text summary.

Example JSON output:

```json
{
  "schema_version": 1,
  "vcs": "git",
  "base": "main",
  "head": "HEAD",
  "scope": null,
  "changes": [
    { "status": "M", "path": "src/lib.rs" },
    { "status": "R", "path": "src/current.rs", "old_path": "src/old.rs" }
  ]
}
```

At repository root `scope` is `null`; in a nested working directory it is the
repository-relative directory path.

`changed` reads VCS state directly and does not depend on the ast-index
database/cache. Its scope is the current working directory, but output paths
remain relative to the repository root. Use it to inventory branch files
before review; use raw `git diff` / `arc diff` for patch hunks. It does not
report changed symbols or include staged/unstaged working-tree-only edits.

**`hotspots`** - Rank files by what their Git history says about them: commit
count, churn (added + deleted lines, and churn relative to the file's current
size for files of 10+ lines), bugfix share, distinct authors, age, and time
since the last change.
Use it when choosing which of several similar files to copy a pattern from, or
when scoping a refactor: a file with 28 commits and 54% bugfixes is not the
same as one written once and untouched for 86 days.

Thresholds are **percentiles within this repository**, not constants — 28
commits is a lot for a library and unremarkable in a monorepo. Raw numbers are
always printed next to the label.

```bash
ast-index hotspots --collect                   # Read new Git history, then report
ast-index hotspots --limit 50 --sort fixes     # Report only; no Git subprocess
ast-index hotspots --path src/parsers          # Narrow output; percentiles stay global
ast-index hotspots --exclude-tests             # Hide spec/test files; percentiles stay global
ast-index hotspots --collect --full            # Discard the cursor, rescan everything
ast-index --format json hotspots --limit 10    # Paginated JSON schema v2
```

Collection is never implicit: `rebuild` and `update` do not run it. The first
`--collect` walks the whole history into a per-commit store; later runs move it
to the current `HEAD` by set difference — commits `HEAD` no longer reaches
(branch switch, rebase, reset, force-push) are subtracted, new ones are added —
so switching branches costs time proportional to the commits that differ, and
switching back re-reads nothing. The numbers equal a full recollection at that
`HEAD`; line counts come from the working tree, so an uncommitted edit counts
once a later `--collect` recomputes the file or on `--full`. Only a
garbage-collected cursor commit, a changed project root or `--full` rebuild
from scratch. `rebuild` keeps the collected history (it
does not depend on the index) unless it came from an older version or another
working tree or scope; then the next `--collect` reads it again.

Labels: `churn:high` / `churn:elevated`, `rewritten-often` (relative churn,
only for files of 10+ lines: below that a line count no longer measures
content — one-line bundles, fixtures, gutted views), `fixes:high` /
`fixes:elevated` (only for files with 4+ commits), `authors:many`, `veteran`.
`--sort` takes `score` (default), `commits`, `churn`, `relative-churn`,
`fixes`, `authors`, `recent`. `score` is printed rounded; the order (and
`score_exact` in JSON) uses the unrounded mean of the percentiles, so the top
of a large repository does not collapse into ties. `fixes` orders by the
bugfix share discounted for thin history (lower bound of its 95% Wilson
interval: 11 of 17 above 2 of 2), files under 4 commits last. Bugfix
detection is a commit-subject heuristic (English and Russian; leading tracker
keys are stripped first, tags such as `[HOTFIX]` / `[FIX]` count as fixes);
merge commits are excluded and renames carry history onto the new path.

### Public API

**`api`** - Show public API of a module. Accepts module path or module name (dots converted to slashes).

```bash
ast-index api "path/to/module"           # By path
ast-index api "module.name"              # By module name (dots → slashes)
```

## Project Insights

### Project Map

**`map`** - Show compact project overview: top directories by size with symbol kind counts. Use `--module` to drill down into a specific area with full class/inheritance details.

```bash
ast-index map                                # Summary: top 50 dirs with kind counts (~54 lines)
ast-index map --limit 20                     # Show only top 20 directories
ast-index map --module features/payments     # Detailed: classes with inheritance for a module
ast-index map --module src/core --per-dir 10 # More symbols per directory in detailed mode
ast-index map --format json                  # JSON output
```

Summary mode output (default, no `--module`):
```
Project: Android (Kotlin/Java) | 29144 files | 859 modules | top 50 of 728 dirs

  features/taxi_order/impl/          1626 files | 371 iface, 94 obj, 1834 cls
  features/masstransit/impl/          862 files | 165 obj, 1261 cls, 280 iface
```

Detailed mode output (with `--module`):
```
features/payments/impl/ (250 files)
  PaymentInteractor : class > BaseInteractor
  PaymentRepository : interface
  PaymentMapper : class
```

### Project Conventions

**`conventions`** - Auto-detect architecture patterns, frameworks, and naming conventions from the indexed codebase. Runs read-only SQL queries — no file scanning needed.

```bash
ast-index conventions                        # Text output (~30 lines)
ast-index conventions --format json          # JSON output
```

Detects:
- **Architecture**: Clean Architecture, Feature-sliced, BLoC, MVC, MVVM, MVP, Redux, Composition API, Hooks
- **Frameworks**: DI (Hilt, Dagger, Koin), Async (Coroutines, RxJava, Combine), Network (Retrofit, OkHttp), DB (Room, Realm, ActiveRecord, Sequel), UI (Compose, SwiftUI, React, Flutter), Testing (JUnit, Kotest, XCTest, pytest, Jest, RSpec), Web (Rails, Django, Flask, FastAPI, Express), Jobs (Sidekiq, Celery) — from import and `require` names, matched on whole name segments (`sequel-combine` is not Combine)
- **Naming patterns**: ViewModel, Repository, UseCase, Service, Controller, Fragment, etc. (with counts)

## Common Flags

Most search commands (`search`, `symbol`, `class`, `usages`, `implementations`, `refs`) support:

| Flag | Description |
|------|-------------|
| `--fuzzy` | Fuzzy search: exact match → prefix match → contains match (`search`, `symbol`, `class` only) |
| `--in-file <PATH>` | Filter results by file path (substring match; also on `callers` and `call-tree`) |
| `--module <PATH>` | Filter results by path prefix |
| `--limit <N>` | Max results to return |
| `--format json` | JSON output for structured processing |

**When to use `--fuzzy`:** When you don't know the exact name. `ast-index class "Store" --fuzzy` finds `Store`, `StoreImpl`, `AppStore`, `DataStoreRepository`, etc. The `--fuzzy` flag performs three-stage matching: exact match first, then prefix match, then contains match.

## Index Management

```bash
ast-index rebuild                        # Full rebuild with dependencies
ast-index rebuild --no-deps              # Skip module dependency indexing
ast-index rebuild --no-ignore            # Include gitignored files (build/, etc.)
ast-index rebuild --sub-projects         # Index each sub-project separately (large monorepos)
ast-index rebuild -j 32                  # Use 32 threads (for network/FUSE filesystems)
ast-index rebuild -v                     # Verbose logging with timing per step
ast-index update                         # Incremental update (changed files only)
ast-index stats                          # Show index statistics
ast-index clear                          # Delete index for current project
ast-index restore /path/to/index.db      # Restore index from a .db file
ast-index watch                          # Watch for file changes and auto-update index
```

### Directory-Scoped Search

When running search from a subdirectory, results are automatically limited to that subtree (the index DB in cache is used to detect the project root):

```bash
cd /project/root
ast-index rebuild                        # Index entire project
ast-index search "Payment"              # Finds results across entire project

cd /project/root/services/payments
ast-index search "Payment"              # Only finds results within services/payments/
ast-index class "PaymentService"        # Only classes in services/payments/ subtree
```

This works with all search commands: `search`, `symbol`, `class`, `implementations`, `usages`.

## Multi-Root Projects

Attach named subtrees for workspaces that intentionally span sibling source trees. They are indexed alongside the primary project, which must be indexed first:

```bash
ast-index subtree add shared ../shared-library   # Attach a named subtree
ast-index update                                 # Index its files
ast-index subtree list                           # List attached subtrees
ast-index subtree remove shared                  # Detach, then rebuild to drop its files
ast-index --subtree shared search "Payment"      # Query one subtree
ast-index --local search "Payment"               # Query the primary project only
```

`add-root`, `remove-root` and `list-roots` remain as compatibility aliases. Do not attach one git worktree to another: each worktree gets its own index.

## Utility Commands

```bash
ast-index version                    # Show CLI version
ast-index help                       # Show help message
ast-index help <command>             # Show help for specific command
ast-index install-claude-plugin      # Install Claude Code plugin to ~/.claude/plugins/
```

## Programmatic Access (SQL & SDK)

For complex analysis that requires combining multiple queries, filtering, or aggregation — use direct SQL access instead of chaining multiple commands.

### Structural Search (ast-grep)

Structural code search using AST patterns. Requires `sg` (ast-grep) installed.

```bash
# Simple pattern matching
ast-index agrep "router.launch($$$)" --lang kotlin
ast-index agrep "@Inject constructor($$$)" --lang kotlin
ast-index agrep "suspend fun $NAME($$$)" --lang kotlin

# Find classes extending a specific base
ast-index agrep "class $NAME : BasePresenter($$$)" --lang kotlin

# Find async functions (Swift)
ast-index agrep "func $NAME($$$) async" --lang swift

# Find React components
ast-index agrep "export default function $NAME($$$)" --lang typescript

# JSON output for programmatic use
ast-index agrep "@Composable fun $NAME($$$)" --lang kotlin --json
```

**Pattern syntax** (ast-grep):
- `$NAME` — matches a single AST node (captures as metavariable)
- `$$$` — matches zero or more AST nodes (variadic)
- Everything else is literal pattern matching against the AST

**When to use `agrep` vs built-in commands:**
- `composables`, `inject`, `suspend` → use built-in (faster, pre-configured)
- Custom structural patterns, negative matching, context queries → use `agrep`

### Raw SQL Query

```bash
# Execute any SELECT query against the index database (JSON output)
ast-index query "SELECT s.name, s.kind, f.path FROM symbols s JOIN files f ON s.file_id = f.id WHERE s.kind = 'class'"

# With limit
ast-index query "SELECT * FROM symbols WHERE name LIKE '%Service%'" --limit 50

# Complex joins — find classes that implement an interface but have no usages
ast-index query "SELECT s.name, f.path FROM symbols s JOIN files f ON s.file_id = f.id JOIN inheritance i ON s.id = i.child_id WHERE i.parent_name = 'Repository' AND s.name NOT IN (SELECT name FROM refs)"

# Find files with most symbols (complexity hotspots)
ast-index query "SELECT f.path, COUNT(*) as sym_count FROM symbols s JOIN files f ON s.file_id = f.id GROUP BY f.id ORDER BY sym_count DESC LIMIT 20"

# Find unused classes (defined but never referenced)
ast-index query "SELECT s.name, s.kind, f.path FROM symbols s JOIN files f ON s.file_id = f.id WHERE s.kind IN ('class', 'interface') AND s.name NOT IN (SELECT name FROM refs) AND s.name NOT IN (SELECT parent_name FROM inheritance)"

# Module dependency analysis — find modules with most dependencies
ast-index query "SELECT m.name, COUNT(*) as dep_count FROM module_deps md JOIN modules m ON md.module_id = m.id GROUP BY m.id ORDER BY dep_count DESC"

# CTE — transitive dependency chain
ast-index query "WITH RECURSIVE chain AS (SELECT md.dep_module_id, m.name, 1 as depth FROM module_deps md JOIN modules m ON md.dep_module_id = m.id WHERE md.module_id = (SELECT id FROM modules WHERE name LIKE '%payments%') UNION ALL SELECT md.dep_module_id, m.name, c.depth + 1 FROM chain c JOIN module_deps md ON md.module_id = c.dep_module_id JOIN modules m ON md.dep_module_id = m.id WHERE c.depth < 5) SELECT DISTINCT name, depth FROM chain ORDER BY depth, name"
```

**Security**: Only `SELECT`, `WITH`, and `EXPLAIN` queries allowed. Mutations (`INSERT`, `UPDATE`, `DELETE`, `DROP`) are blocked.

### Database Schema

```bash
# Show all tables with columns and row counts (JSON)
ast-index schema
```

Key tables:
| Table | Description | Key columns |
|-------|-------------|-------------|
| `files` | Indexed source files | `path`, `mtime`, `size` |
| `symbols` | Classes, functions, properties, etc. | `name`, `kind`, `line`, `file_id`, `signature` |
| `symbols_fts` | FTS5 full-text search on symbols | `name`, `signature` |
| `inheritance` | Parent-child type relationships | `child_id`, `parent_name`, `kind` |
| `refs` | Symbol references/usages | `name`, `line`, `file_id`, `context` |
| `modules` | Build modules (Gradle, Maven, etc.) | `name`, `path`, `kind` |
| `module_deps` | Module → module dependencies | `module_id`, `dep_module_id` |
| `transitive_deps` | Pre-computed transitive deps | `module_id`, `dependency_id`, `depth` |
| `xml_usages` | Android XML layout class usages | `class_name`, `file_path` |
| `resources` | Android resource definitions | `type`, `name`, `file_path` |

### Direct Database Access (SDK / Scripting)

```bash
# Get the SQLite database path for external tools
ast-index db-path
# Output: /Users/me/.cache/ast-index/abc123/index.db
```

Use this path with any language that supports SQLite:

```python
import sqlite3, json
db_path = subprocess.check_output(["ast-index", "db-path"]).decode().strip()
conn = sqlite3.connect(db_path)

# Example: find all classes implementing an interface with no references
unused_impls = conn.execute("""
    SELECT s.name, f.path, i.parent_name
    FROM symbols s
    JOIN files f ON s.file_id = f.id
    JOIN inheritance i ON s.id = i.child_id
    WHERE s.name NOT IN (SELECT name FROM refs)
    ORDER BY i.parent_name, s.name
""").fetchall()
for name, path, parent in unused_impls:
    print(f"  {name} implements {parent} — {path}")
```

**When to use `query` vs predefined commands:**
- Simple lookups → use `search`, `class`, `usages` (faster, formatted output)
- Complex joins, aggregation, negative conditions → use `query` (one call instead of N)
- Batch analysis, scripting, CI pipelines → use `db-path` + direct SQLite access

## Performance Reference

Wall time of one call, process start included: a small project, and a
40k-file Rails + React monorepo (300k symbols) with a warm page cache.

| Command | Small | 40k files | Notes |
|---------|-------|-----------|-------|
| class / symbol | ~10ms | ~20ms | Direct index lookup |
| usages / implementations | ~10ms | ~20-40ms | Indexed reference search |
| imports | ~10ms | ~20ms | File-based lookup |
| outline | ~10ms | ~50ms | Parses the file |
| search | ~10ms | ~250-550ms | FTS5 symbols plus a scan of file contents |
| explore | ~15ms | ~100-150ms | `--rwr` ~200ms |
| graph queries | ~10ms | ~100ms | Needs `graph build` (~1-2s) |
| callers | ~20ms | ~200ms | Text match at query time |
| call-tree | ~20ms | ~500ms | Callers of callers |
| map / conventions | ~20ms | ~0.1-1s | SQL aggregation |
| rebuild | seconds | minutes | Full project indexing |

## Platform-Specific Commands

### Android/Kotlin/Java/Spring

Consult: `references/android-commands.md`

Java parser indexes: classes, interfaces, enums, methods, constructors, fields, and significant annotations (@RestController, @Service, @Repository, @Component, @Entity, @GetMapping, @PostMapping, @Autowired, @Override, @Transactional, @SpringBootApplication, @Test, @Inject, @Data, @Builder, etc.).

Maven modules (pom.xml) are fully supported alongside Gradle modules.

- **DI Commands**: `provides`, `inject` (@Inject + @Autowired), `annotations`
- **Compose Commands**: `composables`, `previews`
- **Coroutines Commands**: `suspend`, `flows`
- **XML Commands**: `xml-usages`, `resource-usages`
- **Code Quality**: `deprecated`, `suppress`, `todo`
- **Extensions**: `extensions`
- **Navigation**: `deeplinks`

### iOS/Swift/ObjC

Consult: `references/ios-commands.md`

- **Storyboard & XIB**: `storyboard-usages`
- **Assets**: `asset-usages`
- **SwiftUI**: `swiftui`
- **Swift Concurrency**: `async-funcs`, `main-actor`
- **Combine**: `publishers`

### TypeScript/JavaScript

Consult: `references/typescript-commands.md`

- Index: `class`, `interface`, `type`, `function`, `const`, decorators
- Supports: React (hooks, components), Vue SFC, Svelte, NestJS, Angular
- `outline` and `imports` work with TS/JS files

### Rust

Consult: `references/rust-commands.md`

- Index: `struct`, `enum`, `trait`, `impl`, `fn`, `macro_rules!`, `mod`
- Supports: Derives, attributes (`#[test]`, `#[derive]`)
- `outline` and `imports` work with Rust files

### Ruby

Consult: `references/ruby-commands.md`

- Index: `class`, `module`, `def`, constants (`A::B = ...`), Rails DSL
- Supports: RSpec (`describe`, `it`, `let`), Rails (associations, validations)
- `db/schema.rb` (indexed even when gitignored): tables as `table`, columns as
  `column` named `table.column` — `ast-index search first_name -t column`
- `outline` and `imports` work with Ruby files

### C#/.NET

Consult: `references/csharp-commands.md`

- Index: `class`, `interface`, `struct`, `record`, `enum`, methods, properties
- Supports: ASP.NET attributes, Unity (`MonoBehaviour`, `SerializeField`)
- `outline` and `imports` work with C# files

### Dart/Flutter

Consult: `references/dart-commands.md`

- Index: `class`, `mixin`, `extension`, `extension type`, `enum`, `typedef`, functions, constructors
- Supports: Dart 3 modifiers (sealed, final, base, interface, mixin class)
- `outline` and `imports` work with Dart files

### Python

Consult: `references/python-commands.md`

- Index: `class`, `def`, `async def`, decorators
- `outline` and `imports` work with Python files

### Go

Consult: `references/go-commands.md`

- Index: `package`, `type struct`, `type interface`, `func`
- `outline` and `imports` work with Go files

### Scala

- Index: `class`, `case class`, `object`, `trait`, `enum` (Scala 3), `def`, `val`, `var`, `type`, `given`
- Supports: Inheritance (`extends`/`with`), companion objects
- `outline` and `imports` work with Scala files

### PHP

- Index: `namespace`, `class`, `interface`, `trait`, `enum`, `function`, `method`, `const`, `property`, `use`
- Supports: Laravel (models, traits, facades), `extends`/`implements`, namespace imports
- `outline` and `imports` work with PHP files

### Perl

Consult: `references/perl-commands.md`

- **Exports**: `perl-exports`
- **Subroutines**: `perl-subs`
- **POD**: `perl-pod`
- **Imports**: `perl-imports`
- **Tests**: `perl-tests`

### Module Analysis

Consult: `references/module-commands.md`

- **Module Search**: `module`
- **Dependencies**: `deps`, `dependents`
- **Unused Dependencies**: `unused-deps`
- **Unused Symbols**: `unused-symbols`
- **Public API**: `api`

## Workflow Recommendations

1. Run `ast-index rebuild` once in project root to build the index
2. **Start a session** with `ast-index conventions` + `ast-index map` to understand project structure (~80 lines, ~500 tokens)
3. Use `ast-index map --module <path>` to drill down into specific areas
4. Use `ast-index search` for quick universal search when exploring
5. Use `ast-index class` for precise class/interface lookup
6. Use `ast-index usages` to find all references before refactoring (`graph dependents` for the dependents of one definition)
7. Use `ast-index implementations` to understand inheritance
8. Use `ast-index changed --base main` to inventory branch files before code review
9. Run `ast-index update` periodically to keep index fresh

## JSON Output (Optional)

**Default output is human-readable plain text** — use it unless you specifically need structured data for scripting.

Add `--format json` only when:
- Parsing output programmatically (pipelines, scripts)
- Need exact field values (file paths, statuses, line numbers, symbol kinds)
- Integrating with another tool

Commands with JSON output include `search`, `explore`, `file`, `symbol`, `class`, `usages`, `callers`, `implementations`, `refs`, `stats`, `changed`, `hotspots`, `graph …`, `unused-symbols`, `map` and `conventions`; others (`outline`, `call-tree`, `hierarchy`, `imports`, `todo`) print text either way.

```bash
ast-index --format json search "Query" | jq '.symbols[].path'
```

## Additional Resources

For detailed platform-specific commands, consult:

- **`references/android-commands.md`** - DI, Compose, Coroutines, XML
- **`references/ios-commands.md`** - Storyboard, SwiftUI, Combine
- **`references/typescript-commands.md`** - React, Vue, Svelte, NestJS, Angular
- **`references/rust-commands.md`** - Structs, traits, impl blocks, macros
- **`references/ruby-commands.md`** - Rails, RSpec, classes, modules
- **`references/csharp-commands.md`** - ASP.NET, Unity, controllers, interfaces
- **`references/dart-commands.md`** - Dart/Flutter classes, mixins, extensions
- **`references/perl-commands.md`** - Perl exports, subs, POD
- **`references/python-commands.md`** - Python classes, functions
- **`references/go-commands.md`** - Go structs, interfaces
- **`references/cpp-commands.md`** - C/C++ classes, JNI functions
- **`references/proto-commands.md`** - Protocol Buffers messages, services
- **`references/wsdl-commands.md`** - WSDL services, XSD types
- **`references/module-commands.md`** - Module dependencies
