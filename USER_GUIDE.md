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

### Cargo (pending first crates.io release)

This channel becomes available with the first ast-index release on crates.io.
After that rollout, install with a Rust toolchain:

```bash
cargo install ast-index --locked
```

Before the first crates.io release, use one of the binary channels above or
build from source.

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
`ast-index rebuild` in that project. The same pass also deletes the lock files
in the cache's `.leases` directory that belong to caches which no longer exist,
whatever their age; a lock that another process holds is kept.

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

Minified JavaScript and CSS is left out without any configuration: it never
enters the index, the grep-based commands (`search` file contents, `callers`,
`call-tree`, `todo`, …) never read it, and `outline` / `imports` on such a file
answer `Skipped: minified file` instead of parsing it. A `.js`, `.mjs`, `.cjs`
or `.css` file counts as minified when its name ends in `.min` or `-min` before
the extension (`app.min.js`, `vendor-min.css`), or when its first 64 KiB has
lines of 1000 bytes or more on average, at least 100 of them outside string
literals. Source with a few long lines — an inline SVG path, a data URI, an
HTML or legal-text template in a string — stays below that. TypeScript, JSX,
SCSS and `.d.ts` files are never judged. Set `AST_INDEX_SKIP_MINIFIED=0` to index
and search minified files like any other source; the variable applies to
whichever command runs with it, so keep it set for `update` and the grep-based
commands too.

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

A new or changed file that is minified is not indexed, and one that is already
in the index is removed like a deleted file. An index written by an older
version (or with `AST_INDEX_SKIP_MINIFIED=0`) may still hold minified files
whose `mtime` has not moved; the first `update` after that also checks the
unchanged `.js` / `.mjs` / `.cjs` / `.css` files once, so no `rebuild` is
needed to clear them.

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

Collected Git history (`hotspots --collect`) is not refreshed by `update`. Run
`ast-index hotspots --collect` again after switching: it subtracts the commits
the new `HEAD` no longer reaches and adds the new ones, reusing diffs it has
read before, so a branch switch or a rebase costs about a second instead of a
full rescan, and the numbers match a fresh `hotspots --collect --full`.

`rebuild` keeps the collected history: it depends on the repository, not on
the index, so the next `hotspots --collect` stays incremental. Only history
collected by an older version, from another working tree or for another
directory of the repository is left behind; the rebuild says so and the next
`--collect` reads the history again.

Use `rebuild` instead of `update` when:

- the project root or `.ast-index.yaml` changed significantly;
- generated or vendored folders were added or removed;
- the index looks inconsistent after a very large branch switch;
- you want a clean baseline before sharing results with an agent.

## Independent Git Worktrees

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

Do not attach one worktree to another with `subtree add`. Worktrees can contain
different revisions of the same files; combining them would mix branch states
in one result set.

## Intentional Cross-Root Workspaces

Named subtrees solve a different problem: one logical workspace that
intentionally spans sibling source trees, such as an application and a local
shared library. All attached subtrees share the primary project's index.

The command order matters. `subtree add` stores its configuration in an
existing index, so create the primary index first, attach the subtree second,
then run an update or rebuild to index its files:

```bash
cd /path/to/application
ast-index rebuild
ast-index subtree add shared ../shared-library
ast-index update
ast-index subtree list
```

`update` incrementally indexes the newly attached subtree. A later `rebuild`
also preserves named subtree configuration and rebuilds every attached root.

Use a name to narrow results to one attached tree, or `--local` to keep only
the primary project:

```bash
ast-index --subtree shared search "Payment"
ast-index --local search "Payment"
```

Detach by name, then rebuild to remove that subtree's indexed files:

```bash
ast-index subtree remove shared
ast-index rebuild
```

The legacy `add-root`, `remove-root`, and `list-roots` commands remain
compatibility aliases. New scripts and documentation should use the named
`subtree` commands.

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
- **Graph:** `graph dependents|dependencies|impact|path|cycles|top|metrics` — symbol dependency graph (`graph build` first)
- **Hierarchy:** `implementations`, `hierarchy`, `extensions` — class hierarchy
- **Modules:** `module`, `deps`, `dependents`, `api` — module dependencies
- **Files:** `outline`, `imports`, `changed` — file analysis
- **iOS:** `storyboard-usages`, `asset-usages`, `asset-unused` — storyboard/asset search
- **Quality:** `todo`, `deprecated`, `hotspots` — TODOs, deprecated items, Git-history risk
- **Index:** `rebuild`, `update`, `watch`, `stats` — index management

## Common Use Cases

