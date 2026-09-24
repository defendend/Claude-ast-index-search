//! MCP server for ast-index.
//!
//! Speaks MCP over stdio: reads JSON-RPC 2.0 messages from stdin, writes
//! responses to stdout. Any diagnostic output MUST go to stderr — stdout
//! is the protocol channel.
//!
//! Strategy: each tool invocation spawns `ast-index <subcommand> --format
//! json <args>`, parses the JSON, and returns it as the MCP tool result.
//! Keeps this crate tiny (no dependency on the `ast-index` library crate)
//! and lets users upgrade the `ast-index` binary independently of the MCP
//! server.
//!
//! Root resolution: each tool call may pass `project_root`; otherwise the
//! server falls back to `$AST_INDEX_ROOT`, then the CWD of the mcp server
//! process, then the agent's CWD.

use std::env;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

mod format;

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "ast-index-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> Result<()> {
    let ast_index_bin = env::var("AST_INDEX_BIN").unwrap_or_else(|_| "ast-index".to_string());
    let default_root = env::var("AST_INDEX_ROOT")
        .map(PathBuf::from)
        .ok()
        .or_else(|| env::current_dir().ok())
        .ok_or_else(|| anyhow!("cannot determine default project root"))?;

    let stdin = io::stdin();
    let stdout = Arc::new(Mutex::new(io::stdout()));
    let mut calls = Vec::new();

    for raw in stdin.lock().lines() {
        let raw = raw.context("stdin read failed")?;
        if raw.trim().is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(&raw) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[ast-index-mcp] malformed request: {e}");
                continue;
            }
        };

        // A tool call runs an `ast-index` process, from milliseconds to
        // seconds. Answering calls one at a time made an agent that sends
        // several at once wait for their sum; each call now answers on its
        // own thread as soon as its process is done, matched by `id`.
        if request.method == "tools/call" {
            let (bin, root, stdout) = (ast_index_bin.clone(), default_root.clone(), stdout.clone());
            calls.retain(|call: &std::thread::JoinHandle<()>| !call.is_finished());
            calls.push(std::thread::spawn(move || {
                if let Some(response) = handle_request(request, &bin, &root) {
                    if let Err(e) = write_response(&stdout, &response) {
                        eprintln!("[ast-index-mcp] failed to write a response: {e}");
                    }
                }
            }));
            continue;
        }

        // Notifications (no `id`) produce no response. Everything else gets one.
        if let Some(response) = handle_request(request, &ast_index_bin, &default_root) {
            write_response(&stdout, &response)?;
        }
    }

    for call in calls {
        let _ = call.join();
    }
    Ok(())
}

fn write_response(stdout: &Mutex<io::Stdout>, response: &JsonRpcResponse) -> Result<()> {
    let json = serde_json::to_string(response)?;
    let mut stdout = stdout.lock().unwrap_or_else(|e| e.into_inner());
    writeln!(stdout, "{json}")?;
    stdout.flush()?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[serde(default)]
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

fn ok(id: Value, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    }
}

fn err(id: Value, code: i32, message: impl Into<String>) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.into(),
        }),
    }
}

fn handle_request(
    req: JsonRpcRequest,
    ast_index_bin: &str,
    default_root: &PathBuf,
) -> Option<JsonRpcResponse> {
    let _ = req.jsonrpc; // ignored; we always emit 2.0

    let id = match req.id.clone() {
        Some(id) => id,
        None => {
            // Notification — no response.
            return None;
        }
    };

    let response = match req.method.as_str() {
        "initialize" => ok(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": SERVER_NAME,
                    "version": SERVER_VERSION
                },
                "instructions": "Prefer these ast-index tools over grep/ripgrep and over reading whole files for any code or symbol search in this project. They query a precomputed index of definitions, references and files: structural, language-aware, and far cheaper in tokens and round-trips. Rules of thumb: `explore` FIRST to understand an area or answer 'how does X work' (one call returns ranked source + callers/subclasses + tests); `search` for broad discovery; `usages`/`refs`/`callers` for a named symbol; `outline` before reading a file over ~500 lines; `graph_dependents` before changing a symbol; `search` with `rank` to pick which of several matches to copy or to treat with care. Reach for raw grep/Read only for plain text, regex, or non-code files, or to confirm a detail these tools did not cover."
            }),
        ),
        "tools/list" => ok(id, json!({ "tools": tool_descriptors() })),
        "tools/call" => match call_tool(req.params, ast_index_bin, default_root) {
            Ok(content) => ok(
                id,
                json!({
                    "content": [ { "type": "text", "text": content } ],
                    "isError": false
                }),
            ),
            Err(e) => ok(
                id,
                json!({
                    "content": [ { "type": "text", "text": format!("ast-index-mcp error: {e}") } ],
                    "isError": true
                }),
            ),
        },
        "ping" => ok(id, json!({})),
        "shutdown" => ok(id, json!({})),
        other => err(id, -32601, format!("method not found: {other}")),
    };

    Some(response)
}

