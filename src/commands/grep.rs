//! Grep-based search commands
//!
//! General pattern-based search commands:
//! - todo: Find TODO/FIXME/HACK comments
//! - callers: Find function callers
//! - provides: Find Dagger @Provides/@Binds for a type
//! - suspend: Find suspend functions
//! - composables: Find @Composable functions
//! - deprecated: Find @Deprecated annotations
//! - suppress: Find @Suppress annotations
//! - inject: Find @Inject points for a type
//! - annotations: Find uses of specific annotation
//! - deeplinks: Find deeplink definitions
//! - extensions: Find extension functions/types
//! - flows: Find Flow declarations
//! - previews: Find @Preview functions

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::Result;
use colored::Colorize;
use regex::Regex;

use super::{print_truncation_notice, relative_path, search_files_limited, PathResolver};
use crate::db;

/// All source code extensions (for grep-based commands: todo, search, callers, etc.)
pub const ALL_SOURCE_EXTENSIONS: [&str; 58] = [
    "kt", "java", "swift", "m", "h",    // Mobile
    "dart", // Flutter
    "gd",   // GDScript (Godot)
    "pm", "pl", "t", "rb", // Scripting
    "ts", "tsx", "mts", "js", "jsx", "mjs", "cjs", // JavaScript/TypeScript
    "vue", "svelte", // Web frameworks
    "css", "pcss", "postcss", "scss", "less", // CSS family
    "py",   // Python
    "go",   // Go
    "rs",   // Rust
    "zig",  // Zig
    "cs",   // C#
    "cpp", "cc", "c", "hpp", // C++
    "scala", "sc", // Scala
    "php", "phtml", // PHP
    "groovy", "gradle", // Groovy
    "lua",    // Lua
    "ex", "exs", // Elixir
    "sh", "bash", "zsh", // Shell
    "sql", // SQL
    "r", "R", // R
    "bsl", "os", // BSL
    "lisp", "lsp", "cl", "asd", // Common Lisp
    "proto", "wsdl", "xsd", // Schema
];

/// Combine a search pattern with a literal, case-insensitive line filter.
///
/// `search_files_limited` counts `limit` against raw pattern matches, so any
/// filtering done afterwards in the handler silently under-reports. Folding
/// the filter into the pattern keeps `--limit` honest.
fn pattern_with_line_filter(pattern: &str, filter: Option<&str>) -> String {
    match filter.filter(|f| !f.is_empty()) {
        Some(f) => {
            let f = regex::escape(f);
            format!("(?:{pattern}).*(?i:{f})|(?i:{f}).*(?:{pattern})")
        }
        None => pattern.to_string(),
    }
}

/// Trailing word boundary: `\b` for normal names, empty for Ruby bang/question methods
fn trailing_boundary(function_name: &str) -> &str {
    if function_name.ends_with('!') || function_name.ends_with('?') {
        "" // ! and ? are non-word chars — natural boundary, \b would fail here
    } else {
        r"\b"
    }
}

/// Call idioms recognised across languages, joined into one alternation.
/// `{fn}` stands for the escaped function name and `{tb}` for its trailing
/// boundary. Every idiom contains `{fn}`, so a line that matches always
/// contains the name verbatim; the batched call-tree scan relies on that.
const CALLER_IDIOMS: [&str; 13] = [
    r"[.>]{fn}\s*\(",                // obj.func( or obj->func(
    r"\b{fn}\s*\(",                  // bare func( anywhere in line
    r"->{fn}\s*\(",                  // ->func(
    r"&{fn}\s*\(",                   // &func(
    r"this\.{fn}\s*\(",              // this.func(
    r"super\.{fn}\s*\(",             // super.func(
    r"\.{fn}(?:\s|$)",               // Ruby: obj.method (no parens)
    r":{fn}{tb}",                    // Ruby: :method_name (symbol ref in callbacks)
    r"\b{fn}\.",                     // Ruby: bare method.chain (e.g. scope.where)
    r"\bawait\s+{fn}\s*\(",          // TS: await func(
    r"\bawait\s+[\w.]+\.{fn}\s*\(",  // TS: await obj.func(
    r"\breturn\s+{fn}\s*\(",         // TS: return func(
    r"\breturn\s+[\w.]+\.{fn}\s*\(", // TS: return obj.func(
];

/// The call idiom of a Ruby predicate or bang method alone: the bare name,
/// with neither receiver nor parentheses (`if next_page? && …`, `save!`). No
/// local variable can end in `?` or `!`, so the word itself is a call. Not
/// after `#`, which marks a method in documentation (`describe "#valid?"`),
/// and not before `=` or `~`: `save!=` and `save!~` apply `!=` and `!~` to
/// `save`.
const BARE_PREDICATE_CALL_IDIOM: &str = r"(?:^|[^#\w]){fn}(?:[^=~]|$)";

/// Whether [`BARE_PREDICATE_CALL_IDIOM`] applies: a plain identifier ending in
/// `?` or `!`.
fn is_predicate_or_bang_name(function_name: &str) -> bool {
    function_name
        .strip_suffix(&['?', '!'][..])
        .is_some_and(|body| {
            !body.is_empty() && body.chars().all(|c| c.is_alphanumeric() || c == '_')
        })
}

fn caller_pattern(fn_pattern: &str, trailing: &str) -> String {
    CALLER_IDIOMS
        .iter()
        .map(|idiom| idiom.replace("{fn}", fn_pattern).replace("{tb}", trailing))
        .collect::<Vec<_>>()
        .join("|")
}

/// Build regex pattern that matches function/method calls across languages
fn build_caller_pattern(function_name: &str) -> String {
    let escaped = regex::escape(function_name);
    let mut pattern = caller_pattern(&escaped, trailing_boundary(function_name));
    if is_predicate_or_bang_name(function_name) {
        pattern.push('|');
        pattern.push_str(&BARE_PREDICATE_CALL_IDIOM.replace("{fn}", &escaped));
    }
    pattern
}

/// A pattern matching every line that [`build_caller_pattern`] matches for
/// at least one of `function_names`. Dropping the trailing boundary only
/// widens the `:symbol` idiom, and the bare name at a word boundary widens
/// the bare predicate idiom, so the superset holds for every name.
fn build_any_caller_pattern(function_names: &[String]) -> String {
    let names: Vec<String> = function_names
        .iter()
        .map(|name| regex::escape(name))
        .collect();
    let mut pattern = caller_pattern(&format!("(?:{})", names.join("|")), "");
    let predicates: Vec<String> = function_names
        .iter()
        .filter(|name| is_predicate_or_bang_name(name))
        .map(|name| regex::escape(name))
        .collect();
    if !predicates.is_empty() {
        pattern.push_str(&format!(r"|\b(?:{})", predicates.join("|")));
    }
    pattern
}

/// Words the Java-style branch of [`build_def_skip_pattern`] would otherwise
/// read as a return type. Standing right before a name they make the line a
/// call, never a definition: `return foo(`, `await foo(`, `new Foo(`,
/// `if foo(`, `for x in foo(`, `export default foo(`, `go foo(`, `puts foo(`.
const KEYWORDS_BEFORE_CALL: [&str; 33] = [
    "and", "assert", "await", "case", "default", "defer", "echo", "elif", "else", "elsif", "from",
    "go", "if", "in", "match", "new", "not", "of", "or", "print", "puts", "raise", "range",
    "return", "then", "throw", "try", "unless", "until", "when", "while", "with", "yield",
];

/// Lines that define one particular function, as opposed to calling it.
struct DefinitionPattern(Regex);

impl DefinitionPattern {
    fn is_match(&self, line: &str) -> bool {
        // `regex` has no lookaround, so the word in return-type position is
        // captured and a keyword there is ruled out here instead.
        self.0.captures_iter(line).any(|caps| {
            caps.name("type")
                .map_or(true, |word| !KEYWORDS_BEFORE_CALL.contains(&word.as_str()))
        })
    }
}

