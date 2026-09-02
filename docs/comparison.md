# ast-index and CodeGraph

This is a neutral, source-backed feature comparison, not a performance
benchmark. The table describes the pinned snapshots below. Planned changes in
the current ast-index worktree are called out separately and are not presented
as v3.50.0 release facts.

## Snapshot

Checked on **2026-09-01**:

- **ast-index 3.50.0**, tag `v3.50.0`, commit
  [`e499dcc6`](https://github.com/defendend/Claude-ast-index-search/tree/e499dcc6fcc90dfceafb629fbf5289824a40cccb),
  committed 2026-07-31.
- **CodeGraph 1.6.0** (`package.json` version on `main`), commit
  [`b9ca4b79`](https://github.com/colbymchenry/codegraph/tree/b9ca4b7981116909900368cc1686a1074cd4d4c1),
  committed 2026-08-31. This identifies the reviewed source snapshot; it does
  not imply that the commit was the latest published package.

Both projects evolve quickly. Re-check the linked sources before making a
long-lived tooling decision.

## Comparison

| Criterion | ast-index | CodeGraph |
|---|---|---|
| Primary interface | Native Rust CLI with broad discovery, reference, inheritance, module, resource, and language-specific commands. | CLI centered on `query`, `explore`, `node`, callers/callees, impact, affected tests, index management, and the viewer. |
| MCP | Separate `ast-index-mcp` workspace binary; the reviewed source lists 21 command-oriented tools, including `explore`, search/navigation, graph, and index-management operations. | MCP server exposes `codegraph_explore` by default. Seven narrower tools remain available through an environment allowlist. |
| Updating | Explicit incremental `update`; optional foreground `watch`; `rebuild` creates a clean generation. | `sync` is incremental. The MCP server starts a debounced file watcher and performs connect-time reconciliation; manual full indexing remains available. |
| Storage | Local SQLite/FTS5 cache outside the source tree, keyed by canonical project-root path. | Local SQLite/FTS5 database at `.codegraph/codegraph.db`, normally in the project tree and ignored by version control. |
| Languages | The reviewed ast-index documentation lists Kotlin, Java, Swift, Objective-C, TypeScript/JavaScript, Vue/Svelte, CSS-family languages, Rust, Zig, C#, Python, Go, C/C++, Scala, PHP, Ruby, Perl, Dart, Protocol Buffers, WSDL/XSD, BSL, Lua, Bash, Elixir, SQL, R, Matlab, Groovy, Common Lisp, and GDScript. Support depth varies by language and command. | The reviewed CodeGraph language table documents TypeScript/JavaScript and ArkTS, Python, Go, Rust, Java, C#, PHP, Ruby, C/C++ and related GPU languages, Objective-C, Swift, Kotlin, Scala, Dart, several web component/markup formats, Lua/Luau, R, CFML, COBOL, VB.NET, Erlang, Solidity, Terraform/OpenTofu, and others. Its table labels support depth per language. |
| Graph model | Stores symbols, references, imports, modules/dependencies, and inheritance. CLI commands expose callers, implementations, hierarchy, module routes, and optional graph re-ranking for `explore`. | Stores nodes and resolved edges for calls, imports, inheritance, and framework patterns. `explore` combines source, call paths, and impact context; dedicated callers/callees/impact commands are also available. |
| Multiple roots and workspaces | Named `subtree` attachments intentionally combine sibling roots in one index and support `--subtree` / `--local` filtering. Independent worktrees keep independent indexes. | A project root can cover a monorepo. Separately indexed projects or subprojects can be selected with MCP `projectPath`; an equivalent documented command for attaching arbitrary external roots into one graph was not found in the reviewed official docs. |
| Browser UI | No bundled browser UI is documented in the reviewed release. CLI output includes text/JSON and selected graph-oriented formats. | `codegraph ui` opens a local browser viewer for symbols, callers/callees, source, dependency maps, and saved trails. |
| Installation | Homebrew, npm, Winget, release archives, or source build. The main CLI is a native binary; the MCP server is a separate workspace binary. The reviewed v3.50.0 release did not provide crates.io installation. | Standalone OS installer scripts or npm. Official docs say standalone bundles include their runtime; `codegraph install` then configures supported agents. |
| License | MIT. | MIT. |

## Planned ast-index distribution change

The current worktree prepares crates.io metadata, trusted-publishing release
automation, and post-publication `cargo install` smoke checks. That work is
pending its first crates.io release. It is intentionally excluded from the
v3.50.0 facts in the table above.

## How to choose

Consider **ast-index** when you want a native CLI with many explicit,
scriptable commands, specialized mobile/resource/module queries, an index kept
outside the repository, or one named workspace spanning intentional sibling
roots.

Consider **CodeGraph** when you want a single default MCP exploration tool,
resolved call/impact context as the central abstraction, automatic MCP-session
sync, or a bundled local graph viewer.

Those are workflow differences, not quality or speed rankings. No comparable,
independently run benchmark covering the same versions, repositories, hardware,
queries, and correctness criteria was available for this snapshot, so this
document makes no relative performance claim.

## Primary sources

### ast-index

- [README and CLI overview](https://github.com/defendend/Claude-ast-index-search/blob/e499dcc6fcc90dfceafb629fbf5289824a40cccb/README.md)
- [Parser and language dispatch](https://github.com/defendend/Claude-ast-index-search/blob/e499dcc6fcc90dfceafb629fbf5289824a40cccb/src/parsers/mod.rs)
- [Index lifecycle commands](https://github.com/defendend/Claude-ast-index-search/blob/e499dcc6fcc90dfceafb629fbf5289824a40cccb/src/commands/management.rs)
- [MCP tool descriptors](https://github.com/defendend/Claude-ast-index-search/blob/e499dcc6fcc90dfceafb629fbf5289824a40cccb/crates/ast-index-mcp/src/main.rs)
- [MIT license](https://github.com/defendend/Claude-ast-index-search/blob/e499dcc6fcc90dfceafb629fbf5289824a40cccb/LICENSE)

### CodeGraph

- [README and CLI overview](https://github.com/colbymchenry/codegraph/blob/b9ca4b7981116909900368cc1686a1074cd4d4c1/README.md)
- [Package version and distribution metadata](https://github.com/colbymchenry/codegraph/blob/b9ca4b7981116909900368cc1686a1074cd4d4c1/package.json)
- [How indexing and storage work](https://github.com/colbymchenry/codegraph/blob/b9ca4b7981116909900368cc1686a1074cd4d4c1/site/src/content/docs/core-concepts/how-it-works.md)
- [Indexing and synchronization](https://github.com/colbymchenry/codegraph/blob/b9ca4b7981116909900368cc1686a1074cd4d4c1/site/src/content/docs/guides/indexing.md)
- [Project and nested-repository configuration](https://github.com/colbymchenry/codegraph/blob/b9ca4b7981116909900368cc1686a1074cd4d4c1/site/src/content/docs/getting-started/configuration.md)
- [CLI reference](https://github.com/colbymchenry/codegraph/blob/b9ca4b7981116909900368cc1686a1074cd4d4c1/site/src/content/docs/reference/cli.md)
- [MCP server reference](https://github.com/colbymchenry/codegraph/blob/b9ca4b7981116909900368cc1686a1074cd4d4c1/site/src/content/docs/reference/mcp-server.md)
- [Language support](https://github.com/colbymchenry/codegraph/blob/b9ca4b7981116909900368cc1686a1074cd4d4c1/site/src/content/docs/reference/languages.md)
- [Browser viewer](https://github.com/colbymchenry/codegraph/blob/b9ca4b7981116909900368cc1686a1074cd4d4c1/site/src/content/docs/guides/viewer.md)
- [Installation](https://github.com/colbymchenry/codegraph/blob/b9ca4b7981116909900368cc1686a1074cd4d4c1/site/src/content/docs/getting-started/installation.md)
- [MIT license](https://github.com/colbymchenry/codegraph/blob/b9ca4b7981116909900368cc1686a1074cd4d4c1/LICENSE)
