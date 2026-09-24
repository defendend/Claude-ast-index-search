# ast-index MCP server

`ast-index-mcp` exposes ast-index as an [MCP](https://modelcontextprotocol.io)
server so any MCP-compatible AI agent (Claude Code, Cursor, Codex, Cline,
Continue, OpenCode, Windsurf, and future ones) can call its code-search
tools directly — no per-agent plugin required.

## Why MCP alongside the native plugin?

- The Claude Code plugin works great — if you use Claude Code. Everyone
  else (Cursor, Codex, Cline, Continue, etc.) can't use it.
- MCP is the cross-agent protocol. One server → all agents.
- The MCP server is a thin wrapper that shells out to the `ast-index`
  binary you already have — you can upgrade one without the other.

## Tools exposed

| Tool | Purpose |
|---|---|
| `explore` | One-shot answer to "how does X work": the relevant symbols ranked, an outline of the best types/modules and the source of the best functions, their callers/subclasses, and tests |
| `search` | Literal search: filenames + symbols + imports/usages + content, one call; `rank` re-orders by history and graph (see below) |
| `outline` | Structural outline of one file (classes, functions, line numbers) — ALWAYS run before reading files > 500 lines |
| `usages` | Every indexed reference to a symbol, matched by name |
| `callers` | Call-site lines of a function, matched by name at query time |
| `call_tree` | The function each call sits in, then its callers, up to a depth |
| `implementations` | Concrete types that implement an interface / protocol / abstract class |
| `hierarchy` | Superclasses and subclasses of a type |
| `refs` | Cross-references in one shot: definitions + imports + usages |
| `symbol` / `class` | Definitions by exact name or glob |
| `find_file` | Files by name |
| `imports` / `api` | A file's imports; a module's public API |
| `module` / `deps` / `dependents` | Modules (Gradle, Cargo, …) and their dependencies in both directions |
| `changed` | Files changed on the current branch |
| `graph_dependents` | Who depends on a symbol: direct edges, or the transitive blast radius with `depth` ≥ 2 |
| `graph_dependencies` | What a symbol depends on |
| `graph_path` | Shortest dependency path(s) from one symbol to another |
| `graph_metrics` | Most central symbols, or fan-in / fan-out / dependents / PageRank of given ones |
| `graph_cycles` | Dependency cycles |
| `graph_build` | Build or refresh the symbol graph the `graph_*` tools and `search` `rank` read |
| `hotspots` | Git-history risk per file: churn, bugfix share, authors, age (report only) |
| `stats` / `update` / `rebuild` | Index statistics and maintenance |

### Graph and history tools need data collected first

The symbol dependency graph and the Git history signals are not part of the
index `rebuild` / `update` produce; each is collected by an explicit command.

- **Symbol graph** — read by the `graph_*` tools and by `search` with `rank`
  `proven`, `risky` or `central`. Build it with the `graph_build` tool or
  `ast-index graph build` (seconds, even on a large monorepo). A `graph_*`
  call with `refresh: true` builds it first when it is missing or stale. After
  `update` changes the index the graph reports itself as stale until rebuilt.
- **Git history** — read by `hotspots` and by `search` with `rank` `proven`,
  `risky` or `hotspots`. Collect it with `ast-index hotspots --collect` in a
  shell. It is deliberately **not** exposed through MCP: the first collection
  reads the whole history (about a minute on a large monorepo), longer than
  many MCP clients wait for a tool call. Later runs take seconds, including
  after a branch switch, rebase or `rebuild`: history is stored per commit, so
  only commits HEAD gained or lost are read. A Git hook or a session-start
  hook is a good place for it.

When either is missing, the tools say so and name the remedy instead of
returning empty results; `search` with `rank` falls back to plain relevance
order.

## Output format (token-efficient by default)

The point of this MCP is to **save context tokens**. Tool results default
to a compact plain-text format that preserves all the information of the
underlying JSON but costs roughly half the tokens:

```
Symbols:
  PathResolver [struct] src/commands/mod.rs:50
  resolve [function] src/commands/mod.rs:70

Content:
  src/commands/mod.rs:50  pub struct PathResolver {
```

If an agent explicitly needs structured JSON for programmatic parsing,
pass `format: "json"` in the tool arguments — at the cost of ~2-3× more
tokens. In practice this is rarely necessary; modern LLMs parse the
compact text format reliably.

`search` with `rank` prints each result's evidence once: a file's history is
shared by every symbol in it, so a symbol whose file was already described
says `history and graph: as above` instead of repeating the numbers.

A multi-word `search` without literal matches answers with `explore` results.
The text starts with `fallback: explore — <reason>` and carries the same
report `explore` returns: the source of the best functions and an outline
(`:start-end name [kind]` rows) of the best types and modules, the ranked
symbols, graph neighbours and tests found by path convention.

## Install

### Prerequisites

`ast-index` must be on `PATH`. Install per your platform:

```bash
# macOS / Linux
brew tap defendend/ast-index
brew install ast-index

# npm (all platforms)
npm install -g @defendend/ast-index

# From source
git clone https://github.com/defendend/Claude-ast-index-search.git
cd Claude-ast-index-search && cargo build --release
```

Verify: `ast-index version`.

### Build the MCP server

```bash
git clone https://github.com/defendend/Claude-ast-index-search.git
cd Claude-ast-index-search
cargo build --release -p ast-index-mcp
```

The binary lands at `target/release/ast-index-mcp`. Copy it somewhere on
`PATH`:

```bash
cp target/release/ast-index-mcp /usr/local/bin/
# or, on macOS Apple Silicon
cp target/release/ast-index-mcp /opt/homebrew/bin/
```

Verify: `which ast-index-mcp`.

### One-time index build

Before the MCP server can answer queries, build the index in every project
you want to search:

```bash
cd /path/to/your/project
ast-index rebuild
```

On large monorepos this takes a minute or two. After that, `ast-index
update` (re-run after pulling fresh trunk) is seconds.

## Configure your agent

The MCP server reads stdin / writes stdout — standard stdio transport.
Every agent has a JSON config where you register it.

> In every snippet below, set `AST_INDEX_ROOT` to an absolute path. It
> becomes the default project root for every tool call. If you work in
> multiple projects, leave it unset and pass `project_root` in each tool
> call, or run separate MCP-server instances per project.

### Claude Code

Edit `~/.claude/mcp.json` (or the project-level `.mcp.json`):

```json
{
  "mcpServers": {
    "ast-index": {
      "command": "ast-index-mcp",
      "env": {
        "AST_INDEX_ROOT": "/absolute/path/to/your/project"
      }
    }
  }
}
```

Restart Claude Code. Verify: ask the agent "use ast-index to find
usages of <a class in your repo>".

### Cursor

Cursor supports MCP from v0.42+. Open `Cursor Settings → Features → MCP`
or edit `~/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "ast-index": {
      "command": "ast-index-mcp",
      "args": [],
      "env": {
        "AST_INDEX_ROOT": "/absolute/path/to/your/project"
      }
    }
  }
}
```

Reload Cursor window.

### Codex (OpenAI CLI)

From the project you want Codex to search:

```bash
cd /absolute/path/to/your/project
ast-index rebuild
ast-index install-codex-mcp
```

The installer runs `codex mcp add`, sets `AST_INDEX_ROOT` to the current
project, and sets `AST_INDEX_BIN` to the current `ast-index` executable.
It expects `ast-index-mcp` next to `ast-index` or on `PATH`. Preview
without changing Codex config:

```bash
ast-index install-codex-mcp --dry-run
```

Manual fallback for `~/.codex/config.toml`:

```toml
[mcp_servers.ast-index]
command = "ast-index-mcp"
env = { AST_INDEX_ROOT = "/absolute/path/to/your/project", AST_INDEX_BIN = "ast-index" }
```

### Cline (VS Code extension)

Settings → Extensions → Cline → MCP Servers, add:

```json
{
  "ast-index": {
    "command": "ast-index-mcp",
    "env": { "AST_INDEX_ROOT": "/absolute/path/to/your/project" }
  }
}
```

### Continue (VS Code / JetBrains)

`~/.continue/config.yaml`:

```yaml
mcpServers:
  - name: ast-index
    command: ast-index-mcp
    env:
      AST_INDEX_ROOT: /absolute/path/to/your/project
```

### OpenCode

`~/.config/opencode/config.json`:

```json
{
  "mcp": {
    "ast-index": {
      "type": "local",
      "command": ["ast-index-mcp"],
      "environment": { "AST_INDEX_ROOT": "/absolute/path/to/your/project" }
    }
  }
}
```

### Windsurf

`~/.codeium/windsurf/mcp_config.json`:

```json
{
  "mcpServers": {
    "ast-index": {
      "command": "ast-index-mcp",
      "env": { "AST_INDEX_ROOT": "/absolute/path/to/your/project" }
    }
  }
}
```

### Generic / other

If your agent supports MCP over stdio (most do), the pattern is:

```
command: ast-index-mcp
env:
  AST_INDEX_ROOT: /absolute/path/to/your/project  (optional)
  AST_INDEX_BIN:  ast-index                        (optional, default 'ast-index')
```

## Environment variables

| Variable | Purpose | Default |
|---|---|---|
| `AST_INDEX_BIN` | Path to the `ast-index` binary to shell out to | `ast-index` (from `PATH`) |
| `AST_INDEX_ROOT` | Default project root for tool calls with no `project_root` argument | `$PWD` of the MCP server process |

## Multi-project setups

Three patterns, pick whichever fits:

1. **One project.** Set `AST_INDEX_ROOT` in the agent config and forget
   about it.
2. **Many projects, one agent session at a time.** Leave `AST_INDEX_ROOT`
   unset — the MCP server picks up its own CWD (which the agent inherits
   from your shell). Open the agent from the project's directory and it
   just works.
3. **Many projects in parallel.** Pass `project_root` explicitly in every
   tool call, or register one MCP-server entry per project with a
   distinct name (`ast-index-projA`, `ast-index-projB`).

## Suggested agent rules

Drop this into your project's rules file (e.g. `CLAUDE.md`,
`.cursor/rules`, Cline system prompt):