/// Build regex pattern that skips function/method definitions
fn build_def_skip_pattern(function_name: &str) -> DefinitionPattern {
    let fn_escaped = regex::escape(function_name);
    let tb = trailing_boundary(function_name);
    DefinitionPattern(
        Regex::new(&format!(
            concat!(
                r"\b(?:fun|func|sub)\s+{fn}\s*[<({{\[]",           // Kotlin/Swift/Perl
                r"|\bdef\s+(?:self\.)?{fn}{tb}",                    // Ruby: def method / def self.method
                r"|\b(?:(?:public|private|protected|static|final|abstract|synchronized|override)\s+)*",
                r"(?:void|int|long|boolean|char|byte|short|float|double|(?P<type>[\w.]+)(?:<[^{{;]*>)?(?:\[\])*)\s+{fn}\s*\(", // Java
            ),
            fn = fn_escaped,
            tb = tb
        ))
        .expect("Invalid def skip pattern"),
    )
}

/// Find TODO/FIXME/HACK comments
pub fn cmd_todo(root: &Path, pattern: &str, limit: usize) -> Result<()> {
    let search_pattern = format!(r"//.*({pattern})|#.*({pattern})");

    let mut todos: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
    todos.insert("TODO".to_string(), vec![]);
    todos.insert("FIXME".to_string(), vec![]);
    todos.insert("HACK".to_string(), vec![]);
    todos.insert("OTHER".to_string(), vec![]);

    let mut count = 0;

    search_files_limited(
        root,
        &search_pattern,
        &ALL_SOURCE_EXTENSIONS,
        limit,
        |path, line_num, line| {
            let rel_path = relative_path(root, path);
            let content: String = line.chars().take(80).collect();
            let upper = content.to_uppercase();

            let category = if upper.contains("TODO") {
                "TODO"
            } else if upper.contains("FIXME") {
                "FIXME"
            } else if upper.contains("HACK") {
                "HACK"
            } else {
                "OTHER"
            };

            todos
                .get_mut(category)
                .unwrap()
                .push((rel_path, line_num, content));
            count += 1;
        },
    )?;

    let total: usize = todos.values().map(|v| v.len()).sum();
    println!("{}", format!("Found {} comments:", total).bold());

    for (category, items) in &todos {
        if !items.is_empty() {
            println!("\n{}", format!("{} ({}):", category, items.len()).cyan());
            for (path, line_num, content) in items.iter().take(20) {
                println!("  {}:{}", path, line_num);
                println!("    {}", content);
            }
            if items.len() > 20 {
                println!("  ... and {} more", items.len() - 20);
            }
        }
    }

    Ok(())
}

/// Find function callers
pub fn cmd_callers(
    root: &Path,
    function_name: &str,
    limit: usize,
    format: &str,
    in_file: Option<&str>,
) -> Result<()> {
    let pattern = build_caller_pattern(function_name);
    let def_pattern = build_def_skip_pattern(function_name);
    let conn = db::open_db_leased(root)?;
    let resolver = PathResolver::try_from_conn(root, &conn)?;
    let roots = resolver.grep_roots();

    let page = super::search_files_page_in(
        root,
        &roots,
        &pattern,
        &ALL_SOURCE_EXTENSIONS,
        limit,
        |path, line_num, line| {
            if def_pattern.is_match(line) {
                return None;
            } // Skip definitions

            let rel_path = super::display_path(&resolver, root, path);
            if let Some(filter) = in_file {
                if !rel_path.contains(filter) {
                    return None;
                }
            }
            let content: String = line.chars().take(70).collect();
            Some((rel_path, line_num, content))
        },
    )?;

    if format == "json" {
        let items: Vec<_> = page
            .items
            .iter()
            .map(|(path, line, content)| {
                serde_json::json!({"path": path, "line": line, "content": content})
            })
            .collect();
        let result = serde_json::json!({
            "schema_version": super::PAGINATED_JSON_SCHEMA_VERSION,
            "items": items,
            "pagination": page.pagination,
        });
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    println!(
        "{}",
        format!(
            "Callers of '{}' (showing {} of {}):",
            function_name, page.pagination.returned, page.pagination.total
        )
        .bold()
    );

    let mut by_file: HashMap<&str, Vec<(usize, &str)>> = HashMap::new();
    for (path, line, content) in &page.items {
        by_file
            .entry(path)
            .or_default()
            .push((*line, content.as_str()));
    }
    let mut paths: Vec<_> = by_file.into_iter().collect();
    paths.sort_by(|left, right| left.0.cmp(right.0));
    for (path, items) in paths {
        println!("\n  {}:", path.cyan());
        for (line_num, content) in items {
            println!("    :{} {}", line_num, content);
        }
    }
    print_truncation_notice(page.pagination);

    Ok(())
}

/// Show call hierarchy (callers tree) for a function
pub fn cmd_call_tree(
    root: &Path,
    function_name: &str,
    max_depth: usize,
    limit_per_level: usize,
    in_file: Option<&str>,
) -> Result<()> {
    println!("{}", format!("Call tree for '{}':", function_name).bold());
    println!("  {}", function_name.cyan());

    // A missing or unreadable index is not fatal here: attribution falls back
    // to the textual scan that predates the index.
    let conn = db::open_db_leased(root).ok();

    let callers = collect_tree_callers(
        root,
        conn.as_deref(),
        function_name,
        max_depth,
        limit_per_level,
        in_file,
    )?;
    walk_call_tree(
        function_name,
        max_depth,
        &callers,
        &mut |depth, caller, site| {
            let indent = "  ".repeat(depth + 1);
            match site {
                Some((file_path, line_num)) => println!(
                    "{}← {} ({}:{})",
                    indent,
                    caller.yellow(),
                    file_path,
                    line_num
                ),
                None => println!("{}← {} (recursive)", indent, caller.dimmed()),
            }
        },
    );

    Ok(())
}

/// Calling functions of one function: `(caller, file, line of the caller)`.
type CallerSites = Vec<(String, String, usize)>;

/// Callers of every function the printed tree expands.
///
/// Each function takes a scan of the whole repository to find its callers,
/// and one scan per function made the cost grow with the tree's width. So the
/// depth-first walk the tree is printed in is replayed against what is known
/// so far, every function it still lacks is looked up in a single shared scan,
/// and the replay repeats until nothing is missing — about one scan per level.
/// A shared scan gives each function the lines a scan of its own would, so the
/// tree comes out the same. The repository is walked once, on the first scan,
/// and every scan goes over the same list of files.
fn collect_tree_callers(
    root: &Path,
    conn: Option<&rusqlite::Connection>,
    function_name: &str,
    max_depth: usize,
    limit: usize,
    in_file: Option<&str>,
) -> Result<HashMap<String, CallerSites>> {
    let mut callers = HashMap::new();
    if limit == 0 {
        return Ok(callers);
    }
    let mut files: Option<Vec<PathBuf>> = None;
    loop {
        let missing = walk_call_tree(function_name, max_depth, &callers, &mut |_, _, _| {});
        if missing.is_empty() {
            return Ok(callers);
        }
        let files = match files {
            Some(ref files) => files,
            None => files.insert(super::project_source_files(root, &ALL_SOURCE_EXTENSIONS)?),
        };
        let found = find_caller_functions(root, conn, files, &missing, limit, in_file)?;
        callers.extend(missing.into_iter().zip(found));
    }
}

/// Visit the call tree depth-first, in print order, handing every edge to
/// `visit` as `(depth, caller, Some((file, line)))`, or `None` for a caller
/// already shown. A caller whose name no code can call is shown but not
/// expanded; see [`is_callable_name`].
///
/// Returns the functions whose callers the walk needed but `callers` lacks;
/// their subtrees are skipped, so a walk with anything missing is only a
/// draft of the final one.
fn walk_call_tree(
    function_name: &str,
    max_depth: usize,
    callers: &HashMap<String, CallerSites>,
    visit: &mut dyn FnMut(usize, &str, Option<(&str, usize)>),
) -> Vec<String> {
    let mut visited = std::collections::HashSet::new();
    visited.insert(function_name.to_string());
    let mut missing = Vec::new();
    walk_callers_of(
        function_name,
        1,
        max_depth,
        callers,
        &mut visited,
        &mut missing,
        visit,
    );
    missing
}

fn walk_callers_of(
    function_name: &str,
    depth: usize,
    max_depth: usize,
    callers: &HashMap<String, CallerSites>,
    visited: &mut std::collections::HashSet<String>,
    missing: &mut Vec<String>,
    visit: &mut dyn FnMut(usize, &str, Option<(&str, usize)>),
) {
    if depth > max_depth {
        return;
    }
    let Some(sites) = callers.get(function_name) else {
        if !missing.iter().any(|name| name == function_name) {
            missing.push(function_name.to_string());
        }
        return;
    };
    for (caller, file_path, line_num) in sites {
        if visited.insert(caller.clone()) {
            visit(depth, caller, Some((file_path, *line_num)));
            if is_callable_name(caller) {
                walk_callers_of(
                    caller,
                    depth + 1,
                    max_depth,
                    callers,
                    visited,
                    missing,
                    visit,
                );
            }
        } else {
            visit(depth, caller, None);
        }
    }
}

/// Whether source code can call `name`, i.e. whether it is an identifier:
/// word characters and the `$`, `#`, `-` some languages allow in names,
/// joined by `::` or `.` (`Applicant::MergeService`, `self.call`), with an
/// optional trailing `!`, `?` or `=` (Ruby `save!`, `valid?`, `name=`).
///
/// The index also names blocks that nothing calls by name — `it "works"`,
/// `let(:user)`, `describe "Foo"`, `attributes :id`. Such a block does own
/// the call lines inside it, so it is a caller worth showing, but looking
/// for calls to it would cost a scan of the whole repository and find none.
fn is_callable_name(name: &str) -> bool {
    let body = name.strip_suffix(&['!', '?', '='][..]).unwrap_or(name);
    let mut has_word = false;
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            ':' => {
                if chars.next() != Some(':') {
                    return false;
                }
            }
            '.' | '$' | '#' | '-' => {}
            c if c.is_alphanumeric() || c == '_' => has_word = true,
            _ => return false,
        }
    }
    has_word
}

