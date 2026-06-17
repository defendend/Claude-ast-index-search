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
    let mut stdout = io::stdout().lock();
    let mut line = String::new();

    for raw in stdin.lock().lines() {
        line.clear();
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

        let response = handle_request(request, &ast_index_bin, &default_root);

        // Notifications (no `id`) produce no response. Everything else gets one.
        if let Some(response) = response {
            let json = serde_json::to_string(&response)?;
            writeln!(stdout, "{json}")?;
            stdout.flush()?;
        }
    }

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
                "instructions": "Prefer these ast-index tools over grep/ripgrep and over reading whole files for any code or symbol search in this project. They query a precomputed index: precise (never match inside comments or strings), language-aware, and far cheaper in tokens and round-trips. Rules of thumb: `explore` FIRST to understand an area or answer 'how does X work' (one call returns ranked source + callers/subclasses + tests); `search` for broad discovery; `usages`/`refs`/`callers` for a named symbol; `outline` before reading a file over ~500 lines. Reach for raw grep/Read only for plain text, regex, or non-code files, or to confirm a detail these tools did not cover."
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
            "name": "search",
            "description": "Universal code search across file paths, symbol definitions, imports/usages, and file contents. Use this FIRST for any 'find X in the codebase' question — it returns files, matching symbols (classes, functions, etc.), and content matches in one call. Prefer this over grep.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query":        { "type": "string", "description": "Search query. Comma-separated for OR: 'email,mail'." },
                    "limit":        { "type": "integer", "description": "Max results per category (default 50)." },
                    "kind":         { "type": "string",  "description": "Filter symbols by kind: class, interface, function, method, struct, enum, etc." },
                    "in_file":      { "type": "string",  "description": "Restrict to files whose path contains this substring." },
                    "module":       { "type": "string",  "description": "Restrict to files whose path starts with this prefix." },
                    "fuzzy":        { "type": "boolean", "description": "Enable typo-tolerant fuzzy matching." },
                    "project_root": { "type": "string",  "description": "Absolute path to project root. Optional if the server was started with --root or AST_INDEX_ROOT." },
                    "format":       { "type": "string",  "enum": ["text", "json"], "description": "Output format. Default 'text' (compact, token-efficient). Pass 'json' only if you need structured parsing — costs ~2-3× more tokens." }
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
            "description": "Find every usage (call site, import, downcast, DI registration) of a symbol anywhere in the indexed codebase. Use this when the question is 'who uses X' / 'where is X called from'. Returns file:line + surrounding context. Prefer this over grepping the symbol name — it resolves real references and won't match inside comments or strings.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "symbol":       { "type": "string",  "description": "Symbol name (class, function, method)." },
                    "limit":        { "type": "integer", "description": "Max results (default 50)." },
                    "in_file":      { "type": "string",  "description": "Restrict to files whose path contains this substring." },
                    "module":       { "type": "string",  "description": "Restrict to files whose path starts with this prefix." },
                    "project_root": { "type": "string",  "description": "Absolute path to project root. Optional." },
                    "format":       { "type": "string",  "enum": ["text", "json"], "description": "Output format. Default 'text' (compact, token-efficient). Pass 'json' only if you need structured parsing — costs ~2-3× more tokens." }
                },
                "required": ["symbol"]
            }
        }),
        json!({
            "name": "callers",
            "description": "Find every function that calls the given function, one level up. Use for 'who calls processPayment' questions. For the full transitive caller tree, call this repeatedly or use `search` with deeper queries. Prefer this over grep for call sites — it attributes calls to the calling function, not just raw text matches.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "function":     { "type": "string",  "description": "Function or method name." },
                    "limit":        { "type": "integer", "description": "Max results (default 50)." },
                    "project_root": { "type": "string",  "description": "Absolute path to project root. Optional." },
                    "format":       { "type": "string",  "enum": ["text", "json"], "description": "Output format. Default 'text' (compact, token-efficient). Pass 'json' only if you need structured parsing — costs ~2-3× more tokens." }
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
                    "format":       { "type": "string",  "enum": ["text", "json"], "description": "Output format. Default 'text' (compact, token-efficient). Pass 'json' only if you need structured parsing — costs ~2-3× more tokens." }
                },
                "required": ["parent"]
            }
        }),
        json!({
            "name": "refs",
            "description": "Show cross-references for a symbol in one shot: every definition, every import, every usage. Use this when you want the complete picture in a single response, rather than calling `usages` / `callers` separately. Prefer this over grep for a symbol — precise, deduplicated, and grouped by kind.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "symbol":       { "type": "string",  "description": "Symbol name." },
                    "limit":        { "type": "integer", "description": "Max results per category (default 50)." },
                    "project_root": { "type": "string",  "description": "Absolute path to project root. Optional." },
                    "format":       { "type": "string",  "enum": ["text", "json"], "description": "Output format. Default 'text' (compact, token-efficient). Pass 'json' only if you need structured parsing — costs ~2-3× more tokens." }
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
            "description": "List symbols that changed since a base git/arc branch — additions, modifications, deletions. Essential for code-review prep, changelog generation, 'what did I actually change' questions. Default base is `origin/main` for git repos, `trunk` for arc repos.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "base":         { "type": "string", "description": "Base branch name. Defaults to 'origin/main' (git) or 'trunk' (arc)." },
                    "project_root": { "type": "string", "description": "Absolute path to project root. Optional." }
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

    let rendered = match output_format {
        "json" => stdout, // caller asked for raw JSON — pass through
        _ => format::to_compact(name, &stdout),
    };
    Ok(rendered)
}

/// Whether the underlying ast-index command honours `--format json`.
/// Commands not in this set print plain text regardless of the flag, so
/// we avoid passing it to keep the argv honest.
fn supports_json_format(tool: &str) -> bool {
    matches!(
        tool,
        "search" | "usages" | "implementations" | "refs" | "stats" | "symbol" | "class"
    )
}

/// Translate an MCP `tools/call` invocation into the equivalent
/// `ast-index <subcommand> <args> [--format json]` argv. Pure function —
/// no I/O, suitable for unit testing.
pub fn build_argv(name: &str, arguments: &Value) -> Result<Vec<String>> {
    let mut argv: Vec<String> = Vec::new();
    match name {
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
            push_if_str(&mut argv, &arguments, "base", "--base");
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // --- tool_descriptors metadata ---

    #[test]
    fn descriptors_expose_exactly_twenty_tools() {
        let names: Vec<String> = tool_descriptors()
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string))
            .collect();
        assert_eq!(names.len(), 20, "MCP must expose 20 tools, got {names:?}");
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
            "pattern": "*", "module": "m"
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
        assert_eq!(argv, vec!["changed"]);
    }

    #[test]
    fn changed_with_base() {
        let argv = build_argv("changed", &json!({"base": "develop"})).unwrap();
        assert_eq!(argv, vec!["changed", "--base", "develop"]);
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
            "implementations",
            "refs",
            "stats",
            "symbol",
            "class",
        ];
        let no = [
            "outline",
            "callers",
            "rebuild",
            "find_file",
            "update",
            "hierarchy",
            "imports",
            "api",
            "changed",
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