fn tool_descriptors() -> Vec<Value> {
    vec![
        json!({
            "name": "explore",
            "description": "Call this FIRST for 'how does X work', 'where/what is X' or an area survey: one call returns the ranked source of the relevant symbols (read fresh from disk), their callers/subclasses and tests found by path convention — instead of a grep + read loop. Dependencies' .d.ts and cross-stack matches rank lower.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query":        { "type": "string",  "description": "Natural-language question or a bag of symbol/file names." },
                    "max_files":    { "type": "integer", "description": "Max source files to include (default 6)." },
                    "rwr":          { "type": "boolean", "description": "Re-rank by an in-memory call/inheritance graph to surface callers/subclasses; slightly slower." },
                    "project_root": { "type": "string",  "description": "Absolute path to project root. Optional." },
                    "format":       { "type": "string",  "enum": ["text", "json"], "description": "Output format. Default 'text' (compact, token-efficient)." }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "search",
            "description": "Literal search over paths, definitions, imports/usages and file contents, for a known identifier or path fragment (`UserService`, `auth/`). A question goes to `explore` (a multi-word query without literal hits falls back to it). Choosing among several matches? Add `rank`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query":        { "type": "string", "description": "Search query. Comma-separated for OR: 'email,mail'." },
                    "limit":        { "type": "integer", "description": "Max results per category (default 20)." },
                    "kind":         { "type": "string",  "description": "Filter symbols by kind: class, interface, function, method, struct, enum, etc." },
                    "in_file":      { "type": "string",  "description": "Restrict to files whose path contains this substring." },
                    "module":       { "type": "string",  "description": "Restrict to files whose path starts with this prefix." },
                    "fuzzy":        { "type": "boolean", "description": "Enable typo-tolerant fuzzy matching." },
                    "rank":         { "type": "string",  "enum": ["proven", "hotspots", "risky", "central"], "description": "Re-order by Git history and the symbol graph, evidence per result. proven: safest to copy; risky: dangerous to change; hotspots: keeps being changed and fixed; central: what the code leans on. Exact names stay first. Needs `ast-index hotspots --collect` in a shell (not for central) and `graph_build` (not for hotspots); what is missing is reported." },
                    "exclude_tests": { "type": "boolean", "description": "With `rank`: leave test files out of the ranked results (they crowd `hotspots` and `risky`)." },
                    "project_root": { "type": "string",  "description": "Absolute path to project root. Optional if the server was started with --root or AST_INDEX_ROOT." },
                    "format":       { "type": "string",  "enum": ["text", "json"], "description": "Default 'text' (compact); 'json' costs ~2-3× the tokens." }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "outline",
            "description": "Extract the structural outline (classes, functions, methods with line numbers) of a single source file. ALWAYS call this BEFORE reading a file larger than 500 lines — then read only the targeted slice by offset/limit instead of the whole file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file":         { "type": "string", "description": "Path to the source file (relative to project root or absolute)." },
                    "project_root": { "type": "string", "description": "Absolute path to project root. Optional." },
                    "format":       { "type": "string", "enum": ["text", "json"], "description": "Output format. Default 'text' (compact)." }
                },
                "required": ["file"]
            }
        }),
        json!({
            "name": "usages",
            "description": "Every indexed reference to a symbol name (call, type use, import, …) as file:line + the line's text — for 'who uses X'. Matched by name, not resolved by type: same-named symbols are mixed, and a mention in a comment or string can slip in. For the dependents of one definition use `graph_dependents`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "symbol":       { "type": "string",  "description": "Symbol name (class, function, method)." },
                    "limit":        { "type": "integer", "description": "Max results (default 50)." },
                    "in_file":      { "type": "string",  "description": "Restrict to files whose path contains this substring." },
                    "module":       { "type": "string",  "description": "Restrict to files whose path starts with this prefix." },
                    "project_root": { "type": "string",  "description": "Absolute path to project root. Optional." },
                    "format":       { "type": "string",  "enum": ["text", "json"], "description": "Default 'text' (compact); 'json' costs ~2-3× the tokens." }
                },
                "required": ["symbol"]
            }
        }),
        json!({
            "name": "callers",
            "description": "Call sites of a function by name: matching lines (`name(`, `.name`, …) as file:line + text — for 'where is X called'. It does not name the calling function; for that, and for callers of callers, use `call_tree`. A text match at query time: same-named methods, comments and strings can appear.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "function":     { "type": "string",  "description": "Function or method name." },
                    "limit":        { "type": "integer", "description": "Max results (default 50)." },
                    "project_root": { "type": "string",  "description": "Absolute path to project root. Optional." },
                    "format":       { "type": "string",  "enum": ["text", "json"], "description": "Default 'text' (compact); 'json' costs ~2-3× the tokens." }
                },
                "required": ["function"]
            }
        }),
        json!({
            "name": "implementations",
            "description": "Find every class/struct/type that implements (Java/Kotlin/Swift/Scala) or extends (C++, Rust trait, etc.) the given interface, protocol, or abstract class. Use this for 'what implements PaymentProcessing' questions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "parent":       { "type": "string",  "description": "Name of the interface, protocol, trait, or abstract class." },
                    "limit":        { "type": "integer", "description": "Max results (default 50)." },
                    "in_file":      { "type": "string",  "description": "Restrict to files whose path contains this substring." },
                    "module":       { "type": "string",  "description": "Restrict to files whose path starts with this prefix." },
                    "project_root": { "type": "string",  "description": "Absolute path to project root. Optional." },
                    "format":       { "type": "string",  "enum": ["text", "json"], "description": "Default 'text' (compact); 'json' costs ~2-3× the tokens." }
                },
                "required": ["parent"]
            }
        }),
        json!({
            "name": "refs",
            "description": "Show cross-references for a symbol in one shot: every definition, every import, every usage. Use this when you want the complete picture in a single response, rather than calling `usages` / `callers` separately. Deduplicated and grouped by kind; usages are matched by name, as in `usages`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "symbol":       { "type": "string",  "description": "Symbol name." },
                    "limit":        { "type": "integer", "description": "Max results per category (default 50)." },
                    "project_root": { "type": "string",  "description": "Absolute path to project root. Optional." },
                    "format":       { "type": "string",  "enum": ["text", "json"], "description": "Default 'text' (compact); 'json' costs ~2-3× the tokens." }
                },
                "required": ["symbol"]
            }
        }),
        json!({
            "name": "rebuild",
            "description": "Rebuild the code index from scratch. Only needed on first setup or if `update` (incremental) is producing stale results. This can take minutes on large repositories — prefer `update` for everyday use (run it manually between sessions, NOT via this tool).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_root": { "type": "string", "description": "Absolute path to project root. Optional." },
                    "format":       { "type": "string", "enum": ["text", "json"], "description": "Output format. Default 'text' (compact)." }
                }
            }
        }),
        json!({
            "name": "find_file",
            "description": "Find files in the indexed project by name pattern. Much cheaper than listing a directory tree when you only need a few matches (e.g. 'where is PaymentViewModel.kt').",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pattern":      { "type": "string",  "description": "File name substring, or full name when 'exact' is true." },
                    "exact":        { "type": "boolean", "description": "Match the file name exactly instead of as a substring." },
                    "limit":        { "type": "integer", "description": "Max results (default 20)." },
                    "project_root": { "type": "string",  "description": "Absolute path to project root. Optional." }
                },
                "required": ["pattern"]
            }
        }),
        json!({
            "name": "stats",
            "description": "Show index statistics: detected project type, counts of files / symbols / refs / modules, DB size, extra roots. Call this to verify the index is populated and up-to-date before other queries.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_root": { "type": "string", "description": "Absolute path to project root. Optional." },
                    "format":       { "type": "string", "enum": ["text", "json"], "description": "Output format. Default 'text' (compact)." }
                }
            }
        }),
        json!({
            "name": "update",
            "description": "Incrementally update the code index — reindex only changed and deleted files since the last run. Fast (seconds) even on large repos. Call this instead of `rebuild` whenever you suspect the index is slightly stale.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_root": { "type": "string", "description": "Absolute path to project root. Optional." }
                }
            }
        }),
        json!({
            "name": "symbol",
            "description": "Find symbols by exact name or glob pattern, optionally filtered by kind (class/function/method/struct/etc). Sharper than `search` when you know what you're looking for. Use `search` for broad discovery; use this when the name or pattern is specific.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name":         { "type": "string",  "description": "Exact symbol name. Use either 'name' or 'pattern', not both." },
                    "pattern":      { "type": "string",  "description": "Glob pattern for symbol name (e.g. '*Service', '*Email*')." },
                    "kind":         { "type": "string",  "description": "Filter by symbol kind: class, interface, function, method, struct, enum, etc." },
                    "limit":        { "type": "integer", "description": "Max results (default 50)." },
                    "in_file":      { "type": "string",  "description": "Restrict to files whose path contains this substring." },
                    "module":       { "type": "string",  "description": "Restrict to files whose path starts with this prefix." },
                    "fuzzy":        { "type": "boolean", "description": "Enable typo-tolerant fuzzy matching." },
                    "project_root": { "type": "string",  "description": "Absolute path to project root. Optional." },
                    "format":       { "type": "string",  "enum": ["text", "json"], "description": "Output format. Default 'text'." }
                }
            }
        }),
        json!({
            "name": "class",
            "description": "Find classes, interfaces, objects, enums, protocols, structs, actors, or packages by name or glob pattern. A type-filtered `symbol` lookup. Use this for 'where is class X defined' questions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name":         { "type": "string",  "description": "Class/interface/type name. Use either 'name' or 'pattern', not both." },
                    "pattern":      { "type": "string",  "description": "Glob pattern (e.g. '*Controller', '*Handler*')." },
                    "limit":        { "type": "integer", "description": "Max results (default 50)." },
                    "in_file":      { "type": "string",  "description": "Restrict to files whose path contains this substring." },
                    "module":       { "type": "string",  "description": "Restrict to files whose path starts with this prefix." },
                    "fuzzy":        { "type": "boolean", "description": "Enable typo-tolerant fuzzy matching." },
                    "project_root": { "type": "string",  "description": "Absolute path to project root. Optional." },
                    "format":       { "type": "string",  "enum": ["text", "json"], "description": "Output format. Default 'text'." }
                }
            }
        }),
        json!({
            "name": "hierarchy",
            "description": "Show the inheritance tree for a class — both its superclasses/protocols it conforms to AND its subclasses/implementors. Complements `implementations` (which only shows one direction). Use this to understand the full inheritance neighborhood of a type.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name":         { "type": "string", "description": "Class, interface, protocol, trait, or abstract class name." },
                    "in_file":      { "type": "string", "description": "Restrict to a specific file — useful when multiple classes share the same name (e.g. inner DTOs)." },
                    "module":       { "type": "string", "description": "Restrict to files whose path starts with this prefix." },
                    "project_root": { "type": "string", "description": "Absolute path to project root. Optional." }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "imports",
            "description": "List all imports / uses / includes declared in a source file. Fast way to understand a file's dependency fan-out without reading the file itself.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file":         { "type": "string", "description": "Path to the source file (relative to project root or absolute)." },
                    "project_root": { "type": "string", "description": "Absolute path to project root. Optional." }
                },
                "required": ["file"]
            }
        }),
        json!({
            "name": "api",
            "description": "Show the public API (exported symbols) of a module — classes, functions, interfaces visible from outside the module. Use this when planning a refactor or writing a changelog entry.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "module_path":  { "type": "string",  "description": "Module path or directory prefix (e.g. 'src/auth', 'com.example.billing')." },
                    "limit":        { "type": "integer", "description": "Max results (default 100)." },
                    "project_root": { "type": "string",  "description": "Absolute path to project root. Optional." }
                },
                "required": ["module_path"]
            }
        }),
        json!({
            "name": "changed",
            "description": "Summarize files changed on the current Git/Arc branch from merge-base(base, HEAD) to HEAD; staged and unstaged working-tree edits are not included. Returns repo-relative current paths with A/M/D/R statuses and `old_path` for renames. This is branch-level file metadata, not changed symbols or a replacement for a raw VCS diff. It is independent of the ast-index database/cache; `project_root` sets the working-directory scope.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "base":         { "type": "string",  "description": "Base branch or revision. The comparison is merge-base(base, HEAD) to HEAD. Omit to resolve Git origin/HEAD, then try origin/main, origin/master, main, master, and trunk; Arc uses trunk." },
                    "timeout_ms":   { "type": "integer", "description": "VCS command timeout in milliseconds (default 30000)." },
                    "project_root": { "type": "string",  "description": "Working directory used as the scope. Results are limited to this subtree but paths remain repository-relative. Optional." },
                    "format":       { "type": "string",  "enum": ["text", "json"], "description": "Output format. Default 'text' (compact A/M/D/R summary); pass 'json' for schema v1." }
                }
            }
        }),
        json!({
            "name": "module",
            "description": "Find modules matching a pattern. A module is the coarse unit above file — Gradle subproject, Cargo crate, Python package, Go package, etc. Use this to orient yourself in a large monorepo before drilling into files.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pattern":      { "type": "string",  "description": "Glob or substring to match module paths." },
                    "limit":        { "type": "integer", "description": "Max results (default 50)." },
                    "project_root": { "type": "string",  "description": "Absolute path to project root. Optional." }
                },
                "required": ["pattern"]
            }
        }),
        json!({
            "name": "deps",
            "description": "Show what a given module depends on (its dependency list). Complements `dependents` which goes the other direction. Use for 'what does moduleX pull in' questions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "module":       { "type": "string", "description": "Module path (as returned by `module`)." },
                    "project_root": { "type": "string", "description": "Absolute path to project root. Optional." }
                },
                "required": ["module"]
            }
        }),
        json!({
            "name": "dependents",
            "description": "Reverse-deps: which modules depend on this one. Use this for impact analysis — 'if I refactor module X, what else breaks'. Critical before any module-level API change.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "module":       { "type": "string", "description": "Module path." },
                    "project_root": { "type": "string", "description": "Absolute path to project root. Optional." }
                },
                "required": ["module"]
            }
        }),
        json!({
            "name": "call_tree",
            "description": "Recursive caller tree — shows callers of a function, then THEIR callers, up to a configurable depth. Use for understanding how deep a function's usage reaches without chasing references by hand.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "function":     { "type": "string",  "description": "Function or method name." },
                    "depth":        { "type": "integer", "description": "Max tree depth (default 3)." },
                    "limit":        { "type": "integer", "description": "Max callers per level (default 10)." },
                    "project_root": { "type": "string",  "description": "Absolute path to project root. Optional." }
                },
                "required": ["function"]
            }
        }),
        json!({
            "name": "graph_dependents",
            "description": "Call BEFORE changing a symbol's name, signature or behaviour: who depends on it, from the precomputed symbol graph. `depth` 1 (default): direct dependents with resolution confidence; 2-3: transitive blast radius per hop. Unlike `usages`, edges point at one definition, so same-named symbols stay out. Best resolved for Ruby and JS/TS; elsewhere confirm 'no dependents' with `usages`. Missing or stale graph: add `refresh: true`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "symbol":            { "type": "string",  "description": "Name, `Outer::Name`, or `Class#member` (e.g. `Billing::Invoice#total`)." },
                    "depth":             { "type": "integer", "description": "1 (default): direct dependents. 2+: transitive impact per hop." },
                    "members":           { "type": "boolean", "description": "For a class, also cover the definitions inside it (the class alone owns only superclass/mixin references)." },
                    "include_ambiguous": { "type": "boolean", "description": "Also list/follow references whose name matched several definitions (upper bound)." },
                    "exclude_tests":     { "type": "boolean", "description": "Leave out dependents defined in test files (spec/, tests/, *_test.*, *.spec.*); with depth 2+ they are not followed either." },
                    "in_file":           { "type": "string",  "description": "Only definitions whose path contains this substring." },
                    "kind":              { "type": "string",  "description": "Only definitions of this kind: class, function, property, ..." },
                    "limit":             { "type": "integer", "description": "Max rows (default 50)." },
                    "refresh":           { "type": "boolean", "description": "Rebuild the graph first if it is missing or stale; no-op when fresh." },
                    "project_root":      { "type": "string",  "description": "Absolute path to project root. Optional." },
                    "format":            { "type": "string",  "enum": ["text", "json"], "description": "Default 'text' (compact). 'json' = raw CLI JSON, ~2-3× the tokens." }
                },
                "required": ["symbol"]
            }
        }),
        json!({
            "name": "graph_dependencies",
            "description": "What a symbol's definition references (classes, methods, constants), each resolved to one definition with confidence — what it pulls in before you move, extract or stub it. For a class pass `members: true` (calls live in its methods). Same graph, caveats and `refresh` as `graph_dependents`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "symbol":            { "type": "string",  "description": "Name, `Outer::Name`, or `Class#member`." },
                    "members":           { "type": "boolean", "description": "For a class, also cover every definition inside it." },
                    "include_ambiguous": { "type": "boolean", "description": "Also list references whose name matched several definitions." },
                    "in_file":           { "type": "string",  "description": "Only definitions whose path contains this substring." },
                    "kind":              { "type": "string",  "description": "Only definitions of this kind: class, function, property, ..." },
                    "limit":             { "type": "integer", "description": "Max rows (default 50)." },
                    "refresh":           { "type": "boolean", "description": "Rebuild the graph first if it is missing or stale." },
                    "project_root":      { "type": "string",  "description": "Absolute path to project root. Optional." },
                    "format":            { "type": "string",  "enum": ["text", "json"], "description": "Default 'text' (compact). 'json' = raw CLI JSON." }
                },
                "required": ["symbol"]
            }
        }),
        json!({
            "name": "graph_path",
            "description": "How does A reach B? Shortest dependency path(s) from `from` to `to`, hop by hop with edge confidence. A class stands for itself and its methods (`contains` hops); if only `to` reaches `from`, the reverse path is shown. Same graph and `refresh` as `graph_dependents`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from":              { "type": "string",  "description": "Starting symbol (e.g. an entry point)." },
                    "to":                { "type": "string",  "description": "Destination symbol." },
                    "from_file":         { "type": "string",  "description": "Only `from` definitions whose path contains this substring." },
                    "to_file":           { "type": "string",  "description": "Only `to` definitions whose path contains this substring." },
                    "max_depth":         { "type": "integer", "description": "Give up beyond this many hops (default 8)." },
                    "max_paths":         { "type": "integer", "description": "Max shortest paths to list (default 3)." },
                    "include_ambiguous": { "type": "boolean", "description": "Also follow references whose name matched several definitions." },
                    "refresh":           { "type": "boolean", "description": "Rebuild the graph first if it is missing or stale." },
                    "project_root":      { "type": "string",  "description": "Absolute path to project root. Optional." },
                    "format":            { "type": "string",  "enum": ["text", "json"], "description": "Default 'text' (compact). 'json' = raw CLI JSON." }
                },
                "required": ["from", "to"]
            }
        }),
        json!({
            "name": "graph_metrics",
            "description": "Centrality from the symbol graph. Without `symbols`: the most central symbols, to orient in an unfamiliar repo. With `symbols`: their fan-in, fan-out, dependents and PageRank percentile — how load-bearing before you touch them. Same graph and `refresh` as `graph_dependents`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "symbols":       { "type": "array", "items": { "type": "string" }, "description": "Symbols to measure. Omit to list the most central ones." },
                    "sort":          { "type": "string",  "enum": ["pagerank", "fan-in", "fan-out", "dependents"], "description": "Top list only: ranking key (default pagerank)." },
                    "kind":          { "type": "string",  "description": "Only symbols of this kind: class, function, ..." },
                    "path":          { "type": "string",  "description": "Top list only: symbols under this path prefix." },
                    "exclude_tests": { "type": "boolean", "description": "Top list only: skip symbols defined in test files." },
                    "in_file":       { "type": "string",  "description": "With `symbols` only: definitions whose path contains this substring." },
                    "limit":         { "type": "integer", "description": "Max rows (default 20 for the top list, 50 with `symbols`)." },
                    "refresh":       { "type": "boolean", "description": "Rebuild the graph first if it is missing or stale." },
                    "project_root":  { "type": "string",  "description": "Absolute path to project root. Optional." },
                    "format":        { "type": "string",  "enum": ["text", "json"], "description": "Default 'text' (compact). 'json' = raw CLI JSON." }
                }
            }
        }),
        json!({
            "name": "graph_cycles",
            "description": "Dependency cycles (strongly connected components over resolved edges), largest first, each with one concrete cycle — for untangling architecture. Same graph and `refresh` as `graph_dependents`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path":         { "type": "string",  "description": "Only components with a member under this path prefix." },
                    "min_size":     { "type": "integer", "description": "Smallest component size to report (default 2)." },
                    "limit":        { "type": "integer", "description": "Max components (default 20)." },
                    "refresh":      { "type": "boolean", "description": "Rebuild the graph first if it is missing or stale." },
                    "project_root": { "type": "string",  "description": "Absolute path to project root. Optional." },
                    "format":       { "type": "string",  "enum": ["text", "json"], "description": "Default 'text' (compact). 'json' = raw CLI JSON." }
                }
            }
        }),
        json!({
            "name": "graph_build",
            "description": "Build the symbol graph that the `graph_*` tools and `search` `rank` (proven, risky, central) read — only when one of them reports it missing or stale (`graph_*` tools can take `refresh: true` instead). Seconds even on a large monorepo; `rebuild` and `update` never build it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_root": { "type": "string", "description": "Absolute path to project root. Optional." },
                    "format":       { "type": "string", "enum": ["text", "json"], "description": "Default 'text' (compact). 'json' = raw CLI JSON." }
                }
            }
        }),
        json!({
            "name": "hotspots",
            "description": "Git-history risk per file — commits, churn, bugfix share, authors, age, last change — labelled by percentiles within this repository. Use before editing a file, when scoping a refactor, or with `sort: fixes` to find where bugs keep landing. Reads collected history only: collect with `ast-index hotspots --collect` in a shell (the first run reads the whole history, later ones are incremental).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sort":         { "type": "string",  "enum": ["score", "commits", "churn", "relative-churn", "fixes", "authors", "recent"], "description": "Ranking key. score (default): mean percentile of commits, churn and bugfix ratio; fixes: bugfix share discounted for thin history." },
                    "path":         { "type": "string",  "description": "Only files whose path starts with this prefix." },
                    "min_commits":  { "type": "integer", "description": "Skip files with fewer commits (default 1)." },
                    "exclude_tests": { "type": "boolean", "description": "Leave test files out of the list; percentiles still rank every file." },
                    "limit":        { "type": "integer", "description": "Max files (default 20)." },
                    "project_root": { "type": "string",  "description": "Absolute path to project root. Optional." },
                    "format":       { "type": "string",  "enum": ["text", "json"], "description": "Default 'text' (compact). 'json' = raw CLI JSON." }
                }
            }
        }),
    ]
}