/// Find the functions that call each of `function_names`, in one scan of
/// `files`.
///
/// Call sites are still located textually: the regex knows call idioms the
/// `refs` table does not record (`obj.method` without parentheses, Ruby
/// `:symbol` callbacks, `await obj.fn(`), so replacing it would cost recall.
/// Only the "which function is this line inside" step consults the index,
/// which knows real symbol ranges instead of guessing from the nearest
/// definition line above.
///
/// Each function gets the first `limit * 3` call lines in path order. A
/// definition line or a file outside `in_file` does not count against that:
/// in path order definitions cluster (every worker in `app/workers` defines
/// `perform`) and would leave no budget for the calls.
fn find_caller_functions(
    root: &Path,
    conn: Option<&rusqlite::Connection>,
    files: &[PathBuf],
    function_names: &[String],
    limit: usize,
    in_file: Option<&str>,
) -> Result<Vec<CallerSites>> {
    let patterns: Vec<(String, String)> = function_names
        .iter()
        .map(|name| (build_caller_pattern(name), name.clone()))
        .collect();
    let def_patterns: Vec<DefinitionPattern> = function_names
        .iter()
        .map(|name| build_def_skip_pattern(name))
        .collect();

    // Pattern to find function definitions (for locating the containing function)
    // Group 1: fun/func/function/def/sub style, Group 2: Ruby def/def self., Group 3: Java return-type style, Group 4: TS arrow function
    let func_def_re = Regex::new(concat!(
        r"(?:fun|function|func|sub)\s+(\w+)\s*[<(\[]",
        r"|\bdef\s+(?:self\.)?(\w[!\w?]*)",
        r"|(?:(?:public|private|protected|static|final|abstract|synchronized|override|export|async)\s+)*",
        r"(?:void|int|long|boolean|char|byte|short|float|double|[\w.]+(?:<[^{;]*>)?(?:\[\])*)\s+(\w+)\s*\(",
        r"|(?:const|let)\s+(\w+)\s*=\s*(?:async\s+)?(?:\([^)]*\)|[a-zA-Z_]\w*)\s*(?::\s*[^=]+)?\s*=>",
    ))?;

    let mut files_with_calls: Vec<BTreeMap<PathBuf, Vec<usize>>> =
        function_names.iter().map(|_| BTreeMap::new()).collect();

    // First pass: find all files and line numbers with calls
    super::search_files_limited_each(
        files,
        &build_any_caller_pattern(function_names),
        &patterns,
        limit * 3,
        |index, path, line| {
            !def_patterns[index].is_match(line)
                && in_file.map_or(true, |filter| relative_path(root, path).contains(filter))
        },
        |index, path, line_num, _line| {
            files_with_calls[index]
                .entry(path.to_path_buf())
                .or_default()
                .push(line_num);
        },
    )?;

    let root_key = db::normalize_root_for_storage(root);
    Ok(files_with_calls
        .into_iter()
        .map(|files| attribute_call_lines(root, &root_key, conn, files, limit, &func_def_re))
        .collect())
}

/// Second pass of [`find_caller_functions`]: the function containing each
/// call line, at most `limit` distinct ones, the first in path order.
///
/// Every file lies under the primary root `root`, stored as `root_key`.
fn attribute_call_lines(
    root: &Path,
    root_key: &str,
    conn: Option<&rusqlite::Connection>,
    files_with_calls: BTreeMap<PathBuf, Vec<usize>>,
    limit: usize,
    func_def_re: &Regex,
) -> CallerSites {
    let mut results: CallerSites = vec![];

    for (file_path, call_lines) in files_with_calls {
        if results.len() >= limit {
            break;
        }

        let rel_path = relative_path(root, &file_path);
        // When the index has ranges for this file, "no owner" is an answer,
        // not a gap: the call sits at module level and has no calling
        // function. Only a file the index cannot speak for gets the scan,
        // and only such a file has to be read off disk at all.
        let ranges_known = conn
            .map(|conn| {
                db::file_has_symbol_ranges(conn, Some(root_key), &rel_path).unwrap_or(false)
            })
            .unwrap_or(false);
        let content = if ranges_known {
            String::new()
        } else {
            match std::fs::read_to_string(&file_path) {
                Ok(content) => content,
                Err(_) => continue,
            }
        };
        let lines: Vec<&str> = content.lines().collect();

        for call_line in call_lines {
            if results.len() >= limit {
                break;
            }

            let owner = conn
                .and_then(|conn| {
                    db::find_owning_symbol(conn, Some(root_key), &rel_path, call_line as i64)
                        .unwrap_or(None)
                })
                .map(|symbol| (symbol.name, symbol.line as usize));
            let owner = match owner {
                Some(owner) => Some(owner),
                None if ranges_known => None,
                None => find_containing_function(&lines, call_line, func_def_re),
            };

            if let Some((func_name, func_line)) = owner {
                // Avoid adding the same function twice for this target
                if !results
                    .iter()
                    .any(|(f, p, _)| f == &func_name && p == &rel_path)
                {
                    results.push((func_name, rel_path.clone(), func_line));
                }
            }
        }
    }

    results
}