- `ast-index usages "PaymentViewController"` — where is this class used?
- `ast-index implementations "PaymentProcessing"` — what implements this protocol?
- `ast-index callers "processPayment"` — what calls this function?
- `ast-index call-tree "processPayment" -d 3` — call hierarchy
- `ast-index deps "PaymentFeature"` — module dependencies
- `ast-index dependents "NetworkKit"` — what depends on this module?
- `ast-index changed` — what changed in my branch?
- `ast-index hotspots --collect` — which files churn most and attract the most bugfixes?
- `ast-index search Service --rank proven` — which of these is safe to copy? (also `risky`, `hotspots`, `central`)
- `ast-index graph impact "PaymentGateway" --depth 3` — what breaks if I change this, transitively?
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
ast-index search "Payment" --rank risky # re-rank by history + graph (proven|hotspots|risky|central)
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
ast-index changed                       # files changed on the current branch
ast-index hotspots --collect            # rank files by Git history (churn, fixes, authors)
ast-index graph build                   # precompute the symbol dependency graph
ast-index graph dependents "Invoice"    # who depends on it, with resolution confidence
ast-index graph impact "Invoice" -d 3   # transitive dependents per depth
ast-index graph path "OrdersController" "Invoice"  # how one reaches the other
ast-index graph top --kind class        # most central symbols (PageRank)
ast-index map                           # compact project map
ast-index conventions                   # detected frameworks and patterns
```

Use JSON for scripts or agents:

```bash
ast-index --format json search "Payment"
```

Paginated search commands use JSON schema v2. Single-result-set commands return
`items` plus `pagination { total, returned, truncated, limit }`; `search` and
`refs` keep named arrays with per-array pagination metadata. Clients written
for bare arrays must unwrap `items`, and every client should check `truncated`
before treating results as complete. Increase `--limit` to request more rows.
The `changed` command remains on its independent schema v1.

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

`call-tree` finds callers by text search at query time and prints every
caller with its file, same-named definitions of other files included (two
`it "works"` blocks are two callers). Callers are looked up by name, so each
name is expanded once: a later caller of that name is marked
`(expanded above)`, and `(recursive)` marks only a definition already on its
own path — a real cycle.

Build the symbol dependency graph when you need to know who really depends on
a definition, how central it is, or what a change would reach transitively:

```bash
ast-index graph build                         # explicit; rebuild/update never run it
ast-index graph dependents "Invoice"          # incoming edges with confidence
ast-index graph dependencies "OrdersController" --members
ast-index graph impact "Invoice" --depth 3    # blast radius per depth (symbols, files)
ast-index graph path "OrdersController" "Invoice"
ast-index graph cycles
ast-index graph top --sort pagerank --kind class
```

Each edge records how its target was resolved: `local`, `scoped` (namespace,
receiver type or inheritance), `import`, `unique`, or `ambiguous` with the
number of candidates. Installed packages (files under `node_modules`) are
never part of the graph; everything else in the index is project code,
including a `vendor/` directory and the project's own `.d.ts` files — keep
vendored third-party code out with `exclude` in `.ast-index.yaml`. Code
outside tests never resolves to a definition inside them (a spec helper that
reopens a class to stub a method is not what production code calls; see
**Test files** below for what counts as a test). Metrics count resolved edges only;
`--include-ambiguous` lists the rest. After `update` changes the index the graph reports itself as
stale until `graph build` (or a query with `--refresh`) runs again.

References are capitalized names and calls written `name(` — snake_case and
`_private` names included (`update_profile(user)`, `self._compute()`). Reserved
words of C/C++, Go, Python, Rust, Perl and JavaScript never count: `sizeof (x)`,
`#if defined(X)`, Go's `func (r *T)`, Rust's `pub(crate)`, Python's
`except (A, B):` and `None`. A reserved word used as a member
(`map.delete(key)`) or called as a Perl `&name(...)` is still a reference.

BSL (1C:Enterprise, OneScript) references are calls in Cyrillic or Latin
(`ПолучитьДанные()`), a module or object before `.` (`ОбщегоНазначения.`) and
the type after `Новый` / `New`; a plain capitalized word is a variable, and
keywords are skipped in any letter case (`НЕ`, `Не`). Stylesheets (CSS, SCSS,
Less) record no references.

Ruby references include calls without parentheses (`recv.name`, `name arg`,
a bare `name` that is not a local variable), so the graph also links a method
to the instance methods and attribute readers it calls on `self`, and an RSpec
example to the `let` helpers it uses. Calls on a receiver of unknown type stay
`ambiguous`, and core Ruby collection/string methods called without
parentheses are not recorded at all. A lowercase `name(` counts only where the
syntax tree has a call: in a comment, a string or a heredoc it does not.

In a Rails application `db/schema.rb` is indexed even when it is gitignored:
each `create_table` becomes a `table` symbol and each column a `column` symbol
named `table.column` (`ast-index search email -t column`). `graph build`
matches tables to models by Active Record's rules — `self.table_name`,
single-table inheritance, a model nested in another model, a namespace's
`table_name_prefix` or engine `isolate_namespace`, then the pluralized class
name — and prints what it could not match (`--format json` lists the tables
without a model, the models without a table, and models for which both a
plain and a namespaced table exist). Inside a model, a column reader or
attribute method (`email`, `self.email`, `email?`, `email_changed?`,
`saved_change_to_email?`) that no method in the class chain defines resolves
as a `scoped` edge to the column; a call on any other receiver
(`user.email`) is never guessed:

```bash
ast-index graph dependents users.email       # or users#email
```

### Ranking search results by history and structure

`search --rank <preset>` re-orders the **Files** and **Symbols** sections of a
search by what the index knows beyond the name: the Git history of the file
(`hotspots --collect`) and the symbol's place in the dependency graph
(`graph build`). References and content matches keep their plain order.

```bash
ast-index hotspots --collect && ast-index graph build     # once; both are explicit
ast-index search Service --fuzzy --module app/services/ --rank proven   # what to copy
ast-index search Merge --rank risky                       # what is dangerous to touch
ast-index search Import --module app/services/ --rank hotspots          # where it keeps breaking
ast-index search Event --module app/models/ --rank central
ast-index search Import --rank hotspots --exclude-tests   # without spec/test files
ast-index --format json search Merge --rank risky         # dossier per result
```

| Preset | Question | Needs |
|--------|----------|-------|
| `proven` | Which of these is safe to copy as a pattern? | history + graph |
| `hotspots` | Which of these keeps being changed and fixed? | history |
| `risky` | Which of these is dangerous to touch? | history + graph |
| `central` | Which of these does the rest of the code lean on? | graph |

**Formulas.** Every input is a 0..1 value; history percentiles are against all
live files of the repository, graph percentiles against all symbols with at
least one resolved caller.

- `hotspots` = the file's hotspot score: mean percentile of commits, churn and
  bugfix ratio — the number `ast-index hotspots` prints rounded (`score`) and
  in full (`score_exact` in JSON). Presets use the unrounded percentiles, so
  files that share a rounded score near the top still order meaningfully.
- `proven` = mean of four terms: *calm* (1 − hotspot score), *age* (file age
  percentile), *idle* (percentile of days since the file last changed) and
  *used* (1 when at least one resolved reference points at the symbol, else 0).
- `risky` = *blast radius* × hotspot score, where blast radius is the
  percentile of the symbol's transitive dependents (≤ 3 hops, resolved edges),
  0 when nothing depends on it. Both have to be high.
- `central` = PageRank percentile, 0 when nothing resolves to the symbol.

**Why these formulas.** They were chosen by backtesting on a 40k-file
Ruby/TypeScript monorepo with 25k commits: file signals were computed from the
history up to a cut-off T, and the outcome was bugfix commits to the same file
in the 12 months after T, for T = 12, 24 and 36 months before HEAD.

- The hotspot score's top 10% of files received a bugfix 4.2×, 2.2× and 5.1× as
  often as the average file. Bugfix ratio on its own managed only 1.5×, 0.9×
  and 1.6×: it is part of the score, but activity is what predicts.
- `proven`: among files something depends on, the top 10% by `proven` were
  fixed 0.15×, 0.06× and 0.09× as often as the average such file, and every
  one of them is used. Adding the author count made the top decile *more*
  fix-prone (0.5–1.3× the base rate, because authors track activity), so authors
  are shown but not scored. Weighting usage by how many files use a symbol
  instead of 1/0 was worse too (0.45×, 0.21×, 0.47×): more callers, more
  exposure.
- `risky` was measured as impact-weighted damage — P(bugfix next year) ×
  log2(1 + dependents) — collected by the top decile: 7.4×, 6.9× and 8.0×
  random, against 6.3×/5.8×/6.7× for centrality alone, 5.1×/4.0×/6.4× for the
  hotspot score alone, and 7.0×/6.3×/7.5× for the same product with dependents
  ranked against all graph nodes.
- `central`: PageRank, fan-in and dependents rank-correlate at 0.98+ among
  graph nodes, so the choice matters only at the top; PageRank is what
  `graph top` sorts by. 69% of graph nodes have no resolved caller, so against
  all nodes any caller at all lands above the 69th percentile; ranking against
  referenced symbols spreads the scale over the range that varies.

**Relevance is kept, not replaced.**

1. The pool is the top 100 project symbols of the plain relevance order (or
   `--limit` + 1 if larger) and up to 2000 project files matching the path.
2. Symbol tiers are hard: exact name (case-sensitive), exact name ignoring
   case, last `::` segment of the name equal to the query (case-sensitive, not
   with `--fuzzy`; `Billing::LedgerImporter` for `LedgerImporter`), a word of
   the name starting with the query (a substring with `--fuzzy`, where case is
   not told apart), signature-only match. The plain order uses the same tiers.
   A preset only re-orders inside a tier, so an exact match is never pushed
   below a partial one.
3. Inside a tier the sort key is `0.9 × score + 0.1 × relevance`, where
   relevance is `1 / (1 + position / 20)` and position is the candidate's place
   in the tier's plain order. The weight was swept over 11 queries: 0.9
   realizes 93% of the score the tiers allow in the top five while reaching
   about 21 positions deep on average; 0.8 kept 73%, 1.0 reached 33 deep.
4. Files all contain the query in their path and come back alphabetically, so
   where the match sits — file stem, file name, directory — is their relevance
   term (`1 / (1 + tier)`), with the same 0.9 weight.

**Granularity.** History is per *file*: every symbol in a file shares its
file's history, and the output says "file history". Graph metrics are per
*symbol*; a file result borrows them from its strongest symbol (highest
PageRank), named in the output.

**Missing evidence.** A preset whose data is missing is not applied: results
stay in plain relevance order, the text output says what is missing and which
command collects it, and JSON reports `rank.applied: false` with
`rank.missing: [{signal, reason, command}]`. A stale graph (the index changed
since `graph build`) still ranks, with a warning and `rank.graph.stale: true`.

**Unscored results.** Third-party code (`node_modules`, `.d.ts`) has no
history in the repository and no graph edges (`graph build` never targets
installed packages), so presets never score it and list it after every project
result. Project files without collected history (untracked, or newer than the
last `hotspots --collect`) and files of attached subtrees (history covers the
primary root only) keep their relevance order after the scored results of
their tier, marked `unscored`.

**Test files.** Specs churn and get fixed by nature, so they crowd the top of
`hotspots` and `risky`. `--exclude-tests` leaves them out of the ranked files
and symbols sections and their totals. Percentiles are still computed against
every file, so a file's score does not change with the flag; `hotspots
--exclude-tests` works the same way.

One test-path rule serves `search --rank`, `hotspots` and `graph top` with
`--exclude-tests`, the graph (code outside tests never resolves into them) and
`explore` (test files rank below source). A file is a test when its name
follows a test convention — `*_test.*`, `*_spec.*`, `*.test.*`, `*.spec.*`,
`test_*.py`, `conftest.py`, and `FooTest` / `FooTests` / `FooSpec` in Java,
Kotlin, Scala, Groovy, Swift, Objective-C, C#, PHP and C++ — or when it sits in
a `spec/`, `test/`, `tests/` or `__tests__/` directory. Two exceptions keep
production code in: in JavaScript / TypeScript those directory names count in
lowercase only (`components/Test/` is a component), and a Ruby file under
`app/` or `lib/` is namespaced code (`app/jobs/tests/` is
`module Tests`), as is a Ruby file in a `tests/` directory, which no Ruby test
framework uses. `latest.rb`, `contest.py` and `Testimonial.kt` are not tests.

**Output.** Each file and symbol carries its dossier: the preset score and its
terms, the relevance position and tier, the raw history numbers with their
percentiles and labels (`churn:high`, `fixes:elevated`, `authors:many`,
`veteran`, …) and the graph numbers with theirs (`fan-in:high`,
`dependents:high`, `pagerank:high`, `callers:unresolved` when only ambiguous
references point at it). In JSON, `files` become objects `{path, rank}` and
symbols gain a `rank` object; the top-level `rank` object carries the preset,
formula, evidence summary, pool sizes and weight.

`rewritten-often` (churn relative to the file's current size, top 10%) is only
computed for files of 10 lines or more. Below that a line count stops measuring
content: a one-line minified bundle or fixture, or a view gutted to a mount
point, would read as "3000x file", and a routine one-line edit already moves a
three-line file by a third. Such files keep their absolute churn labels.

**Known limits.** When a query's exact-name tier fills the page (`search
Policy` in a code base full of `POLICY` constants), a preset can only re-order
that tier; the files section usually answers better. `proven` favours code that
has been left alone for years — safe by the numbers, but possibly written in an
older style. History is per file, so for a small method it describes the class
around it.

Use structural search through ast-grep when `sg` is installed:

```bash
ast-index agrep 'if ($COND) { return $RET; }' --lang typescript
```

Attach a named subtree when a project intentionally depends on local sibling
code. The primary index must already exist:

```bash
ast-index rebuild
ast-index subtree add shared /path/to/shared-library
ast-index update
ast-index subtree list
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