fn call_tool(params: Value, ast_index_bin: &str, default_root: &PathBuf) -> Result<String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing 'name' in tools/call params"))?;
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    let resolved_root = arguments
        .get("project_root")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| default_root.clone());

    // Default output format is compact text (token-efficient). Agents can
    // request raw JSON via `format: "json"` when they need structured
    // parsing — cost is ~2-3× more tokens.
    let output_format = arguments
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("text");

    let argv = build_argv(name, &arguments)?;

    let output = Command::new(ast_index_bin)
        .args(&argv)
        .current_dir(&resolved_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("failed to spawn {ast_index_bin} — is it on PATH?"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("ast-index exited with {}: {stderr}", output.status));
    }

    let stdout = String::from_utf8(output.stdout).context("ast-index produced non-UTF8 output")?;

    Ok(render_tool_output(name, output_format, stdout))
}

fn render_tool_output(tool: &str, output_format: &str, stdout: String) -> String {
    match output_format {
        "json" => stdout,
        _ => format::to_compact(tool, &stdout),
    }
}

/// Whether the underlying ast-index command honours `--format json`.
/// Commands not in this set print plain text regardless of the flag, so
/// we avoid passing it to keep the argv honest.
fn supports_json_format(tool: &str) -> bool {
    matches!(
        tool,
        "explore"
            | "search"
            | "usages"
            | "callers"
            | "implementations"
            | "refs"
            | "stats"
            | "symbol"
            | "class"
            | "changed"
            | "hotspots"
            | "graph_dependents"
            | "graph_dependencies"
            | "graph_path"
            | "graph_metrics"
            | "graph_cycles"
            | "graph_build"
    )
}