/// Find the function that contains a given line number
fn find_containing_function(
    lines: &[&str],
    target_line: usize,
    func_def_re: &Regex,
) -> Option<(String, usize)> {
    // Search backwards from the target line to find a function definition
    let start_idx = (target_line.saturating_sub(1)).min(lines.len().saturating_sub(1));

    for i in (0..=start_idx).rev() {
        let line = lines[i];
        if let Some(caps) = func_def_re.captures(line) {
            // Group 1: fun/function/func/sub, Group 2: Ruby def, Group 3: Java return-type, Group 4: TS arrow
            if let Some(name) = caps
                .get(1)
                .or_else(|| caps.get(2))
                .or_else(|| caps.get(3))
                .or_else(|| caps.get(4))
            {
                return Some((name.as_str().to_string(), i + 1));
            }
        }
    }

    None
}

/// Find Dagger @Provides/@Binds for a type
pub fn cmd_provides(root: &Path, type_name: &str, limit: usize) -> Result<()> {
    let mut results: Vec<(String, usize, String)> = vec![];

    // Parallel grep narrows the scan to files that declare providers at all;
    // reading every .kt/.java file sequentially is prohibitively slow on large
    // or network-backed checkouts.
    let mut candidate_files: std::collections::BTreeSet<PathBuf> = Default::default();
    search_files_limited(
        root,
        r"@Provides|@Binds",
        &["kt", "java"],
        100_000,
        |path, _line_num, _line| {
            candidate_files.insert(path.to_path_buf());
        },
    )?;

    let kotlin_re = Regex::new(&format!(r":\s*\w*{}\b", regex::escape(type_name)))?;
    let java_re = Regex::new(&format!(r"\b\w*{}\s+\w+\s*\(", regex::escape(type_name)))?;

    for path in &candidate_files {
        if results.len() >= limit {
            break;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        if !content.contains(type_name) {
            continue;
        }
        let lines: Vec<&str> = content.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if results.len() >= limit {
                break;
            }
            if !(line.contains("@Provides") || line.contains("@Binds")) {
                continue;
            }
            // Look at this line and the next few for the return type.
            // Kotlin: `: ReturnType`; Java: `ReturnType methodName(`.
            // A prefix is allowed so AppIconInteractor matches Interactor.
            let context: String = lines[i..std::cmp::min(i + 5, lines.len())].join(" ");
            if !(kotlin_re.is_match(&context) || java_re.is_match(&context)) {
                continue;
            }
            let rel_path = relative_path(root, path);
            // Kotlin: `fun name()` on the next line; Java: annotation -> modifiers -> method
            let func_line = if i + 1 < lines.len() {
                let next_line = lines[i + 1].trim();
                if next_line.contains("fun ") || next_line.contains("(") {
                    next_line.to_string()
                } else if i + 2 < lines.len() && lines[i + 2].trim().contains("(") {
                    lines[i + 2].trim().to_string()
                } else {
                    line.trim().to_string()
                }
            } else {
                line.trim().to_string()
            };
            results.push((rel_path, i + 1, func_line));
        }
    }

    println!(
        "{}",
        format!("Providers for '{}' ({}):", type_name, results.len()).bold()
    );

    for (path, line_num, content) in &results {
        println!("  {}:{}", path, line_num);
        let truncated: String = content.chars().take(100).collect();
        println!("    {}", truncated);
    }

    Ok(())
}

/// Captures a suspend function's name, skipping type parameters and an extension
/// receiver (`suspend fun <T> Foo<T>.bar(`) so the receiver type isn't reported.
const SUSPEND_FUN_NAME_PATTERN: &str =
    r"suspend\s+fun\s+(?:<[^>]*>\s*)?(?:[\w.<>?,* ]+\.)?`?(\w+)`?\s*[(<]";

/// Find suspend functions
pub fn cmd_suspend(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let pattern = pattern_with_line_filter(r"suspend\s+fun\s", query);
    let func_regex = Regex::new(SUSPEND_FUN_NAME_PATTERN)?;

    let mut suspends: Vec<(String, String, usize)> = vec![];

    search_files_limited(root, &pattern, &["kt"], limit, |path, line_num, line| {
        if let Some(caps) = func_regex.captures(line) {
            let func_name = caps.get(1).unwrap().as_str().to_string();

            if let Some(q) = query {
                if !func_name.to_lowercase().contains(&q.to_lowercase()) {
                    return;
                }
            }

            let rel_path = relative_path(root, path);
            suspends.push((func_name, rel_path, line_num));
        }
    })?;

    println!(
        "{}",
        format!("Suspend functions ({}):", suspends.len()).bold()
    );

    for (func_name, path, line_num) in &suspends {
        println!("  {}: {}:{}", func_name.cyan(), path, line_num);
    }

    Ok(())
}

/// Find @Composable functions
pub fn cmd_composables(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let func_regex = Regex::new(r"fun\s+(\w+)\s*\(")?;

    // Phase 1: find all .kt files containing @Composable
    let mut file_set: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    search_files_limited(
        root,
        r"@Composable",
        &["kt"],
        100_000,
        |path, _line_num, _line| {
            file_set.insert(path.to_path_buf());
        },
    )?;

    // Phase 2: read each file and find @Composable + fun pairs (multi-line aware)
    let mut composables: Vec<(String, String, usize)> = vec![];
    let mut sorted_files: Vec<_> = file_set.into_iter().collect();
    sorted_files.sort();

    for file_path in &sorted_files {
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let lines: Vec<&str> = content.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            if lines[i].contains("@Composable") {
                // Look at current and next few lines for fun definition
                for j in i..=(i + 5).min(lines.len() - 1) {
                    if let Some(caps) = func_regex.captures(lines[j]) {
                        let func_name = caps.get(1).unwrap().as_str().to_string();

                        if let Some(q) = query {
                            if !func_name.to_lowercase().contains(&q.to_lowercase()) {
                                break;
                            }
                        }

                        let rel_path = relative_path(root, file_path);
                        composables.push((func_name, rel_path, j + 1));
                        i = j;
                        break;
                    }
                }
            }
            i += 1;
        }

        if composables.len() >= limit {
            composables.truncate(limit);
            break;
        }
    }

    composables.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));

    println!(
        "{}",
        format!("@Composable functions ({}):", composables.len()).bold()
    );

    for (func_name, path, line_num) in &composables {
        println!("  {}: {}:{}", func_name.cyan(), path, line_num);
    }

    Ok(())
}