```
When you need to find code in this repository:
1. Use the ast-index MCP tools BEFORE grep or bulk Read.
2. Before reading any file longer than 500 lines, call `outline` on it
   first, then Read only the line range you actually need.
3. For "who uses X" questions use `usages`; for "where is X called" use
   `callers`, or `call_tree` to get the calling functions; for "what
   implements X" use `implementations`.
4. Before changing a class or method, call `graph_dependents` on it; pass
   `depth: 3` to see how far the change reaches.
5. Picking an existing file to copy as a pattern, use `search` with
   `rank: "proven"`; to see which matches are dangerous to touch,
   `rank: "risky"`.
6. If ast-index returns empty, fall back to grep — don't bulk-read files.
```

## Smoke test

Talk to the server directly over stdio. Each line is one JSON-RPC message;
the server answers every message that has an `id`:

```bash
cd /path/to/your/project
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search","arguments":{"query":"Service","rank":"proven","limit":3}}}' \
  '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"graph_dependents","arguments":{"symbol":"ApplicationService","depth":2,"limit":5}}}' \
  | AST_INDEX_BIN=ast-index ast-index-mcp
```

Point `AST_INDEX_BIN` at a specific `ast-index` build to test it, e.g.
`AST_INDEX_BIN=/path/to/checkout/target/release/ast-index`. A tool failure
comes back as a normal response with `"isError": true` and the CLI's stderr in
the text.

## Troubleshooting

**Agent says "tool not found" / server won't start.** The agent spawns
`ast-index-mcp` from your login-shell's `PATH` — GUI apps may not see it.
Put the binary in `/usr/local/bin` or `/opt/homebrew/bin`, or give an
absolute path in the config (`"command": "/usr/local/bin/ast-index-mcp"`).

**Every call returns "Index not found. Run 'ast-index rebuild' first".**
`AST_INDEX_ROOT` is wrong — either missing, or pointing to a directory
without an index. `cd` into that directory manually and run `ast-index
stats` to check.

**Results are stale.** Run `ast-index update` in the project directory.
The `rebuild` MCP tool also works but is slower.

**Multiple projects, `AST_INDEX_ROOT` too rigid.** Don't set it — the
server falls back to its own CWD, or pass `project_root` in each tool
call. Alternatively register one MCP-server entry per project.