const RANK_PRESETS: [&str; 4] = ["proven", "hotspots", "risky", "central"];

/// Translate an MCP `tools/call` invocation into the equivalent
/// `ast-index <subcommand> <args> [--format json]` argv. Pure function —
/// no I/O, suitable for unit testing.
pub fn build_argv(name: &str, arguments: &Value) -> Result<Vec<String>> {
    let mut argv: Vec<String> = Vec::new();
    match name {
        "explore" => {
            argv.push("explore".into());
            // query is a single string; the CLI tokenizes it into terms.
            argv.push(require_string(&arguments, "query")?);
            push_if_num(&mut argv, &arguments, "max_files", "--max-files");
            if arguments
                .get("rwr")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                argv.push("--rwr".into());
            }
        }
        "search" => {
            argv.push("search".into());
            argv.push(require_string(&arguments, "query")?);
            push_if_num(&mut argv, &arguments, "limit", "--limit");
            push_if_str(&mut argv, &arguments, "kind", "--type");
            push_if_str(&mut argv, &arguments, "in_file", "--in-file");
            push_if_str(&mut argv, &arguments, "module", "--module");
            if arguments
                .get("fuzzy")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                argv.push("--fuzzy".into());
            }
            if let Some(preset) = arguments.get("rank").and_then(Value::as_str) {
                if !RANK_PRESETS.contains(&preset) {
                    return Err(anyhow!(
                        "'rank' must be one of: {}",
                        RANK_PRESETS.join(", ")
                    ));
                }
                argv.push("--rank".into());
                argv.push(preset.into());
            }
            if arguments
                .get("exclude_tests")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                if arguments.get("rank").and_then(Value::as_str).is_none() {
                    return Err(anyhow!(
                        "'exclude_tests' applies to ranked search; add 'rank'"
                    ));
                }
                argv.push("--exclude-tests".into());
            }
        }
        "outline" => {
            argv.push("outline".into());
            argv.push(require_string(&arguments, "file")?);
        }
        "usages" => {
            argv.push("usages".into());
            argv.push(require_string(&arguments, "symbol")?);
            push_if_num(&mut argv, &arguments, "limit", "--limit");
            push_if_str(&mut argv, &arguments, "in_file", "--in-file");
            push_if_str(&mut argv, &arguments, "module", "--module");
        }
        "callers" => {
            argv.push("callers".into());
            argv.push(require_string(&arguments, "function")?);
            push_if_num(&mut argv, &arguments, "limit", "--limit");
        }
        "implementations" => {
            argv.push("implementations".into());
            argv.push(require_string(&arguments, "parent")?);
            push_if_num(&mut argv, &arguments, "limit", "--limit");
            push_if_str(&mut argv, &arguments, "in_file", "--in-file");
            push_if_str(&mut argv, &arguments, "module", "--module");
        }
        "refs" => {
            argv.push("refs".into());
            argv.push(require_string(&arguments, "symbol")?);
            push_if_num(&mut argv, &arguments, "limit", "--limit");
        }
        "rebuild" => {
            argv.push("rebuild".into());
        }
        "find_file" => {
            argv.push("file".into());
            argv.push(require_string(&arguments, "pattern")?);
            if arguments
                .get("exact")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                argv.push("--exact".into());
            }
            push_if_num(&mut argv, &arguments, "limit", "--limit");
        }
        "stats" => {
            argv.push("stats".into());
        }
        "update" => {
            argv.push("update".into());
        }
        "symbol" => {
            argv.push("symbol".into());
            if let Some(n) = arguments.get("name").and_then(Value::as_str) {
                argv.push(n.into());
            }
            push_if_str(&mut argv, &arguments, "pattern", "--pattern");
            push_if_str(&mut argv, &arguments, "kind", "--type");
            push_if_num(&mut argv, &arguments, "limit", "--limit");
            push_if_str(&mut argv, &arguments, "in_file", "--in-file");
            push_if_str(&mut argv, &arguments, "module", "--module");
            if arguments
                .get("fuzzy")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                argv.push("--fuzzy".into());
            }
        }
        "class" => {
            argv.push("class".into());
            if let Some(n) = arguments.get("name").and_then(Value::as_str) {
                argv.push(n.into());
            }
            push_if_str(&mut argv, &arguments, "pattern", "--pattern");
            push_if_num(&mut argv, &arguments, "limit", "--limit");
            push_if_str(&mut argv, &arguments, "in_file", "--in-file");
            push_if_str(&mut argv, &arguments, "module", "--module");
            if arguments
                .get("fuzzy")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                argv.push("--fuzzy".into());
            }
        }
        "hierarchy" => {
            argv.push("hierarchy".into());
            argv.push(require_string(&arguments, "name")?);
            push_if_str(&mut argv, &arguments, "in_file", "--in-file");
            push_if_str(&mut argv, &arguments, "module", "--module");
        }
        "imports" => {
            argv.push("imports".into());
            argv.push(require_string(&arguments, "file")?);
        }
        "api" => {
            argv.push("api".into());
            argv.push(require_string(&arguments, "module_path")?);
            push_if_num(&mut argv, &arguments, "limit", "--limit");
        }
        "changed" => {
            argv.push("changed".into());
            push_if_str(&mut argv, arguments, "base", "--base");
            push_if_num(&mut argv, arguments, "timeout_ms", "--timeout-ms");
        }
        "module" => {
            argv.push("module".into());
            argv.push(require_string(&arguments, "pattern")?);
            push_if_num(&mut argv, &arguments, "limit", "--limit");
        }
        "deps" => {
            argv.push("deps".into());
            argv.push(require_string(&arguments, "module")?);
        }
        "dependents" => {
            argv.push("dependents".into());
            argv.push(require_string(&arguments, "module")?);
        }
        "call_tree" => {
            argv.push("call-tree".into());
            argv.push(require_string(&arguments, "function")?);
            push_if_num(&mut argv, &arguments, "depth", "--depth");
            push_if_num(&mut argv, &arguments, "limit", "--limit");
        }
        "graph_dependents" => {
            let symbol = require_string(arguments, "symbol")?;
            let depth = arguments
                .get("depth")
                .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
                .unwrap_or(1);
            argv.push("graph".into());
            if depth > 1 {
                argv.extend(["impact".into(), symbol, "--depth".into(), depth.to_string()]);
            } else {
                argv.extend(["dependents".into(), symbol]);
            }
            push_graph_symbol_filters(&mut argv, arguments);
            push_if_flag(&mut argv, arguments, "exclude_tests", "--exclude-tests");
        }
        "graph_dependencies" => {
            argv.extend([
                "graph".into(),
                "dependencies".into(),
                require_string(arguments, "symbol")?,
            ]);
            push_graph_symbol_filters(&mut argv, arguments);
        }
        "graph_path" => {
            argv.extend([
                "graph".into(),
                "path".into(),
                require_string(arguments, "from")?,
                require_string(arguments, "to")?,
            ]);
            push_if_str(&mut argv, arguments, "from_file", "--from-file");
            push_if_str(&mut argv, arguments, "to_file", "--to-file");
            push_if_num(&mut argv, arguments, "max_depth", "--max-depth");
            push_if_num(&mut argv, arguments, "max_paths", "--max-paths");
            push_if_flag(
                &mut argv,
                arguments,
                "include_ambiguous",
                "--include-ambiguous",
            );
            push_if_flag(&mut argv, arguments, "refresh", "--refresh");
        }
        "graph_metrics" => {
            let symbols = string_list(arguments, "symbols");
            argv.push("graph".into());
            if symbols.is_empty() {
                if is_set(arguments, "in_file") {
                    return Err(anyhow!(
                        "'in_file' applies only with 'symbols'; filter the top list with 'path'"
                    ));
                }
                argv.push("top".into());
                push_if_str(&mut argv, arguments, "sort", "--sort");
                push_if_num(&mut argv, arguments, "limit", "--limit");
                push_if_str(&mut argv, arguments, "kind", "--kind");
                push_if_str(&mut argv, arguments, "path", "--path");
                push_if_flag(&mut argv, arguments, "exclude_tests", "--exclude-tests");
            } else {
                if let Some(key) = ["sort", "path", "exclude_tests"]
                    .into_iter()
                    .find(|key| is_set(arguments, key))
                {
                    return Err(anyhow!(
                        "'{key}' applies only to the top list; omit 'symbols' or drop '{key}'"
                    ));
                }
                argv.push("metrics".into());
                argv.extend(symbols);
                push_if_str(&mut argv, arguments, "in_file", "--in-file");
                push_if_str(&mut argv, arguments, "kind", "--kind");
                push_if_num(&mut argv, arguments, "limit", "--limit");
            }
            push_if_flag(&mut argv, arguments, "refresh", "--refresh");
        }
        "graph_cycles" => {
            argv.extend(["graph".into(), "cycles".into()]);
            push_if_num(&mut argv, arguments, "limit", "--limit");
            push_if_num(&mut argv, arguments, "min_size", "--min-size");
            push_if_str(&mut argv, arguments, "path", "--path");
            push_if_flag(&mut argv, arguments, "refresh", "--refresh");
        }
        "graph_build" => {
            argv.extend(["graph".into(), "build".into()]);
        }
        "hotspots" => {
            // Collection is deliberately not reachable from here: a first or
            // reset run walks the whole history, longer than common MCP client
            // timeouts, and this server handles one request at a time.
            argv.push("hotspots".into());
            push_if_num(&mut argv, arguments, "limit", "--limit");
            push_if_num(&mut argv, arguments, "min_commits", "--min-commits");
            push_if_str(&mut argv, arguments, "path", "--path");
            push_if_str(&mut argv, arguments, "sort", "--sort");
            push_if_flag(&mut argv, arguments, "exclude_tests", "--exclude-tests");
        }
        other => return Err(anyhow!("unknown tool: {other}")),
    }

    if supports_json_format(name) {
        argv.push("--format".into());
        argv.push("json".into());
    }
    Ok(argv)
}