/// Find @Deprecated annotations
pub fn cmd_deprecated(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    // Kotlin/Java/C#: @Deprecated/@Obsolete, Swift: @available(*, deprecated)
    // Python: @deprecated, Perl: DEPRECATED, Rust: #[deprecated], Go: // Deprecated:
    // JS/TS: @deprecated (JSDoc), PHP: @deprecated (PHPDoc), C++: [[deprecated]]
    let pattern = pattern_with_line_filter(
        r"@Deprecated|@Obsolete|@available\s*\([^)]*deprecated|#\[deprecated|#.*DEPRECATED|=head.*DEPRECATED|@deprecated|\[\[deprecated",
        query,
    );

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files_limited(
        root,
        &pattern,
        &ALL_SOURCE_EXTENSIONS,
        limit,
        |path, line_num, line| {
            if let Some(q) = query {
                if !line.to_lowercase().contains(&q.to_lowercase()) {
                    return;
                }
            }

            let rel_path = relative_path(root, path);
            let content: String = line.trim().chars().take(80).collect();
            items.push((rel_path, line_num, content));
        },
    )?;

    println!("{}", format!("@Deprecated items ({}):", items.len()).bold());

    for (path, line_num, content) in &items {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    Ok(())
}

/// Find @Suppress annotations
pub fn cmd_suppress(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let pattern = pattern_with_line_filter(r"@Suppress", query);

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files_limited(root, &pattern, &["kt"], limit, |path, line_num, line| {
        if let Some(q) = query {
            if !line.to_lowercase().contains(&q.to_lowercase()) {
                return;
            }
        }

        let rel_path = relative_path(root, path);
        let content: String = line.trim().chars().take(80).collect();
        items.push((rel_path, line_num, content));
    })?;

    println!(
        "{}",
        format!("@Suppress annotations ({}):", items.len()).bold()
    );

    for (path, line_num, content) in &items {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    Ok(())
}

/// Find @Inject/@Autowired points for a type: field/setter injection and
/// constructor parameters (`class Foo @Inject constructor(bar: Bar)`), where
/// the type usually sits several lines below the annotation.
pub fn cmd_inject(root: &Path, type_name: &str, limit: usize) -> Result<()> {
    let type_pattern = format!(r"\b{}\b", regex::escape(type_name));
    let type_re = Regex::new(&type_pattern)?;

    let mut candidate_files: std::collections::BTreeSet<PathBuf> = Default::default();
    search_files_limited(
        root,
        &type_pattern,
        &["kt", "java"],
        100_000,
        |path, _line_num, _line| {
            candidate_files.insert(path.to_path_buf());
        },
    )?;

    let mut items: Vec<(String, usize, String)> = vec![];
    for path in &candidate_files {
        if items.len() >= limit {
            break;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        if !content.contains("@Inject") && !content.contains("Autowired") {
            continue;
        }
        let lines: Vec<&str> = content.lines().collect();
        let rel_path = relative_path(root, path);
        for line_idx in injection_lines(&content, &type_re) {
            if items.len() >= limit {
                break;
            }
            let text: String = lines
                .get(line_idx)
                .map(|l| l.trim().chars().take(80).collect())
                .unwrap_or_default();
            items.push((rel_path.clone(), line_idx + 1, text));
        }
    }

    println!(
        "{}",
        format!("Injection points for '{}' ({}):", type_name, items.len()).bold()
    );

    for (path, line_num, content) in &items {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    Ok(())
}

static DI_ANNOTATION_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"@(?:\w+:)?(?:Inject|Autowired)\b").expect("valid DI annotation regex")
});

static LEADING_ANNOTATION_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"^\s*@[\w.:]+(?:\s*\([^()]*\))?").expect("valid annotation regex")
});

/// 0-based line indices where `type_re` occurs inside an injection site:
/// the parameter list following `@Inject` (constructor / method injection),
/// or the declaration line of an injected field or property.
fn injection_lines(content: &str, type_re: &Regex) -> Vec<usize> {
    let mut lines = std::collections::BTreeSet::new();
    for di in DI_ANNOTATION_RE.find_iter(content) {
        let mut decl_start = di.end();
        while let Some(a) = LEADING_ANNOTATION_RE.find(&content[decl_start..]) {
            decl_start += a.end();
        }
        let rest = &content[decl_start..];
        let stop = rest.find(['(', ';', '=', '{', '}']);
        let head = &rest[..stop.unwrap_or(rest.len())];
        let is_property = head.split_whitespace().any(|w| w == "var" || w == "val");

        let span = match stop {
            Some(i) if !is_property && rest.as_bytes()[i] == b'(' => {
                let open = decl_start + i;
                match matching_paren(content, open) {
                    Some(close) => open..close,
                    None => continue,
                }
            }
            _ => {
                let offset = rest.len() - rest.trim_start().len();
                let line_start = decl_start + offset;
                let line_end = content[line_start..]
                    .find('\n')
                    .map_or(content.len(), |n| line_start + n);
                let decl_end = stop.map_or(content.len(), |i| decl_start + i);
                line_start..line_end.min(decl_end).max(line_start)
            }
        };

        for m in type_re.find_iter(&content[span.clone()]) {
            let pos = span.start + m.start();
            lines.insert(content[..pos].matches('\n').count());
        }
    }
    lines.into_iter().collect()
}

/// Byte offset of the `)` that closes the `(` at `open`, if any.
fn matching_paren(content: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, b) in content.as_bytes()[open..].iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Find uses of specific annotation
pub fn cmd_annotations(root: &Path, annotation: &str, limit: usize) -> Result<()> {
    // Normalize annotation (add @ if missing for Java/Kotlin/Swift/ObjC)
    // For Perl, attributes are like :lvalue, :method
    let search_annotation = if annotation.starts_with('@') || annotation.starts_with(':') {
        annotation.to_string()
    } else {
        format!("@{}", annotation)
    };
    let pattern = regex::escape(&search_annotation);

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files_limited(
        root,
        &pattern,
        &ALL_SOURCE_EXTENSIONS,
        limit,
        |path, line_num, line| {
            let rel_path = relative_path(root, path);
            let content: String = line.trim().chars().take(80).collect();
            items.push((rel_path, line_num, content));
        },
    )?;

    println!(
        "{}",
        format!("Classes with {} ({}):", search_annotation, items.len()).bold()
    );

    for (path, line_num, content) in &items {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    Ok(())
}

/// Find deeplink definitions
pub fn cmd_deeplinks(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    // Search for specific deeplink patterns (NOT generic :// URLs)
    // Android: @DeepLink, DeepLinkHandler, @AppLink, NavDeepLink, intent-filter with android:scheme
    // iOS: openURL, application(_:open:, handleOpen, CFBundleURLSchemes, UniversalLink
    let pattern = pattern_with_line_filter(
        r#"[Dd]eep[Ll]ink|@DeepLink|DeepLinkHandler|@AppLink|NavDeepLink|android:scheme|openURL|application\([^)]*open:|handleOpen|CFBundleURLSchemes|UniversalLink|NSUserActivity"#,
        query,
    );

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files_limited(
        root,
        &pattern,
        &["kt", "java", "xml", "swift", "m", "h", "plist"],
        limit,
        |path, line_num, line| {
            if let Some(q) = query {
                if !line.to_lowercase().contains(&q.to_lowercase()) {
                    return;
                }
            }

            let rel_path = relative_path(root, path);
            let content: String = line.trim().chars().take(100).collect();
            items.push((rel_path, line_num, content));
        },
    )?;

    println!("{}", format!("Deeplinks ({}):", items.len()).bold());

    for (path, line_num, content) in &items {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    Ok(())
}

/// Find extension functions/types
pub fn cmd_extensions(root: &Path, receiver_type: &str, limit: usize) -> Result<()> {
    // Kotlin: fun ReceiverType.functionName
    // Swift: extension ReceiverType
    let kotlin_pattern = format!(r"fun\s+{}\.(\w+)", regex::escape(receiver_type));
    let swift_pattern = format!(r"extension\s+{}", regex::escape(receiver_type));
    let pattern = format!(r"{}|{}", kotlin_pattern, swift_pattern);

    let kotlin_regex = Regex::new(&kotlin_pattern)?;
    let swift_regex = Regex::new(&swift_pattern)?;

    let mut items: Vec<(String, String, usize, String)> = vec![]; // (name, path, line, lang)

    search_files_limited(
        root,
        &pattern,
        &["kt", "swift"],
        limit,
        |path, line_num, line| {
            let rel_path = relative_path(root, path);

            if let Some(caps) = kotlin_regex.captures(line) {
                let func_name = caps.get(1).unwrap().as_str().to_string();
                items.push((func_name, rel_path, line_num, "kt".to_string()));
            } else if swift_regex.is_match(line) {
                let content: String = line.trim().chars().take(60).collect();
                items.push((content, rel_path, line_num, "swift".to_string()));
            }
        },
    )?;

    println!(
        "{}",
        format!("Extensions for {} ({}):", receiver_type, items.len()).bold()
    );

    for (name, path, line_num, lang) in &items {
        if lang == "kt" {
            println!("  {}.{}: {}:{}", receiver_type.cyan(), name, path, line_num);
        } else {
            println!("  {}:{} {}", path.cyan(), line_num, name);
        }
    }

    Ok(())
}

/// Find Flow declarations
pub fn cmd_flows(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    // The search pattern must be exactly the extraction regex: lines like
    // `.asStateFlow()` would otherwise consume the limit without producing a result.
    let flow_pattern = r"\b(MutableStateFlow|MutableSharedFlow|StateFlow|SharedFlow|Flow)<";
    let pattern = pattern_with_line_filter(flow_pattern, query);
    let flow_regex = Regex::new(flow_pattern)?;

    let mut items: Vec<(String, String, usize, String)> = vec![];

    search_files_limited(root, &pattern, &["kt"], limit, |path, line_num, line| {
        if let Some(caps) = flow_regex.captures(line) {
            let flow_type = caps.get(1).unwrap().as_str().to_string();

            if let Some(q) = query {
                if !line.to_lowercase().contains(&q.to_lowercase()) {
                    return;
                }
            }

            let rel_path = relative_path(root, path);
            let content: String = line.trim().chars().take(70).collect();
            items.push((flow_type, rel_path, line_num, content));
        }
    })?;

    println!("{}", format!("Flow declarations ({}):", items.len()).bold());

    for (flow_type, path, line_num, content) in &items {
        println!("  [{}] {}:{}", flow_type.cyan(), path, line_num);
        println!("    {}", content);
    }

    Ok(())
}

/// Find @Preview functions
pub fn cmd_previews(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let func_regex = Regex::new(r"fun\s+(\w+)\s*\(")?;

    // Phase 1: find all .kt files containing @Preview
    let mut file_set: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    search_files_limited(
        root,
        r"@Preview",
        &["kt"],
        100_000,
        |path, _line_num, _line| {
            file_set.insert(path.to_path_buf());
        },
    )?;

    // Phase 2: read each file and find @Preview + fun pairs (multi-line aware)
    let mut items: Vec<(String, String, usize)> = vec![];
    let mut sorted_files: Vec<_> = file_set.into_iter().collect();
    sorted_files.sort();

    for file_path in &sorted_files {
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let lines: Vec<&str> = content.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            if lines[i].contains("@Preview") {
                // Look at current and next few lines for fun definition
                for j in i..=(i + 5).min(lines.len() - 1) {
                    if let Some(caps) = func_regex.captures(lines[j]) {
                        let func_name = caps.get(1).unwrap().as_str().to_string();

                        if let Some(q) = query {
                            if !func_name.to_lowercase().contains(&q.to_lowercase()) {
                                break;
                            }
                        }

                        let rel_path = relative_path(root, file_path);
                        items.push((func_name, rel_path, j + 1));
                        i = j;
                        break;
                    }
                }
            }
            i += 1;
        }

        if items.len() >= limit {
            items.truncate(limit);
            break;
        }
    }

    items.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));

    println!(
        "{}",
        format!("@Preview functions ({}):", items.len()).bold()
    );

    for (func_name, path, line_num) in &items {
        println!("  {}: {}:{}", func_name.cyan(), path, line_num);
    }

    Ok(())
}

