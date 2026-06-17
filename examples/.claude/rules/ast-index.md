# ast-index Rules — EXAMPLE (adapt before using)

> ⚠️ **This is a template, not a drop-in config.** Do not copy this file
> into your project as-is. Read through, change the sections listed below,
> then save it as `.claude/rules/ast-index.md` in your project.
>
> Rules written here teach Claude Code to prefer ast-index over grep/Read,
> outline before reading, and pass the same instructions to any subagents
> it spawns.

## What to change before using this file

1. **CLI invocation.** Every occurrence of `ast-index` below assumes the
   binary is on `PATH`. If you call it differently (monorepo tool-runner,
   `npx ast-index`, custom wrapper), do a find-and-replace of `ast-index`
   → your invocation throughout this file.
2. **Example symbol names** in the "Common use cases" section are
   placeholders (`PaymentViewController`, `NetworkKit`, `processPayment`).
   Replace them with classes / functions / modules that **actually exist
   in your codebase** — concrete precedents help the agent choose commands
   faster than generic ones.
3. **Index hygiene paragraph.** If your workflow pulls fresh trunk / rebases
   often, keep the `update` instruction. Static project — drop it.
4. **Language-specific command list.** Keep only sections that match your
   stack (delete the iOS block if you don't have Swift, etc.). A shorter
   rules file is a more followed rules file.
5. **Extra-roots section** — remove it if your project lives in a single
   directory. Keep it if you vendor external sources.
6. **Delete this "What to change" section** once you've adapted the rest —
   your project's `.claude/rules/ast-index.md` should not carry template
   instructions.

## Keep the index up to date

After pulling fresh trunk or rebasing, run `ast-index update` — it reindexes
only changed files (seconds, not minutes).

If you work across multiple source roots (e.g. vendored dependencies outside
the project), register them once and `update` / `rebuild` will walk them too:

```bash
ast-index add-root /path/to/other-source-root
ast-index rebuild
```

Search output then prints absolute paths for files under extra roots so
there's no ambiguity about which root owns a hit.

## Mandatory search rules

1. **ALWAYS use ast-index FIRST** for any code-search task.
2. **NEVER duplicate results** — if ast-index returned hits, that IS the
   complete answer. Do not re-run grep to "double-check".
3. Use the Grep tool **only when** ast-index returned empty, or for
   regex / string-literal patterns that are not symbol names.

## Mandatory read rules

1. **Before `Read`-ing any file over 500 lines, run `ast-index outline
   <file>` first.**
2. Use the outline to locate the specific symbol / line range you need,
   then `Read` that slice via `offset` / `limit`.
3. Never bulk-read large files without an outline — it wastes the agent's
   context window and produces worse answers.

## Rules for subagents

When you spawn a subagent for code search (via the Agent/Task tool), the
subagent does **not** inherit this file. Include the block below verbatim
in the subagent's prompt:

```
Use `ast-index` via Bash for code search (NOT grep / the Grep tool):
  ast-index explore "how does X work" — one-shot context: ranked source + neighbours + tests (--rwr for graph)
  ast-index search "query"           — universal search
  ast-index file "Name"              — find a file by name fragment
  ast-index symbol "Name"            — find a symbol definition
  ast-index class "Name"             — find a class / interface / struct
  ast-index usages "Name"            — every usage of a symbol
  ast-index callers "func"           — functions that call this one
  ast-index implementations "Iface"  — concrete implementers of an interface
  ast-index refs "Name"              — cross-references (defs + imports + usages)
Use Grep ONLY if ast-index returned empty.

Before Read-ing any file over 500 lines, FIRST run
  ast-index outline <file>
to get its structure, then Read only the targeted slice via offset/limit.
Never bulk-read large files.
```

## Command cheat sheet

Grouped by intent. Full list and flags: `ast-index --help`.

- **Search:** `search`, `file`, `symbol`, `class`
- **Usages & flow:** `usages`, `callers`, `call-tree`, `refs`
- **Hierarchy:** `implementations`, `hierarchy`, `extensions`
- **Modules / deps:** `module`, `deps`, `dependents`, `api`, `unused-deps`
- **Files:** `outline`, `imports`, `changed`
- **Quality:** `todo`, `deprecated`, `unused-symbols`
- **Language-specific** (available when the project is detected as such):
  - Android: `xml-usages`, `resource-usages`, `composables`, `previews`
  - iOS: `storyboard-usages`, `asset-usages`, `swiftui`, `main-actor`, `async-funcs`, `publishers`
  - Kotlin: `suspend`, `flows`, `extensions`, `provides`, `inject`
  - Perl: `perl-subs`, `perl-exports`, `perl-imports`, `perl-pod`, `perl-tests`
- **Index mgmt:** `rebuild`, `update`, `stats`, `add-root`, `remove-root`

## Common use cases

> Replace the example symbol names with real ones from your codebase — the
> agent makes better choices when it has concrete precedents to pattern-match
> against.

- `ast-index usages "PaymentViewController"` — every place this class is
  used (constructors, downcasts, DI registrations, tests).
- `ast-index implementations "PaymentProcessing"` — all concrete types that
  implement this interface / protocol.
- `ast-index callers "processPayment"` — who calls this function, without
  the noise of definition lines.
- `ast-index call-tree "processPayment" -d 3` — transitive caller tree up
  to depth 3. Use when tracing a bug back to its user-facing entry point.
- `ast-index deps "PaymentFeature"` — what this module depends on.
- `ast-index dependents "NetworkKit"` — what depends on this module (useful
  before a breaking change).
- `ast-index changed` — symbols modified in your current branch vs trunk.
  Great for "what am I actually changing?" summaries in PR descriptions.
- `ast-index outline Foo.kt` — structure of a single file before reading it.
- `ast-index todo` — all TODO / FIXME / HACK comments, grouped.
- `ast-index deprecated` — every use of `@Deprecated` / `@deprecated` /
  `#[deprecated]` across languages.

## Scoping searches

All symbol-returning commands accept scope filters — use them to kill noise
on monorepos:

```bash
ast-index usages "Config" --module core/config        # only within one module
ast-index search "retry" --in-file HttpClient.kt      # only inside this file
ast-index symbol "User" --type class                  # only class-kind symbols
```

## When `ast-index` returns empty

Legitimate reasons:

- Symbol genuinely doesn't exist in the codebase.
- Index is stale — run `ast-index update` and retry.
- Symbol is behind a macro or preprocessor directive (C/C++, some Rust
  macros) — ast-index doesn't expand macros. Fall back to Grep.
- You're searching for a string literal, not a symbol — use Grep.

Do **not** fall back to bulk `Read` of files in these cases. Use Grep with
a specific pattern.