fn require_string(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("missing required argument '{key}'"))
}

fn push_if_str(argv: &mut Vec<String>, args: &Value, key: &str, flag: &str) {
    if let Some(s) = args.get(key).and_then(Value::as_str) {
        argv.push(flag.into());
        argv.push(s.into());
    }
}

fn push_if_num(argv: &mut Vec<String>, args: &Value, key: &str, flag: &str) {
    if let Some(n) = args
        .get(key)
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
    {
        argv.push(flag.into());
        argv.push(n.to_string());
    }
}

fn push_if_flag(argv: &mut Vec<String>, args: &Value, key: &str, flag: &str) {
    if args.get(key).and_then(Value::as_bool).unwrap_or(false) {
        argv.push(flag.into());
    }
}

/// Filters shared by the graph queries that take one symbol spec.
fn push_graph_symbol_filters(argv: &mut Vec<String>, args: &Value) {
    push_if_str(argv, args, "in_file", "--in-file");
    push_if_str(argv, args, "kind", "--kind");
    push_if_flag(argv, args, "members", "--members");
    push_if_flag(argv, args, "include_ambiguous", "--include-ambiguous");
    push_if_num(argv, args, "limit", "--limit");
    push_if_flag(argv, args, "refresh", "--refresh");
}

/// Whether an optional argument carries a value that would change the query:
/// `false`, `null` and `""` are what clients send for "not set".
fn is_set(args: &Value, key: &str) -> bool {
    match args.get(key) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(Value::String(value)) => !value.is_empty(),
        Some(_) => true,
    }
}

