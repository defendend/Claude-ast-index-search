//! End-to-end tests for the local usage log: every CLI invocation is
//! recorded by the hook in `main`, and `ast-index usage` reports it.

use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

struct Sandbox {
    project: TempDir,
    cache: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let project = TempDir::new().unwrap();
        std::fs::write(project.path().join("lib.rs"), "pub fn greet() {}\n").unwrap();
        Sandbox {
            project,
            cache: TempDir::new().unwrap(),
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ast-index"));
        cmd.current_dir(self.project.path())
            .args(args)
            .env("AST_INDEX_CACHE_DIR", self.cache.path())
            .env_remove("AST_INDEX_DB_PATH")
            .env_remove("AST_INDEX_NO_USAGE")
            .env_remove("AST_INDEX_CALLER")
            .env_remove("AST_INDEX_USAGE_ID")
            .env_remove("AST_INDEX_MCP_TOOL")
            .env_remove("CLAUDECODE");
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        let out = self.command(args).output().unwrap();
        assert!(
            out.status.success(),
            "ast-index {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn report(&self) -> Value {
        let out = self.run(&["--format", "json", "usage", "--since", "all"]);
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn usage_db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.cache.path().join("usage.db")).unwrap()
    }
}

fn command_stats<'a>(report: &'a Value, command: &str) -> &'a Value {
    report["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["command"] == command)
        .unwrap_or_else(|| panic!("no '{command}' row in {report:#}"))
}

#[test]
fn every_subcommand_is_recorded_with_measured_stdout() {
    let sb = Sandbox::new();
    sb.run(&["rebuild"]);
    let search = sb.run(&["search", "greet"]);
    sb.run(&["search", "NoSuchSymbolZZ"]);

    let report = sb.report();
    let search_stats = command_stats(&report, "search");
    assert_eq!(search_stats["calls"], 2);
    assert_eq!(search_stats["no_results"], 1);
    assert_eq!(search_stats["by_source"]["script"], 2);
    assert_eq!(command_stats(&report, "rebuild")["calls"], 1);
    assert_eq!(report["windows"]["all"], 3);

    // The tee must forward output untouched and count exactly what it forwarded.
    let conn = sb.usage_db();
    let bytes: i64 = conn
        .query_row(
            "SELECT stdout_bytes FROM invocations WHERE args = '[\"search\",\"greet\"]'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bytes as usize, search.stdout.len());
    assert!(String::from_utf8_lossy(&search.stdout).contains("greet"));
}

#[test]
fn nested_subcommands_are_named_by_their_chain() {
    let sb = Sandbox::new();
    sb.run(&["rebuild"]);
    sb.run(&["subtree", "list"]);

    let conn = sb.usage_db();
    let commands: Vec<String> = conn
        .prepare("SELECT command FROM invocations ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(commands, vec!["rebuild", "subtree list"]);
}

#[test]
fn mcp_calls_are_tagged_and_take_the_compacted_response_size() {
    let sb = Sandbox::new();
    sb.run(&["rebuild"]);
    sb.command(&["search", "greet"])
        .env("AST_INDEX_CALLER", "mcp")
        .env("AST_INDEX_MCP_TOOL", "search")
        .env("AST_INDEX_USAGE_ID", "call-1")
        .output()
        .unwrap();
    sb.run(&["usage", "--record-response", "call-1", "--response-bytes", "42"]);

    let report = sb.report();
    let search = command_stats(&report, "search");
    assert_eq!(search["by_source"]["mcp"], 1);
    assert_eq!(search["mcp_response_bytes"], 42);
    // Neither the report nor the response hook shows up as a call.
    assert!(report["commands"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["command"] != "usage"));
}

#[test]
fn claude_code_shell_calls_are_attributed() {
    let sb = Sandbox::new();
    sb.command(&["version"]).env("CLAUDECODE", "1").output().unwrap();

    let report = sb.report();
    assert_eq!(command_stats(&report, "version")["by_source"]["claude-code"], 1);
}

#[test]
fn opt_out_env_disables_recording() {
    let sb = Sandbox::new();
    sb.command(&["version"])
        .env("AST_INDEX_NO_USAGE", "1")
        .output()
        .unwrap();

    assert_eq!(sb.report()["windows"]["all"], 0);
}

#[test]
fn rejected_arguments_and_help_are_not_recorded() {
    let sb = Sandbox::new();
    sb.command(&["--help"]).output().unwrap();
    sb.command(&["search"]).output().unwrap(); // missing required query

    assert_eq!(sb.report()["windows"]["all"], 0);
}

#[test]
fn failed_commands_count_as_errors() {
    let sb = Sandbox::new();
    let out = sb.command(&["usage", "--since", "bogus"]).output().unwrap();
    assert!(!out.status.success());
    // `usage` itself is never recorded; use a recorded command that fails.
    let out = sb.command(&["restore", "/nonexistent/index.db"]).output().unwrap();
    assert!(!out.status.success());

    let report = sb.report();
    assert_eq!(command_stats(&report, "restore")["errors"], 1);
}

#[test]
fn report_is_scoped_to_the_current_project() {
    let sb = Sandbox::new();
    let other = TempDir::new().unwrap();
    sb.run(&["version"]);
    Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(other.path())
        .arg("version")
        .env("AST_INDEX_CACHE_DIR", sb.cache.path())
        .env_remove("AST_INDEX_NO_USAGE")
        .output()
        .unwrap();

    assert_eq!(sb.report()["windows"]["all"], 1);

    let all = sb.run(&["--format", "json", "usage", "--since", "all", "--all-projects"]);
    let all: Value = serde_json::from_slice(&all.stdout).unwrap();
    assert_eq!(all["windows"]["all"], 2);
    assert_eq!(all["projects"].as_array().unwrap().len(), 2);
}

#[test]
fn text_report_lists_windows_and_commands() {
    let sb = Sandbox::new();
    sb.run(&["version"]);
    let out = sb.run(&["usage"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("1 last 7 days, 1 last 30 days, 1 all time"), "{text}");
    assert!(text.contains("version"), "{text}");
}