/// Structural code search via ast-grep (requires `sg` or `ast-grep` installed)
pub fn cmd_ast_grep(root: &Path, pattern: &str, lang: Option<&str>, json: bool) -> Result<()> {
    // Find ast-grep binary
    let binary = find_ast_grep_binary()
        .ok_or_else(|| anyhow::anyhow!(
            "ast-grep not found. Install it:\n  brew install ast-grep    # macOS\n  npm i -g @ast-grep/cli   # npm\n  cargo install ast-grep    # cargo"
        ))?;

    let mut cmd = std::process::Command::new(&binary);
    cmd.arg("run")
        .arg("--pattern")
        .arg(pattern)
        .current_dir(root);

    if let Some(lang) = lang {
        cmd.arg("--lang").arg(lang);
    }

    if json {
        cmd.arg("--json=compact");
    }

    let status = cmd.status()?;

    if !status.success() && status.code() != Some(1) {
        // Exit code 1 = no matches (normal for grep), anything else is an error
        anyhow::bail!("ast-grep exited with code {:?}", status.code());
    }

    Ok(())
}

fn find_ast_grep_binary() -> Option<String> {
    for name in &["sg", "ast-grep"] {
        if std::process::Command::new(name)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
        {
            return Some(name.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    fn inject_lines(src: &str, ty: &str) -> Vec<usize> {
        let re = Regex::new(&format!(r"\b{}\b", regex::escape(ty))).unwrap();
        injection_lines(src, &re)
            .into_iter()
            .map(|l| l + 1)
            .collect()
    }

    #[test]
    fn inject_finds_kotlin_constructor_parameters() {
        let src = "class A @Inject constructor(\n    private val repo: Lazy<Repo>,\n    @Named(\"x\") private val other: Other,\n) {\n    val unrelated: Repo? = null\n}\n";
        assert_eq!(inject_lines(src, "Repo"), vec![2]);
        assert_eq!(inject_lines(src, "Other"), vec![3]);
    }

    #[test]
    fn inject_finds_fields_with_annotation_on_previous_line() {
        let src = "class A {\n    @Inject\n    lateinit var repo: Repo\n    @field:Inject lateinit var other: Other\n    fun f(r: Repo) {}\n}\n";
        assert_eq!(inject_lines(src, "Repo"), vec![3]);
        assert_eq!(inject_lines(src, "Other"), vec![4]);
    }

    #[test]
    fn inject_finds_java_constructor_and_field() {
        let src = "class A {\n  @Inject @Named(\"a\") Repo repo;\n  @Inject\n  public A(Other o,\n           Repo r) {\n  }\n}\n";
        assert_eq!(inject_lines(src, "Repo"), vec![2, 5]);
        assert_eq!(inject_lines(src, "Other"), vec![4]);
    }

    #[test]
    fn pattern_with_line_filter_requires_both_parts() {
        let re = Regex::new(&pattern_with_line_filter(r"@Suppress", Some("unchecked"))).unwrap();
        assert!(re.is_match(r#"@Suppress("UNCHECKED_CAST")"#));
        assert!(!re.is_match(r#"@Suppress("DEPRECATION")"#));
        assert_eq!(pattern_with_line_filter("x", None), "x");
    }

    #[test]
    fn suspend_regex_skips_extension_receiver() {
        let re = Regex::new(SUSPEND_FUN_NAME_PATTERN).unwrap();
        let name = |l: &str| re.captures(l).map(|c| c[1].to_string());
        assert_eq!(
            name("override suspend fun ScreenStackNavigator.handle(x: X)").as_deref(),
            Some("handle")
        );
        assert_eq!(
            name("suspend fun <T> Flow<T>.firstOrNull(): T?").as_deref(),
            Some("firstOrNull")
        );
        assert_eq!(
            name("suspend fun load(id: String)").as_deref(),
            Some("load")
        );
    }

    use super::*;

    // --- build_caller_pattern tests ---

    fn matches(pattern: &str, text: &str) -> bool {
        Regex::new(pattern).unwrap().is_match(text)
    }

    #[test]
    fn test_caller_pattern_dot_call_with_parens() {
        let pat = build_caller_pattern("perform_async");
        assert!(matches(&pat, "  MyWorker.perform_async(id)"));
        assert!(matches(&pat, "  worker.perform_async(1, 2)"));
    }

    #[test]
    fn test_caller_pattern_dot_call_without_parens() {
        let pat = build_caller_pattern("process");
        // Ruby: obj.method without parens
        assert!(matches(&pat, "  new(*args).process"));
        assert!(matches(&pat, "  service.process"));
    }

    #[test]
    fn test_caller_pattern_bare_call_with_parens() {
        let pat = build_caller_pattern("normalize_phone");
        assert!(matches(&pat, "  normalized = normalize_phone(number)"));
        assert!(matches(&pat, "    if result = normalize_phone(input)"));
    }

    #[test]
    fn test_caller_pattern_symbol_ref() {
        let pat = build_caller_pattern("set_timestamps");
        // Ruby callbacks: before_action :method_name
        assert!(matches(&pat, "  before_save :set_timestamps"));
        assert!(matches(
            &pat,
            "  after_create :set_timestamps, if: :active?"
        ));
    }

    #[test]
    fn test_caller_pattern_method_chain() {
        let pat = build_caller_pattern("recalc_counters");
        // Ruby: bare method.chain
        assert!(matches(&pat, "    recalc_counters.where(job_id: job.id)"));
    }

    #[test]
    fn test_caller_pattern_no_false_positives_in_substring() {
        let pat = build_caller_pattern("process");
        // Should NOT match "preprocess" as a bare call with parens
        assert!(!matches(&pat, "  preprocess(data)"));
    }

    #[test]
    fn test_caller_pattern_ruby_bang_method() {
        let pat = build_caller_pattern("authenticate_user!");
        // Ruby callbacks with bang methods
        assert!(matches(&pat, "  before_action :authenticate_user!"));
        assert!(matches(
            &pat,
            "  skip_before_action :authenticate_user!, only: [:index]"
        ));
        // Direct calls
        assert!(matches(&pat, "  authenticate_user!(request)"));
        assert!(matches(&pat, "  current_user.authenticate_user!"));
    }

    #[test]
    fn test_caller_pattern_ruby_question_method() {
        let pat = build_caller_pattern("valid?");
        assert!(matches(&pat, "  record.valid?"));
        assert!(matches(&pat, "  valid?(params)"));
    }

    #[test]
    fn bare_predicate_and_bang_calls_are_calls() {
        let pat = build_caller_pattern("next_page?");
        assert!(matches(&pat, "    break unless next_page?"));
        assert!(matches(&pat, "    if next_page? && more"));
        assert!(matches(&pat, "next_page?"));
        assert!(matches(&pat, "  \"page #{next_page?}\""));
        assert!(matches(&pat, "  (response.next_page?)"));
        assert!(!matches(&pat, "  has_next_page?"));
        assert!(!matches(&pat, "  describe \"#next_page?\" do"));
        assert!(build_def_skip_pattern("next_page?").is_match("  def next_page?"));

        let pat = build_caller_pattern("save!");
        assert!(matches(&pat, "    save!"));
        assert!(matches(&pat, "    save! if dirty"));
        assert!(!matches(&pat, "    save!= other"));
        assert!(!matches(&pat, "    save!~ /x/"));

        // Any other word, bare, may be a local variable.
        assert!(!matches(&build_caller_pattern("perform"), "    perform"));
        assert!(!matches(&build_caller_pattern("a::b?"), "    a::b?"));
    }

    #[test]
    fn test_caller_pattern_await_bare_call() {
        let pat = build_caller_pattern("fetchCategories");
        assert!(matches(&pat, "  await fetchCategories()"));
        assert!(matches(&pat, "  const result = await fetchCategories()"));
    }

    #[test]
    fn test_caller_pattern_await_method_call() {
        let pat = build_caller_pattern("loadEventRecords");
        assert!(matches(&pat, "  await store.loadEventRecords()"));
        assert!(matches(&pat, "  await this.loadEventRecords()"));
    }

    #[test]
    fn test_caller_pattern_return_bare_call() {
        let pat = build_caller_pattern("pluralize");
        assert!(matches(&pat, "  return pluralize(count, forms)"));
    }

    #[test]
    fn test_caller_pattern_return_method_call() {
        let pat = build_caller_pattern("serialize");
        assert!(matches(&pat, "  return serializer.serialize()"));
    }

    #[test]
    fn test_caller_pattern_await_chained() {
        let pat = build_caller_pattern("addAction");
        assert!(matches(&pat, "  await syncQueue.addAction(action)"));
    }

    #[test]
    fn test_caller_pattern_same_file_calls() {
        // Common Pinia store / composable pattern: functions calling each other
        let pat = build_caller_pattern("loadFromDB");
        // Bare call (no await)
        assert!(matches(&pat, "    loadFromDB()"));
        // Await call (most common in stores)
        assert!(matches(&pat, "    await loadFromDB()"));
        // Definition line should NOT match
        assert!(!matches(&pat, "  const loadFromDB = async () => {"));
        // Return call
        assert!(matches(&pat, "    return loadFromDB()"));
    }

    #[test]
    fn every_caller_idiom_contains_the_function_name() {
        // The batched call-tree scan skips a name's pattern on lines that do
        // not contain the name; an idiom without `{fn}` would lose matches.
        for idiom in CALLER_IDIOMS {
            assert!(idiom.contains("{fn}"), "{idiom}");
        }
        assert!(BARE_PREDICATE_CALL_IDIOM.contains("{fn}"));
    }

    #[test]
    fn any_caller_pattern_matches_each_names_calls() {
        let names = [
            "perform".to_string(),
            "save!".to_string(),
            "valid?".to_string(),
            "let(:fields)".to_string(),
        ];
        let any = Regex::new(&build_any_caller_pattern(&names)).unwrap();
        let lines = [
            "  Worker.new.perform",
            "  before_action :perform",
            "  record.save!",
            "  skip_callback :save!, if: :x",
            "  return valid?(record)",
            "  await store.valid?(x)",
            "  let(:fields).tap { }",
            "  save!",
            "  return if valid? && ready",
        ];
        for line in lines {
            assert!(
                names
                    .iter()
                    .any(|name| matches(&build_caller_pattern(name), line)),
                "{line}"
            );
            assert!(any.is_match(line), "{line}");
        }
        assert!(!any.is_match("  preperform(data)"));
    }

    fn sites(entries: &[(&str, &str, usize)]) -> CallerSites {
        entries
            .iter()
            .map(|(caller, file, line)| (caller.to_string(), file.to_string(), *line))
            .collect()
    }

    fn walk(
        function_name: &str,
        max_depth: usize,
        callers: &HashMap<String, CallerSites>,
    ) -> (Vec<String>, Vec<String>) {
        let mut edges = vec![];
        let missing = walk_call_tree(
            function_name,
            max_depth,
            callers,
            &mut |depth, caller, site| {
                edges.push(match site {
                    Some((file, line)) => format!("{depth} {caller} {file}:{line}"),
                    None => format!("{depth} {caller} recursive"),
                });
            },
        );
        (edges, missing)
    }

    #[test]
    fn walk_call_tree_reports_what_it_lacks_without_descending() {
        let mut callers = HashMap::new();
        let (edges, missing) = walk("leaf", 3, &callers);
        assert!(edges.is_empty());
        assert_eq!(missing, ["leaf"]);

        callers.insert(
            "leaf".to_string(),
            sites(&[("alpha", "a.rb", 2), ("beta", "b.rb", 6)]),
        );
        let (edges, missing) = walk("leaf", 3, &callers);
        assert_eq!(edges, ["1 alpha a.rb:2", "1 beta b.rb:6"]);
        assert_eq!(missing, ["alpha", "beta"]);
    }

    #[test]
    fn walk_call_tree_marks_callers_already_shown_as_recursive() {
        let callers: HashMap<String, CallerSites> = [
            ("leaf", sites(&[("alpha", "a.rb", 2), ("beta", "b.rb", 6)])),
            ("alpha", sites(&[("top", "t.rb", 2), ("leaf", "a.rb", 9)])),
            ("beta", sites(&[("top", "t.rb", 2)])),
            ("top", sites(&[])),
        ]
        .into_iter()
        .map(|(name, sites)| (name.to_string(), sites))
        .collect();
        let (edges, missing) = walk("leaf", 3, &callers);
        assert_eq!(
            edges,
            [
                "1 alpha a.rb:2",
                "2 top t.rb:2",
                "2 leaf recursive",
                "1 beta b.rb:6",
                "2 top recursive",
            ]
        );
        assert!(missing.is_empty());
    }

    #[test]
    fn walk_call_tree_needs_no_callers_below_the_depth_limit() {
        let callers: HashMap<String, CallerSites> =
            [("leaf".to_string(), sites(&[("alpha", "a.rb", 2)]))]
                .into_iter()
                .collect();
        let (edges, missing) = walk("leaf", 1, &callers);
        assert_eq!(edges, ["1 alpha a.rb:2"]);
        assert!(missing.is_empty());
        assert_eq!(walk("leaf", 0, &callers), (vec![], vec![]));
    }

    #[test]
    fn walk_call_tree_shows_uncallable_callers_without_expanding_them() {
        let callers: HashMap<String, CallerSites> = [(
            "leaf",
            sites(&[
                ("it \"works\"", "leaf_spec.rb", 2),
                ("let(:user)", "leaf_spec.rb", 5),
                ("save!", "record.rb", 3),
            ]),
        )]
        .into_iter()
        .map(|(name, sites)| (name.to_string(), sites))
        .collect();
        let (edges, missing) = walk("leaf", 3, &callers);
        assert_eq!(
            edges,
            [
                "1 it \"works\" leaf_spec.rb:2",
                "1 let(:user) leaf_spec.rb:5",
                "1 save! record.rb:3",
            ]
        );
        assert_eq!(missing, ["save!"]);
    }

    #[test]
    fn callable_names_are_identifiers() {
        for name in [
            "perform",
            "save!",
            "valid?",
            "name=",
            "Applicant::MergeService",
            "::TopLevel",
            "self.call",
            "React.memo",
            "$onChange",
            "#secret",
            "button-variant",
            "ПолучитьДанные",
            "_private",
        ] {
            assert!(is_callable_name(name), "{name}");
        }
    }

    #[test]
    fn dsl_block_names_are_not_callable() {
        for name in [
            "it \"does nothing\"",
            "let(:fields)",
            "let!(:company)",
            "subject(:perform)",
            "describe \"Applicant::MergeService\"",
            "attributes :ext_id",
            "include first_name: [:presence]",
            "scope :active",
            ":result",
            "default(firebase)",
            "`backticked name`",
            "[]",
            "==",
            "a:b",
            "save!!",
            "valid?x",
            "",
        ] {
            assert!(!is_callable_name(name), "{name}");
        }
    }

    // --- build_def_skip_pattern tests ---

    #[test]
    fn test_def_skip_ruby_bang_method() {
        let pat = build_def_skip_pattern("authenticate_user!");
        assert!(pat.is_match("  def authenticate_user!"));
        assert!(pat.is_match("  def self.authenticate_user!"));
    }

    #[test]
    fn test_def_skip_ruby_instance_method() {
        let pat = build_def_skip_pattern("process");
        assert!(pat.is_match("  def process"));
        assert!(pat.is_match("  def process(args)"));
    }

    #[test]
    fn test_def_skip_ruby_self_method() {
        let pat = build_def_skip_pattern("call");
        assert!(pat.is_match("  def self.call(params)"));
        assert!(pat.is_match("  def self.call"));
    }

    #[test]
    fn test_def_skip_does_not_match_calls() {
        let pat = build_def_skip_pattern("process");
        assert!(!pat.is_match("  service.process"));
        assert!(!pat.is_match("  result = process(data)"));
    }

    #[test]
    fn test_def_skip_kotlin_fun() {
        let pat = build_def_skip_pattern("calculate");
        assert!(pat.is_match("  fun calculate(x: Int)"));
    }

    #[test]
    fn def_skip_keeps_calls_behind_a_keyword() {
        let calls = [
            ("foo", "    return foo(x)"),
            ("foo", "  const y = await foo(x)"),
            ("Foo", "    throw new Foo(message)"),
            ("foo", "  } else foo(x)"),
            ("foo", "    yield foo(x)"),
            ("foo", "    puts foo(x)"),
            ("Foo", "    raise Foo(message)"),
            ("foo", "    if foo(x)"),
            ("foo", "    elsif foo(x)"),
            ("foo", "  for item in foo(items):"),
            ("foo", "  for (const item of foo(items)) {"),
            ("foo", "export default foo(App)"),
            ("foo", "  go foo(ch)"),
            ("foo", "  defer foo(conn)"),
            ("foo", "  echo foo($x);"),
            ("foo", "  with foo(path) as handle:"),
            ("foo", "    assert foo(x)"),
            ("foo", "  match foo(x) {"),
            ("foo", "  return await foo(x)"),
        ];
        for (name, line) in calls {
            assert!(matches(&build_caller_pattern(name), line), "{line}");
            assert!(!build_def_skip_pattern(name).is_match(line), "{line}");
        }
    }

    #[test]
    fn def_skip_still_recognises_typed_definitions() {
        let definitions = [
            "  public void foo(int x) {",
            "  String foo(String x) {",
            "  public static List<String> foo(Map<String, Integer> x) {",
            "  int[] foo() {",
            "  private override fun foo() {",
            "export function foo(x) {",
            "  async function foo(x) {",
            "  public foo(x: number): void {",
            "  static foo() {",
            "  async foo() {",
            "  private def foo(x)",
            "  defp foo(x) do",
            "pub fn foo(x: u32) -> u32 {",
            "local function foo(x)",
        ];
        let pat = build_def_skip_pattern("foo");
        for line in definitions {
            assert!(pat.is_match(line), "{line}");
        }
    }

    // --- find_containing_function tests ---

    #[test]
    fn test_find_containing_ruby_method() {
        let code = vec![
            "class MyService",
            "  def process",
            "    result = other_service.call(data)",
            "    transform(result)",
            "  end",
            "end",
        ];
        let func_def_re = Regex::new(
            concat!(
                r"(?:fun|func|sub)\s+(\w+)\s*[<(\[]",
                r"|\bdef\s+(?:self\.)?(\w[!\w?]*)",
                r"|(?:(?:public|private|protected|static|final|abstract|synchronized|override)\s+)*",
                r"(?:void|int|long|boolean|char|byte|short|float|double|[\w.]+(?:<[^{;]*>)?(?:\[\])*)\s+(\w+)\s*\(",
            )
        ).unwrap();

        // Line 3 (0-indexed) = "    result = other_service.call(data)"
        let result = find_containing_function(&code, 3, &func_def_re);
        assert_eq!(result, Some(("process".to_string(), 2)));
    }

    #[test]
    fn test_find_containing_ruby_self_method() {
        let code = vec![
            "class MyService",
            "  def self.call(params)",
            "    new(params).process",
            "  end",
            "end",
        ];
        let func_def_re = Regex::new(
            concat!(
                r"(?:fun|func|sub)\s+(\w+)\s*[<(\[]",
                r"|\bdef\s+(?:self\.)?(\w[!\w?]*)",
                r"|(?:(?:public|private|protected|static|final|abstract|synchronized|override)\s+)*",
                r"(?:void|int|long|boolean|char|byte|short|float|double|[\w.]+(?:<[^{;]*>)?(?:\[\])*)\s+(\w+)\s*\(",
            )
        ).unwrap();

        let result = find_containing_function(&code, 3, &func_def_re);
        assert_eq!(result, Some(("call".to_string(), 2)));
    }

    #[test]
    fn test_find_containing_ruby_bang_method() {
        let code = vec![
            "class Updater",
            "  def update!",
            "    record.save!",
            "  end",
            "end",
        ];
        let func_def_re = Regex::new(
            concat!(
                r"(?:fun|func|sub)\s+(\w+)\s*[<(\[]",
                r"|\bdef\s+(?:self\.)?(\w[!\w?]*)",
                r"|(?:(?:public|private|protected|static|final|abstract|synchronized|override)\s+)*",
                r"(?:void|int|long|boolean|char|byte|short|float|double|[\w.]+(?:<[^{;]*>)?(?:\[\])*)\s+(\w+)\s*\(",
            )
        ).unwrap();

        let result = find_containing_function(&code, 3, &func_def_re);
        assert_eq!(result, Some(("update!".to_string(), 2)));
    }
}