/// A string-array argument; a lone string counts as a one-element list.
fn string_list(args: &Value, key: &str) -> Vec<String> {
    match args.get(key) {
        Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // --- tool_descriptors metadata ---

    #[test]
    fn descriptors_expose_exactly_twentyeight_tools() {
        let names: Vec<String> = tool_descriptors()
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string))
            .collect();
        assert_eq!(names.len(), 28, "MCP must expose 28 tools, got {names:?}");
    }

    #[test]
    fn descriptor_names_are_unique() {
        let names: Vec<String> = tool_descriptors()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        let unique: HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "duplicate tool names: {names:?}");
    }

    #[test]
    fn every_descriptor_has_required_fields() {
        for tool in tool_descriptors() {
            let name = tool.get("name").and_then(Value::as_str).expect("name");
            assert!(
                tool.get("description").and_then(Value::as_str).is_some(),
                "tool {name} missing description"
            );
            let schema = tool
                .get("inputSchema")
                .and_then(Value::as_object)
                .unwrap_or_else(|| panic!("tool {name} missing inputSchema object"));
            assert_eq!(
                schema.get("type").and_then(Value::as_str),
                Some("object"),
                "tool {name} inputSchema.type must be 'object'"
            );
        }
    }

    #[test]
    fn descriptor_set_matches_dispatch() {
        // Every advertised tool must have a dispatch arm — try building argv
        // with minimum required args and assert it doesn't return "unknown tool".
        let stub_args = json!({
            "query": "x", "file": "f", "symbol": "s", "function": "f",
            "parent": "p", "name": "n", "module_path": "m",
            "pattern": "*", "module": "m", "from": "a", "to": "b"
        });
        for tool in tool_descriptors() {
            let name = tool["name"].as_str().unwrap();
            let result = build_argv(name, &stub_args);
            assert!(
                result.is_ok(),
                "tool {name} advertised but build_argv failed: {result:?}"
            );
        }
    }

    // --- build_argv per-tool ---

    #[test]
    fn search_minimal_args() {
        let argv = build_argv("search", &json!({"query": "Foo"})).unwrap();
        assert_eq!(argv, vec!["search", "Foo", "--format", "json"]);
    }

    #[test]
    fn search_full_args() {
        let argv = build_argv(
            "search",
            &json!({
                "query": "Foo", "limit": 100, "kind": "class",
                "in_file": "src/", "module": "core", "fuzzy": true
            }),
        )
        .unwrap();
        assert_eq!(
            argv,
            vec![
                "search",
                "Foo",
                "--limit",
                "100",
                "--type",
                "class",
                "--in-file",
                "src/",
                "--module",
                "core",
                "--fuzzy",
                "--format",
                "json",
            ]
        );
    }

    #[test]
    fn search_forwards_rank_preset_after_existing_flags() {
        for preset in RANK_PRESETS {
            let argv = build_argv(
                "search",
                &json!({"query": "Service", "fuzzy": true, "rank": preset}),
            )
            .unwrap();
            assert_eq!(
                argv,
                vec!["search", "Service", "--fuzzy", "--rank", preset, "--format", "json"]
            );
        }
    }

    #[test]
    fn search_without_rank_keeps_the_plain_argv() {
        let argv = build_argv("search", &json!({"query": "Foo", "limit": 5})).unwrap();
        assert!(!argv.contains(&"--rank".to_string()));
    }

    #[test]
    fn search_rejects_unknown_rank_preset() {
        let err = build_argv("search", &json!({"query": "Foo", "rank": "safest"})).unwrap_err();
        assert!(
            err.to_string().contains("proven, hotspots, risky, central"),
            "got: {err}"
        );
    }

    #[test]
    fn search_descriptor_advertises_the_four_presets() {
        let descriptor = tool_descriptors()
            .into_iter()
            .find(|tool| tool["name"] == "search")
            .unwrap();
        assert_eq!(
            descriptor["inputSchema"]["properties"]["rank"]["enum"],
            json!(RANK_PRESETS)
        );
    }

    // --- graph tools ---

    #[test]
    fn graph_dependents_defaults_to_direct_edges() {
        let argv = build_argv("graph_dependents", &json!({"symbol": "Invoice"})).unwrap();
        assert_eq!(
            argv,
            vec!["graph", "dependents", "Invoice", "--format", "json"]
        );
    }

    #[test]
    fn graph_dependents_depth_one_stays_on_direct_edges() {
        let argv = build_argv("graph_dependents", &json!({"symbol": "X", "depth": 1})).unwrap();
        assert_eq!(argv[1], "dependents");
        assert!(!argv.contains(&"--depth".to_string()));
    }

    #[test]
    fn graph_dependents_deeper_switches_to_impact_with_every_filter() {
        let argv = build_argv(
            "graph_dependents",
            &json!({
                "symbol": "Billing::Invoice#total", "depth": 3, "members": true,
                "include_ambiguous": true, "in_file": "app/models", "kind": "class",
                "limit": 10, "refresh": true, "exclude_tests": true
            }),
        )
        .unwrap();
        assert_eq!(
            argv,
            vec![
                "graph",
                "impact",
                "Billing::Invoice#total",
                "--depth",
                "3",
                "--in-file",
                "app/models",
                "--kind",
                "class",
                "--members",
                "--include-ambiguous",
                "--limit",
                "10",
                "--refresh",
                "--exclude-tests",
                "--format",
                "json",
            ]
        );
    }

    #[test]
    fn graph_dependents_forwards_exclude_tests_to_direct_edges() {
        let argv = build_argv(
            "graph_dependents",
            &json!({"symbol": "MergeService", "exclude_tests": true}),
        )
        .unwrap();
        assert_eq!(
            argv,
            vec![
                "graph",
                "dependents",
                "MergeService",
                "--exclude-tests",
                "--format",
                "json"
            ]
        );
    }

    #[test]
    fn graph_dependents_false_flags_are_not_forwarded() {
        let argv = build_argv(
            "graph_dependents",
            &json!({"symbol": "X", "members": false, "refresh": false, "exclude_tests": false}),
        )
        .unwrap();
        assert_eq!(argv, vec!["graph", "dependents", "X", "--format", "json"]);
    }

    #[test]
    fn graph_dependencies_forwards_members_and_filters() {
        let argv = build_argv(
            "graph_dependencies",
            &json!({"symbol": "OrdersController", "members": true, "limit": 5}),
        )
        .unwrap();
        assert_eq!(
            argv,
            vec![
                "graph",
                "dependencies",
                "OrdersController",
                "--members",
                "--limit",
                "5",
                "--format",
                "json"
            ]
        );
    }

    #[test]
    fn graph_symbol_tools_require_symbol() {
        for tool in ["graph_dependents", "graph_dependencies"] {
            let err = build_argv(tool, &json!({})).unwrap_err();
            assert!(err.to_string().contains("'symbol'"), "{tool}: {err}");
        }
    }

    #[test]
    fn graph_path_passes_both_ends_and_options() {
        let argv = build_argv(
            "graph_path",
            &json!({
                "from": "OrdersController", "to": "Invoice", "from_file": "app/controllers",
                "to_file": "app/models", "max_depth": 5, "max_paths": 2,
                "include_ambiguous": true, "refresh": true
            }),
        )
        .unwrap();
        assert_eq!(
            argv,
            vec![
                "graph",
                "path",
                "OrdersController",
                "Invoice",
                "--from-file",
                "app/controllers",
                "--to-file",
                "app/models",
                "--max-depth",
                "5",
                "--max-paths",
                "2",
                "--include-ambiguous",
                "--refresh",
                "--format",
                "json",
            ]
        );
    }

    #[test]
    fn graph_path_requires_from_and_to() {
        let err = build_argv("graph_path", &json!({"from": "A"})).unwrap_err();
        assert!(err.to_string().contains("'to'"), "got: {err}");
        let err = build_argv("graph_path", &json!({"to": "B"})).unwrap_err();
        assert!(err.to_string().contains("'from'"), "got: {err}");
    }

    #[test]
    fn graph_metrics_without_symbols_lists_the_top() {
        let argv = build_argv(
            "graph_metrics",
            &json!({
                "sort": "fan-in", "limit": 5, "kind": "class",
                "path": "app/", "exclude_tests": true, "refresh": true
            }),
        )
        .unwrap();
        assert_eq!(
            argv,
            vec![
                "graph",
                "top",
                "--sort",
                "fan-in",
                "--limit",
                "5",
                "--kind",
                "class",
                "--path",
                "app/",
                "--exclude-tests",
                "--refresh",
                "--format",
                "json",
            ]
        );
    }

    #[test]
    fn graph_metrics_with_symbols_measures_them() {
        let argv = build_argv(
            "graph_metrics",
            &json!({"symbols": ["Invoice", "Payment"], "in_file": "app/", "kind": "class"}),
        )
        .unwrap();
        assert_eq!(
            argv,
            vec![
                "graph",
                "metrics",
                "Invoice",
                "Payment",
                "--in-file",
                "app/",
                "--kind",
                "class",
                "--format",
                "json"
            ]
        );
    }

    #[test]
    fn graph_metrics_accepts_a_single_symbol_string() {
        let argv = build_argv("graph_metrics", &json!({"symbols": "Invoice"})).unwrap();
        assert_eq!(
            argv,
            vec!["graph", "metrics", "Invoice", "--format", "json"]
        );
    }

    #[test]
    fn graph_metrics_empty_symbols_means_top() {
        let argv = build_argv("graph_metrics", &json!({"symbols": []})).unwrap();
        assert_eq!(argv, vec!["graph", "top", "--format", "json"]);
    }

    #[test]
    fn graph_metrics_rejects_top_only_options_with_symbols() {
        for key in ["sort", "path", "exclude_tests"] {
            let mut args = json!({"symbols": ["Invoice"]});
            args[key] = json!("x");
            let err = build_argv("graph_metrics", &args).unwrap_err();
            assert!(err.to_string().contains(key), "{key}: {err}");
        }
    }

    #[test]
    fn graph_metrics_tolerates_unset_top_options_with_symbols() {
        let argv = build_argv(
            "graph_metrics",
            &json!({"symbols": ["X"], "exclude_tests": false, "sort": "", "path": null}),
        )
        .unwrap();
        assert_eq!(argv, vec!["graph", "metrics", "X", "--format", "json"]);
    }

    #[test]
    fn graph_metrics_rejects_in_file_without_symbols() {
        let err = build_argv("graph_metrics", &json!({"in_file": "app/"})).unwrap_err();
        assert!(err.to_string().contains("'path'"), "got: {err}");
    }

    #[test]
    fn graph_cycles_forwards_filters() {
        let argv = build_argv(
            "graph_cycles",
            &json!({"path": "app/models", "min_size": 3, "limit": 4, "refresh": true}),
        )
        .unwrap();
        assert_eq!(
            argv,
            vec![
                "graph",
                "cycles",
                "--limit",
                "4",
                "--min-size",
                "3",
                "--path",
                "app/models",
                "--refresh",
                "--format",
                "json"
            ]
        );
    }

    #[test]
    fn graph_build_takes_no_arguments() {
        let argv = build_argv("graph_build", &json!({"format": "text"})).unwrap();
        assert_eq!(argv, vec!["graph", "build", "--format", "json"]);
    }

    // --- hotspots ---

    #[test]
    fn hotspots_minimal_is_a_report() {
        let argv = build_argv("hotspots", &json!({})).unwrap();
        assert_eq!(argv, vec!["hotspots", "--format", "json"]);
    }

    #[test]
    fn hotspots_forwards_report_options() {
        let argv = build_argv(
            "hotspots",
            &json!({"limit": 5, "min_commits": 4, "path": "src/", "sort": "fixes"}),
        )
        .unwrap();
        assert_eq!(
            argv,
            vec![
                "hotspots",
                "--limit",
                "5",
                "--min-commits",
                "4",
                "--path",
                "src/",
                "--sort",
                "fixes",
                "--format",
                "json"
            ]
        );
    }

    #[test]
    fn hotspots_forwards_exclude_tests() {
        let argv = build_argv("hotspots", &json!({"exclude_tests": true})).unwrap();
        assert_eq!(
            argv,
            vec!["hotspots", "--exclude-tests", "--format", "json"]
        );
        let argv = build_argv("hotspots", &json!({"exclude_tests": false})).unwrap();
        assert!(!argv.contains(&"--exclude-tests".to_string()));
    }

    #[test]
    fn search_exclude_tests_needs_a_preset() {
        let argv = build_argv(
            "search",
            &json!({"query": "Merge", "rank": "risky", "exclude_tests": true}),
        )
        .unwrap();
        assert_eq!(
            argv,
            vec![
                "search",
                "Merge",
                "--rank",
                "risky",
                "--exclude-tests",
                "--format",
                "json"
            ]
        );
        let err =
            build_argv("search", &json!({"query": "Merge", "exclude_tests": true})).unwrap_err();
        assert!(err.to_string().contains("add 'rank'"), "{err}");
    }

    #[test]
    fn hotspots_never_collects() {
        let argv = build_argv(
            "hotspots",
            &json!({"collect": true, "full": true, "timeout_ms": 1}),
        )
        .unwrap();
        for flag in ["--collect", "--full", "--timeout-ms"] {
            assert!(!argv.contains(&flag.to_string()), "{flag} leaked: {argv:?}");
        }
    }

    #[test]
    fn hotspots_descriptor_says_how_to_collect() {
        let descriptor = tool_descriptors()
            .into_iter()
            .find(|tool| tool["name"] == "hotspots")
            .unwrap();
        let description = descriptor["description"].as_str().unwrap();
        assert!(description.contains("ast-index hotspots --collect"));
        assert!(descriptor["inputSchema"]["properties"]
            .get("collect")
            .is_none());
    }

    #[test]
    fn search_missing_required_query_errors() {
        let err = build_argv("search", &json!({})).unwrap_err();
        assert!(err.to_string().contains("'query'"), "got: {err}");
    }

    #[test]
    fn outline_passes_file_positionally_no_format_flag() {
        let argv = build_argv("outline", &json!({"file": "src/main.rs"})).unwrap();
        // outline does not advertise --format json
        assert_eq!(argv, vec!["outline", "src/main.rs"]);
    }

    #[test]
    fn class_with_pattern_and_fuzzy() {
        let argv = build_argv(
            "class",
            &json!({"pattern": "*Service", "fuzzy": true, "limit": 10}),
        )
        .unwrap();
        // class supports --format json
        assert_eq!(
            argv,
            vec![
                "class",
                "--pattern",
                "*Service",
                "--limit",
                "10",
                "--fuzzy",
                "--format",
                "json",
            ]
        );
    }

    #[test]
    fn symbol_with_name_first_then_flags() {
        let argv = build_argv("symbol", &json!({"name": "PathResolver", "kind": "class"})).unwrap();
        // name is positional, kind is --type, format=json appended
        assert_eq!(
            argv,
            vec![
                "symbol",
                "PathResolver",
                "--type",
                "class",
                "--format",
                "json"
            ]
        );
    }

    #[test]
    fn hierarchy_requires_name() {
        let err = build_argv("hierarchy", &json!({})).unwrap_err();
        assert!(err.to_string().contains("'name'"), "got: {err}");
    }

    #[test]
    fn hierarchy_no_format_flag() {
        // hierarchy is plain-text only
        let argv = build_argv("hierarchy", &json!({"name": "Foo"})).unwrap();
        assert_eq!(argv, vec!["hierarchy", "Foo"]);
    }

    #[test]
    fn call_tree_translates_underscore_to_hyphen() {
        let argv = build_argv(
            "call_tree",
            &json!({"function": "process", "depth": 4, "limit": 20}),
        )
        .unwrap();
        // MCP tool name has _, but ast-index subcommand is hyphenated
        assert_eq!(
            argv,
            vec!["call-tree", "process", "--depth", "4", "--limit", "20"]
        );
    }

    #[test]
    fn find_file_translates_to_file_subcommand() {
        let argv = build_argv(
            "find_file",
            &json!({"pattern": "*.rs", "exact": true, "limit": 50}),
        )
        .unwrap();
        // MCP tool name is find_file, ast-index subcommand is `file`
        assert_eq!(argv, vec!["file", "*.rs", "--exact", "--limit", "50"]);
    }

    #[test]
    fn changed_no_args_omits_base() {
        let argv = build_argv("changed", &json!({})).unwrap();
        assert_eq!(argv, vec!["changed", "--format", "json"]);
    }

    #[test]
    fn changed_forwards_base_timeout_and_internal_json_format() {
        let argv = build_argv(
            "changed",
            &json!({"base": "develop", "timeout_ms": 45000, "format": "text"}),
        )
        .unwrap();
        assert_eq!(
            argv,
            vec![
                "changed",
                "--base",
                "develop",
                "--timeout-ms",
                "45000",
                "--format",
                "json"
            ]
        );
    }

    #[test]
    fn changed_descriptor_documents_file_level_contract() {
        let descriptor = tool_descriptors()
            .into_iter()
            .find(|tool| tool["name"] == "changed")
            .unwrap();
        let description = descriptor["description"].as_str().unwrap();
        assert!(description.contains("A/M/D/R"));
        assert!(description.contains("old_path"));
        assert!(description.contains("independent"));
        assert!(!description.contains("List symbols"));
        let base_description = descriptor["inputSchema"]["properties"]["base"]["description"]
            .as_str()
            .unwrap();
        assert!(base_description.contains("origin/HEAD"));
        assert!(base_description.contains("Arc uses trunk"));
        assert_eq!(
            descriptor["inputSchema"]["properties"]["format"]["enum"],
            json!(["text", "json"])
        );
        assert_eq!(
            descriptor["inputSchema"]["properties"]["timeout_ms"]["type"],
            "integer"
        );
    }

    #[test]
    fn changed_raw_json_is_opt_in_and_preserved_verbatim() {
        let raw = concat!(
            "{\n",
            "  \"schema_version\": 1,\n",
            "  \"vcs\": \"git\",\n",
            "  \"base\": \"origin/main\",\n",
            "  \"head\": \"HEAD\",\n",
            "  \"scope\": null,\n",
            "  \"changes\": []\n",
            "}\n"
        )
        .to_string();
        assert_eq!(render_tool_output("changed", "json", raw.clone()), raw);
        assert_ne!(render_tool_output("changed", "text", raw.clone()), raw);
    }

    #[test]
    fn nonzero_child_exit_remains_an_mcp_tool_error() {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: "tools/call".into(),
            params: json!({"name": "changed", "arguments": {}}),
        };
        let response = handle_request(request, "false", &PathBuf::from(".")).unwrap();
        let result = response.result.unwrap();
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("ast-index exited with"));
    }

    #[test]
    fn deps_dependents_module_required() {
        for tool in &["deps", "dependents"] {
            let err = build_argv(tool, &json!({})).unwrap_err();
            assert!(err.to_string().contains("'module'"), "{tool}: {err}");
        }
    }

    #[test]
    fn unknown_tool_errors() {
        let err = build_argv("foo_bar_does_not_exist", &json!({})).unwrap_err();
        assert!(err.to_string().contains("unknown tool"), "got: {err}");
    }

    // --- supports_json_format ---

    #[test]
    fn supports_json_format_correct_set() {
        let yes = [
            "search",
            "usages",
            "callers",
            "implementations",
            "refs",
            "stats",
            "symbol",
            "class",
            "changed",
            "hotspots",
            "graph_dependents",
            "graph_dependencies",
            "graph_path",
            "graph_metrics",
            "graph_cycles",
            "graph_build",
        ];
        let no = [
            "outline",
            "rebuild",
            "find_file",
            "update",
            "hierarchy",
            "imports",
            "api",
            "module",
            "deps",
            "dependents",
            "call_tree",
        ];
        for t in yes {
            assert!(supports_json_format(t), "{t} should support --format json");
        }
        for t in no {
            assert!(
                !supports_json_format(t),
                "{t} should NOT support --format json"
            );
        }
    }

    #[test]
    fn argv_appends_format_json_only_when_supported() {
        // search: supported → has --format json
        let a = build_argv("search", &json!({"query": "x"})).unwrap();
        assert!(a.contains(&"--format".to_string()) && a.contains(&"json".to_string()));

        // outline: not supported → no --format
        let b = build_argv("outline", &json!({"file": "x"})).unwrap();
        assert!(!b.contains(&"--format".to_string()));
    }
}
