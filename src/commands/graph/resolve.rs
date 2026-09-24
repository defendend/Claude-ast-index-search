//! Turning name-based references into symbol-to-symbol edges: which
//! definition owns each reference, and which definition its name denotes.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use rayon::prelude::*;
use regex::Regex;
use rusqlite::Connection;
use serde::Serialize;

use super::metrics::compute_metrics;
use super::rust::{crate_name, module_location, parse_uses, FileUses, ModuleScope};
use super::schema::{column_candidates, link_models, underscore, ModelClass, SchemaLinkSummary};
use super::{
    is_container_kind, is_node_kind, is_path_suffix, is_schema_kind, language_family, short_name,
    Confidence, AMBIGUITY_CAP, DEPENDENTS_DEPTH,
};
use crate::commands::is_test_path;
use crate::db::{self, SymbolEdgeRow};

/// Inheritance chains deeper than this are cut; real hierarchies are shallow
/// and a cap keeps a malformed cyclic `inheritance` table from looping.
const MAX_ANCESTOR_DEPTH: usize = 8;

fn strip_source_extension(path: &str) -> &str {
    let file_start = path.rfind('/').map(|index| index + 1).unwrap_or(0);
    match path[file_start..].rfind('.') {
        Some(dot) if dot > 0 => &path[..file_start + dot],
        _ => path,
    }
}

fn normalize_relative(dir: &str, spec: &str) -> String {
    let mut parts: Vec<&str> = if dir.is_empty() {
        Vec::new()
    } else {
        dir.split('/').collect()
    };
    for segment in spec.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

#[derive(Clone, Debug)]
struct ImportTarget {
    path: String,
    /// `true` for load-path or alias style specifiers (`require "a/b"`,
    /// `from "shared/components/Button"`) that match by path suffix.
    bare: bool,
    /// `/path`, precomputed for suffix matching of bare specifiers.
    slashed: String,
}

impl ImportTarget {
    fn new(path: &str, bare: bool) -> ImportTarget {
        ImportTarget {
            path: path.to_string(),
            bare,
            slashed: format!("/{path}"),
        }
    }

    fn parse(importing_path: &str, spec: &str) -> Option<ImportTarget> {
        let spec = spec.trim().trim_matches(|c| c == '\'' || c == '"');
        if spec.is_empty() || spec.chars().any(char::is_whitespace) {
            return None;
        }
        if spec.starts_with('.') {
            let dir = importing_path
                .rsplit_once('/')
                .map(|(dir, _)| dir)
                .unwrap_or("");
            let resolved = normalize_relative(dir, spec);
            (!resolved.is_empty())
                .then(|| ImportTarget::new(strip_source_extension(&resolved), false))
        } else {
            let spec = spec
                .strip_prefix("@/")
                .or_else(|| spec.strip_prefix("~/"))
                .unwrap_or(spec)
                .trim_start_matches('/');
            (!spec.is_empty()).then(|| ImportTarget::new(strip_source_extension(spec), true))
        }
    }

    /// A directory import also matches files below it, which covers the
    /// `index.js` re-export pattern without reading the index file.
    fn matches(&self, candidate_stem: &str) -> bool {
        let path = self.path.as_str();
        if candidate_stem == path {
            return true;
        }
        let below = |stem: &str| {
            stem.len() > path.len() && stem.starts_with(path) && stem.as_bytes()[path.len()] == b'/'
        };
        if below(candidate_stem) {
            return true;
        }
        if !self.bare {
            return false;
        }
        let needle = self.slashed.as_str();
        candidate_stem.ends_with(needle)
            || candidate_stem
                .match_indices(needle)
                .any(|(at, _)| candidate_stem.as_bytes().get(at + needle.len()) == Some(&b'/'))
    }

    fn is_exact(&self, candidate_stem: &str) -> bool {
        let path = self.path.as_str();
        let exact = |stem: &str| stem == path || stem.strip_suffix("/index") == Some(path);
        if exact(candidate_stem) {
            return true;
        }
        self.bare
            && (candidate_stem.ends_with(self.slashed.as_str())
                || candidate_stem
                    .strip_suffix("/index")
                    .is_some_and(|stem| stem.ends_with(self.slashed.as_str())))
    }
}

/// Import bindings of one JavaScript / TypeScript module, read from source.
#[derive(Default)]
struct ModuleImports {
    /// local name -> (exported name, index into `FileNode::imports`)
    bindings: HashMap<String, (String, usize)>,
    /// `import * as ns` -> index into `FileNode::imports`
    namespaces: HashMap<String, usize>,
}

fn js_import_regex() -> &'static Regex {
    static CELL: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        Regex::new(r#"(?m)^[ \t]*import\s+(?:type\s+)?([^;'"]*?)\s*from\s*['"]([^'"\n]+)['"]"#)
            .expect("import regex must compile")
    })
}

fn js_require_regex() -> &'static Regex {
    static CELL: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        Regex::new(
            r#"\b(?:const|let|var)\s+(\{[^}]*\}|[A-Za-z_$][\w$]*)\s*=\s*require\(\s*['"]([^'"\n]+)['"]\s*\)"#,
        )
        .expect("require regex must compile")
    })
}

fn is_js_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// Record the bindings of one import clause (`Default, { a, b as c }`,
/// `* as ns`, or a CommonJS destructuring pattern with `alias` = `:`).
fn add_import_clause(clause: &str, target: usize, alias: &str, imports: &mut ModuleImports) {
    let clause = clause.trim();
    let (outside, named) = match (clause.find('{'), clause.rfind('}')) {
        (Some(open), Some(close)) if close > open => (
            format!("{}{}", &clause[..open], &clause[close + 1..]),
            Some(&clause[open + 1..close]),
        ),
        _ => (clause.to_string(), None),
    };
    if let Some(named) = named {
        for item in named.split(',') {
            let item = item.trim();
            let item = item.strip_prefix("type ").unwrap_or(item).trim();
            let (exported, local) = match item.split_once(alias) {
                Some((exported, local)) => (exported.trim(), local.trim()),
                None => (item, item),
            };
            if !is_js_identifier(local) {
                continue;
            }
            let exported = if exported == "default" || !is_js_identifier(exported) {
                local
            } else {
                exported
            };
            imports
                .bindings
                .insert(local.to_string(), (exported.to_string(), target));
        }
    }
    for part in outside.split(',') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix('*') {
            if let Some(namespace) = rest.trim().strip_prefix("as") {
                let namespace = namespace.trim();
                if is_js_identifier(namespace) {
                    imports.namespaces.insert(namespace.to_string(), target);
                }
            }
        } else if is_js_identifier(part) {
            imports
                .bindings
                .insert(part.to_string(), (part.to_string(), target));
        }
    }
}

fn parse_module_imports(
    path: &str,
    content: &str,
    targets: &mut Vec<ImportTarget>,
) -> ModuleImports {
    let mut imports = ModuleImports::default();
    let mut add = |clause: &str, spec: &str, alias: &str, imports: &mut ModuleImports| {
        if let Some(target) = ImportTarget::parse(path, spec) {
            targets.push(target);
            add_import_clause(clause, targets.len() - 1, alias, imports);
        }
    };
    for captures in js_import_regex().captures_iter(content) {
        add(&captures[1], &captures[2], " as ", &mut imports);
    }
    if content.contains("require(") {
        for captures in js_require_regex().captures_iter(content) {
            add(&captures[1], &captures[2], ":", &mut imports);
        }
    }
    imports
}

/// What `graph build` reads from a file's source besides the index: the
/// import bindings of a JavaScript / TypeScript module, the `use`
/// declarations of a Rust file.
enum ParsedSource {
    Js(ModuleImports, Vec<ImportTarget>),
    Rust(FileUses),
}

// ---------------------------------------------------------------------------
// Reference usage classification (from the stored line text)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Usage<'a> {
    /// Plain `Name` or `name(`.
    Bare,
    /// Every occurrence is a namespace qualifier (`Name::Other`): the
    /// dependency is on `Other`, which has its own reference row.
    Namespace,
    /// A hash key, keyword argument or object key (`name: value`).
    Label,
    /// The whole of a string literal (`'Name'`).
    StringLiteral,
    /// Inside prose: a string that is not a constant path, or a comment.
    Prose,
    /// A Ruby symbol (`:name`), e.g. a callback or a stubbed method name.
    Symbol,
    /// The name is not in the stored (truncated) line text, so the way it
    /// is used is unknown.
    Unseen,
    /// `A::B::Name` (`absolute` for a leading `::`).
    Qualified { absolute: bool, path: &'a str },
    /// `self.name` / `this.name`.
    SelfReceiver,
    /// `Type.name` / `A::Type.name`.
    TypeReceiver(&'a str),
    /// `value.name` (token `value`), `call().name` or `.name` on a
    /// continuation line (empty token).
    UnknownReceiver(&'a str),
}

fn is_ident_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// Longest suffix of `text` made only of characters accepted by `allowed`.
fn trailing_run(text: &str, allowed: impl Fn(char) -> bool) -> &str {
    let mut start = text.len();
    for (index, character) in text.char_indices().rev() {
        if !allowed(character) {
            break;
        }
        start = index;
    }
    &text[start..]
}

/// Where a byte offset of a source line falls: code, a string literal
/// (with the offset of its opening quote), or a trailing comment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lexical {
    Code,
    String { open: usize, quote: char },
    Comment,
}

/// Scan `line[..offset]` tracking quotes, escapes, `#{}` / `${}`
/// interpolation and trailing comments. Heuristic by design: the stored
/// context is one (possibly truncated) line, not a token stream.
fn lexical_state(line: &str, offset: usize, ruby: bool) -> Lexical {
    let mut state: Vec<Lexical> = vec![Lexical::Code];
    let mut chars = line[..offset].char_indices().peekable();
    while let Some((index, character)) = chars.next() {
        let current = *state.last().unwrap_or(&Lexical::Code);
        match current {
            Lexical::Comment => return Lexical::Comment,
            Lexical::Code => match character {
                '\'' | '"' | '`' => state.push(Lexical::String {
                    open: index,
                    quote: character,
                }),
                '#' if ruby && chars.peek().map(|(_, c)| *c) != Some('{') => {
                    return Lexical::Comment
                }
                '/' if !ruby && chars.peek().map(|(_, c)| *c) == Some('/') => {
                    return Lexical::Comment
                }
                '}' if state.len() > 1 => {
                    state.pop();
                }
                _ => {}
            },
            Lexical::String { quote, .. } => match character {
                '\\' => {
                    chars.next();
                }
                c if c == quote => {
                    state.pop();
                }
                '#' | '$' if quote != '\'' && chars.peek().map(|(_, c)| *c) == Some('{') => {
                    chars.next();
                    state.push(Lexical::Code);
                }
                _ => {}
            },
        }
    }
    *state.last().unwrap_or(&Lexical::Code)
}

fn is_constant_path(text: &str) -> bool {
    let text = text.trim_start_matches("::");
    !text.is_empty()
        && text.split("::").all(|segment| {
            segment.chars().next().is_some_and(char::is_uppercase)
                && segment.chars().all(|c| c.is_alphanumeric() || c == '_')
        })
}

fn classify_usage<'a>(context: Option<&'a str>, name: &str, ruby: bool) -> Usage<'a> {
    let Some(context) = context else {
        return Usage::Bare;
    };
    // The reference extractor records a lowercase name when it is called:
    // `name(` in every language, and in Ruby also `recv.name` and a bare
    // `name` that is not a local. A call with parentheses on the line is the
    // likeliest producer of the row; `:name` and `name:` never are.
    let call_only = name.chars().next().is_some_and(char::is_lowercase)
        && !name.ends_with('?')
        && !name.ends_with('!');
    let terminated = name.ends_with('?') || name.ends_with('!');
    let occurrences: Vec<usize> = context
        .match_indices(name)
        .map(|(position, _)| position)
        .filter(|&position| {
            let previous = context[..position].chars().next_back();
            let next = context[position + name.len()..].chars().next();
            !previous.is_some_and(|c| is_ident_char(c) || c == '@' || c == '$')
                && (terminated || !next.is_some_and(is_ident_char))
        })
        .collect();
    if occurrences.is_empty() {
        return Usage::Unseen;
    }
    let called: Vec<usize> = occurrences
        .iter()
        .copied()
        .filter(|&position| {
            context[position + name.len()..]
                .trim_start()
                .starts_with('(')
        })
        .collect();
    let candidates = if call_only && !called.is_empty() {
        called
    } else {
        occurrences
    };
    let mut saw_namespace = false;
    let mut saw_label = false;
    let mut saw_prose = false;
    for position in candidates {
        let before = &context[..position];
        let after = &context[position + name.len()..];
        match lexical_state(context, position, ruby) {
            Lexical::Comment => {
                saw_prose = true;
                continue;
            }
            Lexical::String { open, quote } => {
                let content_start = open + quote.len_utf8();
                let content_end = after
                    .find(quote)
                    .map(|index| position + name.len() + index)
                    .unwrap_or(context.len());
                if !is_constant_path(&context[content_start..content_end]) {
                    saw_prose = true;
                    continue;
                }
                if after.starts_with("::") {
                    saw_namespace = true;
                    continue;
                }
                if content_start == position {
                    return Usage::StringLiteral;
                }
            }
            Lexical::Code => {}
        }
        if after.starts_with("::") {
            saw_namespace = true;
            continue;
        }
        let next = after.chars().next();
        if next == Some(':') && !before.trim_end().ends_with("case") {
            saw_label = true;
            continue;
        }
        if ruby && before.ends_with(':') && !before.ends_with("::") {
            // `map(&:total)` and the methods `delegate` forwards are called
            // on another object, not on the surrounding class.
            if before.ends_with("&:") || is_delegated_name(context, before) {
                return Usage::UnknownReceiver("");
            }
            return Usage::Symbol;
        }
        return classify_prefix(before);
    }
    if saw_namespace {
        Usage::Namespace
    } else if saw_label {
        Usage::Label
    } else if saw_prose {
        Usage::Prose
    } else {
        Usage::Bare
    }
}

/// A method name `delegate` forwards (`delegate :name, to: :owner` — `name`,
/// not the `owner` target, which is a method of the class itself).
fn is_delegated_name(context: &str, before: &str) -> bool {
    context.trim_start().starts_with("delegate ")
        && !before.trim_end_matches(':').trim_end().ends_with("to:")
}

fn classify_prefix(before: &str) -> Usage<'_> {
    if before.ends_with("::") {
        let chain = trailing_run(before, |c| is_ident_char(c) || c == ':');
        let path = chain.trim_start_matches("::").trim_end_matches("::");
        // Rust `Self::name` names an item of the enclosing `impl` block.
        if path == "Self" {
            return Usage::SelfReceiver;
        }
        return Usage::Qualified {
            absolute: chain.starts_with("::"),
            path,
        };
    }
    let trimmed = before.trim_end();
    let Some(receiver) = trimmed.strip_suffix('.') else {
        return Usage::Bare;
    };
    // `..` / `...` are ranges and spreads, not member access.
    if receiver.ends_with('.') {
        return Usage::Bare;
    }
    let receiver = receiver
        .strip_suffix('&')
        .or_else(|| receiver.strip_suffix('?'))
        .unwrap_or(receiver);
    let token = trailing_run(receiver, |c| {
        is_ident_char(c) || c == ':' || c == '@' || c == '$'
    });
    match token {
        "self" | "this" | "super" => Usage::SelfReceiver,
        _ => {
            let bare = token.trim_start_matches("::");
            if bare.chars().next().is_some_and(char::is_uppercase) {
                Usage::TypeReceiver(token)
            } else {
                Usage::UnknownReceiver(token)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Graph builder
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DropReason {
    /// The reference sits outside every definition (file-level code).
    NoOwner,
    /// No definition with that name exists in the same language, or a
    /// JavaScript name that the module imports from a package or never
    /// imports at all.
    External,
    /// The reference is the definition's own declaration line.
    Declaration,
    /// Only used as a namespace qualifier; the qualified name carries the edge.
    Namespace,
    /// Used as a hash / object key or keyword argument, not as a reference.
    Label,
    /// Only mentioned in a comment or in the prose of a string.
    Prose,
    /// `A::B::Name` where no indexed definition has that qualified name.
    QualifiedUnresolved,
    /// `Type.name` where neither `Type` nor its ancestors define `name`, or a
    /// member call on a value of unknown type in a module-scoped language.
    ReceiverUnresolved,
    /// More than [`AMBIGUITY_CAP`] candidates.
    TooAmbiguous,
    /// Recursion or a reference resolving to its own owner.
    SelfReference,
}

impl DropReason {
    fn as_str(self) -> &'static str {
        match self {
            DropReason::NoOwner => "no_owner",
            DropReason::External => "external",
            DropReason::Declaration => "declaration",
            DropReason::Namespace => "namespace",
            DropReason::Label => "label",
            DropReason::Prose => "prose",
            DropReason::QualifiedUnresolved => "qualified_unresolved",
            DropReason::ReceiverUnresolved => "receiver_unresolved",
            DropReason::TooAmbiguous => "too_ambiguous",
            DropReason::SelfReference => "self_reference",
        }
    }
}

struct FileNode {
    path: String,
    stem: String,
    family: &'static str,
    /// An installed package ([`db::is_third_party_path`]): neither the source
    /// nor the target of an edge, because resolving a project name against
    /// every copy under `node_modules` only multiplies ambiguity.
    vendor: bool,
    /// Under a test directory or named like a test (see [`is_test_path`]).
    test: bool,
    has_ranges: bool,
    symbols: Vec<u32>,
    imports: Vec<ImportTarget>,
    /// Import bindings read from source; `Some` means the language scopes
    /// names to the module, so a name that is neither defined in the file nor
    /// imported cannot denote another file's definition.
    module: Option<ModuleImports>,
    /// Rust only: the crate ([`RustIndex::crates`]) and the module path the
    /// file is (`commands::graph` for `src/commands/graph/mod.rs`). Its
    /// top-level definitions live in that namespace.
    rust_crate: Option<u32>,
    namespace: String,
}

/// Rust crates and the `use` scope of every module, read from source.
#[derive(Default)]
struct RustIndex {
    /// Crate key ([`super::rust::ModuleLocation::crate_key`], per root) -> id.
    crates: HashMap<String, u32>,
    /// Library name other crates write (`ast_index`) -> crate id.
    by_name: HashMap<String, u32>,
    /// `(crate, module path)` of every module: each file's, the modules its
    /// path goes through, and inline `mod` blocks.
    modules: HashSet<(u32, String)>,
    scopes: HashMap<(u32, String), ModuleScope>,
}

/// Rust paths that never name a definition of the project.
fn is_external_rust_root(segment: &str) -> bool {
    matches!(segment, "std" | "core" | "alloc")
}

fn join_path(base: &str, rest: &[&str]) -> String {
    let mut path = base.to_string();
    for segment in rest {
        if !path.is_empty() {
            path.push_str("::");
        }
        path.push_str(segment);
    }
    path
}

struct SymNode {
    id: i64,
    file: u32,
    name: String,
    kind: String,
    line: i64,
    /// `end_line`, or the start line when the parser reports no range.
    end: i64,
    has_range: bool,
    container: Option<u32>,
    /// Namespace path (`Outer::Inner::name`) used for scoped resolution.
    qual: String,
    /// Nested in a block that is not a namespace (`describe "..."`,
    /// `let(:x)`), so no other file can reference it.
    file_private: bool,
}

#[derive(Clone, Debug)]
struct Resolution {
    confidence: Confidence,
    targets: Vec<u32>,
}

impl Resolution {
    fn new(confidence: Confidence, targets: Vec<u32>) -> Resolution {
        Resolution {
            confidence,
            targets,
        }
    }
}

struct Builder {
    files: Vec<FileNode>,
    file_index: HashMap<i64, u32>,
    syms: Vec<SymNode>,
    by_short: HashMap<String, Vec<u32>>,
    /// Namespace path (`Outer::Inner::name`) -> definitions with that path.
    by_qual: HashMap<String, Vec<u32>>,
    /// Namespace path of a class -> its resolved ancestors. Keyed by path
    /// rather than by definition because Ruby classes are reopened across
    /// files and every reopening shares the same ancestors.
    parents: HashMap<String, Vec<u32>>,
    /// Namespace path of a class -> its superclass as written and, when the
    /// graph resolved it, the superclass's namespace path.
    superclasses: HashMap<String, (String, Option<String>)>,
    /// Namespace path of a Rails model -> the `db/schema.rb` tables it reads.
    model_tables: HashMap<String, Vec<u32>>,
    /// Table definition -> its columns by column name.
    table_columns: HashMap<u32, HashMap<String, u32>>,
    schema: Option<SchemaLinkSummary>,
    rust: RustIndex,
    /// Per importing file: whether one of its imports points at a file,
    /// keyed by that file. Every reference to a common name asks this for
    /// the same candidate files again, and each answer is a scan of the
    /// file's import specifiers.
    import_memo: Vec<std::sync::Mutex<HashMap<u32, bool>>>,
}

fn absolute_file_path(root: &Path, root_path: &str, path: &str) -> std::path::PathBuf {
    if root_path.is_empty() {
        root.join(path)
    } else {
        Path::new(root_path).join(path)
    }
}

impl Builder {
    fn load(conn: &Connection, root: &Path) -> Result<Builder> {
        let file_rows = db::load_graph_files(conn)?;
        let parsed: Vec<Option<ParsedSource>> = file_rows
            .par_iter()
            .map(|row| {
                let family = language_family(&row.path);
                if !matches!(family, "js" | "rust") || db::is_third_party_path(&row.path) {
                    return None;
                }
                let content =
                    std::fs::read_to_string(absolute_file_path(root, &row.root_path, &row.path))
                        .ok()?;
                if family == "rust" {
                    return parse_uses(&content).map(ParsedSource::Rust);
                }
                let mut imports = Vec::new();
                let module = parse_module_imports(&row.path, &content, &mut imports);
                Some(ParsedSource::Js(module, imports))
            })
            .collect();
        let mut files = Vec::with_capacity(file_rows.len());
        let mut file_index = HashMap::with_capacity(file_rows.len());
        let mut rust = RustIndex::default();
        let mut rust_uses: Vec<(u32, FileUses)> = Vec::new();
        for (row, parsed) in file_rows.into_iter().zip(parsed) {
            let index = files.len() as u32;
            file_index.insert(row.id, index);
            let family = language_family(&row.path);
            let vendor = db::is_third_party_path(&row.path);
            let (module, imports) = match parsed {
                Some(ParsedSource::Js(module, imports)) => (Some(module), imports),
                Some(ParsedSource::Rust(uses)) => {
                    rust_uses.push((index, uses));
                    (None, Vec::new())
                }
                None => (None, Vec::new()),
            };
            let (rust_crate, namespace) = if family == "rust" && !vendor {
                let location = module_location(&row.path);
                let key = format!("{}\0{}", row.root_path, location.crate_key);
                let next = rust.crates.len() as u32;
                let id = *rust.crates.entry(key).or_insert(next);
                if id == next {
                    if let Some(name) = location.package.as_deref().and_then(|package| {
                        crate_name(&absolute_file_path(root, &row.root_path, ""), package)
                    }) {
                        rust.by_name.entry(name).or_insert(id);
                    }
                }
                let mut prefix = String::new();
                rust.modules.insert((id, String::new()));
                for segment in location.module.split("::").filter(|s| !s.is_empty()) {
                    prefix = join_path(&prefix, &[segment]);
                    rust.modules.insert((id, prefix.clone()));
                }
                (Some(id), location.module)
            } else {
                (None, String::new())
            };
            files.push(FileNode {
                stem: strip_source_extension(&row.path).to_string(),
                family,
                vendor,
                test: is_test_path(&row.path),
                path: row.path,
                has_ranges: false,
                symbols: Vec::new(),
                imports,
                module,
                rust_crate,
                namespace,
            });
        }

        let symbol_rows = db::load_graph_symbols(conn)?;
        let mut syms = Vec::with_capacity(symbol_rows.len());
        for row in symbol_rows {
            let Some(&file) = file_index.get(&row.file_id) else {
                continue;
            };
            let index = syms.len() as u32;
            let file_node = &mut files[file as usize];
            file_node.symbols.push(index);
            if row.end_line.is_some() {
                file_node.has_ranges = true;
            }
            if row.kind == "import" && file_node.module.is_none() {
                if let Some(target) = ImportTarget::parse(&file_node.path, &row.name) {
                    file_node.imports.push(target);
                }
            }
            syms.push(SymNode {
                id: row.id,
                file,
                end: row.end_line.unwrap_or(row.line).max(row.line),
                has_range: row.end_line.is_some(),
                name: row.name,
                kind: row.kind,
                line: row.line,
                container: None,
                qual: String::new(),
                file_private: false,
            });
        }

        let import_memo = files.iter().map(|_| Default::default()).collect();
        let mut builder = Builder {
            import_memo,
            files,
            file_index,
            syms,
            by_short: HashMap::new(),
            by_qual: HashMap::new(),
            parents: HashMap::new(),
            superclasses: HashMap::new(),
            model_tables: HashMap::new(),
            table_columns: HashMap::new(),
            schema: None,
            rust,
        };
        builder.assign_containers();
        builder.assign_rust_scopes(rust_uses);
        builder.index_short_names();
        builder.resolve_parents(conn)?;
        builder.link_schema();
        Ok(builder)
    }

    /// Sweep each file's symbols outer-first and attach every symbol to the
    /// narrowest class-like range enclosing it, then derive namespace paths.
    fn assign_containers(&mut self) {
        for file_index in 0..self.files.len() {
            let file = &self.files[file_index];
            if file.vendor {
                continue;
            }
            let ruby = file.family == "ruby";
            let namespace = file.namespace.clone();
            let mut order = file.symbols.clone();
            order.sort_by_key(|&s| {
                let sym = &self.syms[s as usize];
                (sym.line, Reverse(sym.end), sym.id)
            });
            let mut stack: Vec<u32> = Vec::new();
            for s in order {
                let (line, end) = {
                    let sym = &self.syms[s as usize];
                    (sym.line, sym.end)
                };
                while stack
                    .last()
                    .is_some_and(|&top| self.syms[top as usize].end < line)
                {
                    stack.pop();
                }
                let container = stack
                    .iter()
                    .rev()
                    .copied()
                    .find(|&c| self.syms[c as usize].end >= end);
                let qual = {
                    let sym = &self.syms[s as usize];
                    let reopened = is_container_kind(&sym.kind)
                        .then(|| reopened_type(&sym.name))
                        .flatten();
                    let short = reopened
                        .or_else(|| short_name(&sym.name))
                        .unwrap_or(&sym.name);
                    // Singleton methods keep their `self.` marker so `Type.call`
                    // and an instance-level `call` stay distinct definitions.
                    let singleton;
                    let own = if sym.name.starts_with("self.") && !short.is_empty() {
                        singleton = format!("self.{short}");
                        singleton.as_str()
                    } else {
                        short
                    };
                    // Ruby parsers qualify `class A::B` and `A::B = value`
                    // with their enclosing scopes already.
                    let qualified_by_parser = (is_container_kind(&sym.kind) && reopened.is_none())
                        || (sym.kind == "constant" && ruby);
                    if qualified_by_parser && sym.name.contains("::") {
                        sym.name.trim_start_matches("::").to_string()
                    } else if let Some(c) = container {
                        format!("{}::{}", self.syms[c as usize].qual, own)
                    } else {
                        join_path(&namespace, &[own])
                    }
                };
                let file_private = container.is_some_and(|c| {
                    let outer = &self.syms[c as usize];
                    outer.file_private
                        || (short_name(&outer.name).is_none()
                            && reopened_type(&outer.name).is_none())
                });
                let sym = &mut self.syms[s as usize];
                sym.container = container;
                sym.qual = qual;
                sym.file_private = file_private;
                if is_container_kind(&sym.kind) && sym.has_range {
                    stack.push(s);
                }
            }
        }
    }

    /// File a Rust file's `use` declarations under the module they sit in —
    /// the file's own, or the inline `mod` block around them — and record
    /// inline modules as modules.
    fn assign_rust_scopes(&mut self, uses: Vec<(u32, FileUses)>) {
        for file in &self.files {
            let Some(krate) = file.rust_crate else {
                continue;
            };
            for &s in &file.symbols {
                let sym = &self.syms[s as usize];
                if sym.kind == "package" {
                    self.rust.modules.insert((krate, sym.qual.clone()));
                }
            }
        }
        for (file, file_uses) in uses {
            let node = &self.files[file as usize];
            let Some(krate) = node.rust_crate else {
                continue;
            };
            let module_at = |line: i64| -> String {
                node.symbols
                    .iter()
                    .map(|&s| &self.syms[s as usize])
                    .filter(|sym| sym.kind == "package" && sym.line < line && sym.end >= line)
                    .min_by_key(|sym| sym.end - sym.line)
                    .map(|sym| sym.qual.clone())
                    .unwrap_or_else(|| node.namespace.clone())
            };
            for binding in file_uses.bindings {
                self.rust
                    .scopes
                    .entry((krate, module_at(binding.line)))
                    .or_default()
                    .bindings
                    .entry(binding.local)
                    .or_insert(binding.path);
            }
            for (line, glob) in file_uses.globs {
                self.rust
                    .scopes
                    .entry((krate, module_at(line)))
                    .or_default()
                    .globs
                    .push(glob);
            }
        }
    }

    /// Schema tables and columns are left out: a column is only reachable
    /// through the model that reads its table (see [`Builder::link_schema`]),
    /// never by name alone.
    fn index_short_names(&mut self) {
        for (index, sym) in self.syms.iter().enumerate() {
            if self.files[sym.file as usize].vendor
                || !is_node_kind(&sym.kind)
                || is_schema_kind(&sym.kind)
            {
                continue;
            }
            if let Some(short) = short_name(&sym.name) {
                self.by_short
                    .entry(short.to_string())
                    .or_default()
                    .push(index as u32);
                self.by_qual
                    .entry(sym.qual.clone())
                    .or_default()
                    .push(index as u32);
            }
        }
    }

    /// Resolve `inheritance` rows and Ruby `include` / `extend` / `prepend`
    /// lines into class -> ancestor links used for inherited lookups.
    fn resolve_parents(&mut self, conn: &Connection) -> Result<()> {
        let id_index: HashMap<i64, u32> = self
            .syms
            .iter()
            .enumerate()
            .map(|(index, sym)| (sym.id, index as u32))
            .collect();
        // (class, parent name, namespace the name is written in, superclass?)
        let mut links: Vec<(u32, String, String, bool)> = Vec::new();
        for (child_id, parent_name) in db::load_inheritance_rows(conn)? {
            let Some(&child) = id_index.get(&child_id) else {
                continue;
            };
            // `class A::B < C` resolves `C` where the `class` keyword sits,
            // not inside `A::B` itself.
            let child_sym = &self.syms[child as usize];
            let namespace = child_sym
                .container
                .map(|c| self.syms[c as usize].qual.clone())
                .unwrap_or_else(|| self.files[child_sym.file as usize].namespace.clone());
            links.push((child, parent_name, namespace, true));
        }
        for sym in &self.syms {
            if sym.kind != "annotation" {
                continue;
            }
            let Some(container) = sym.container else {
                continue;
            };
            for keyword in ["include ", "extend ", "prepend "] {
                if let Some(rest) = sym.name.strip_prefix(keyword) {
                    let namespace = self.syms[container as usize].qual.clone();
                    links.push((container, rest.to_string(), namespace, false));
                }
            }
        }
        for (child, parent_name, namespace, superclass) in links {
            let path = parent_name
                .trim()
                .split(|c: char| c == '[' || c == '(' || c == '<' || c.is_whitespace() || c == ',')
                .next()
                .unwrap_or("");
            let is_constant_path = !path.is_empty()
                && path
                    .trim_start_matches("::")
                    .split("::")
                    .all(|segment| segment.chars().next().is_some_and(char::is_uppercase));
            if !is_constant_path {
                continue;
            }
            let types = self.resolve_type(child, &namespace, path, Some(child));
            if superclass && self.family_of(child) == "ruby" {
                let resolved = match types.as_slice() {
                    [parent] => Some(self.syms[*parent as usize].qual.clone()),
                    _ => None,
                };
                self.superclasses
                    .entry(self.syms[child as usize].qual.clone())
                    .or_insert((path.to_string(), resolved));
            }
            if let [parent] = types.as_slice() {
                let child_qual = self.syms[child as usize].qual.clone();
                if self.syms[*parent as usize].qual != child_qual {
                    let entry = self.parents.entry(child_qual).or_default();
                    if !entry.contains(parent) {
                        entry.push(*parent);
                    }
                }
            }
        }
        Ok(())
    }

    /// Match Rails models to the tables of `db/schema.rb` (see
    /// [`super::schema`]) and index each table's columns by name.
    fn link_schema(&mut self) {
        let mut tables: HashMap<String, Vec<u32>> = HashMap::new();
        let mut table_at: HashMap<(u32, &str), u32> = HashMap::new();
        let mut classes: HashMap<String, ModelClass> = HashMap::new();
        for (index, sym) in self.syms.iter().enumerate() {
            let file = &self.files[sym.file as usize];
            if file.vendor || file.family != "ruby" {
                continue;
            }
            let index = index as u32;
            match sym.kind.as_str() {
                "table" => {
                    tables.entry(sym.name.clone()).or_default().push(index);
                    table_at.insert((sym.file, sym.name.as_str()), index);
                }
                "class" if !sym.file_private => {
                    let test = is_test_path(&file.path);
                    let class = classes.entry(sym.qual.clone()).or_insert(ModelClass {
                        test_only: true,
                        ..ModelClass::default()
                    });
                    class.test_only &= test;
                }
                _ => {}
            }
        }
        if tables.is_empty() {
            return;
        }
        for (qual, (written, resolved)) in &self.superclasses {
            if let Some(class) = classes.get_mut(qual) {
                class.superclass_written = Some(written.clone());
                class.superclass = resolved.clone();
            }
        }
        let mut column_entries: Vec<(u32, String, u32)> = Vec::new();
        let mut prefixes: HashMap<String, String> = HashMap::new();
        for (index, sym) in self.syms.iter().enumerate() {
            match sym.kind.as_str() {
                "annotation" => {
                    if let Some(module) = sym.name.strip_prefix("isolate_namespace ") {
                        let module = module.trim().trim_start_matches("::");
                        let engine: Vec<String> = module.split("::").map(underscore).collect();
                        prefixes
                            .entry(module.to_string())
                            .or_insert(format!("{}_", engine.join("_")));
                        continue;
                    }
                    if let Some(prefix) = sym
                        .name
                        .strip_prefix("table_name_prefix \"")
                        .and_then(|rest| rest.strip_suffix('"'))
                    {
                        if let Some(container) = sym.container {
                            prefixes.insert(
                                self.syms[container as usize].qual.clone(),
                                prefix.to_string(),
                            );
                        }
                        continue;
                    }
                    let Some(class) = sym
                        .container
                        .and_then(|c| classes.get_mut(&self.syms[c as usize].qual))
                    else {
                        continue;
                    };
                    if sym.name == "abstract_class" {
                        class.abstract_class = true;
                    } else if let Some(table) = sym
                        .name
                        .strip_prefix("table_name \"")
                        .and_then(|rest| rest.strip_suffix('"'))
                    {
                        class.explicit_table = Some(table.to_string());
                    }
                }
                "column" => {
                    let Some((table, column)) = sym.name.split_once('.') else {
                        continue;
                    };
                    let Some(&table) = table_at.get(&(sym.file, table)) else {
                        continue;
                    };
                    column_entries.push((table, column.to_string(), index as u32));
                }
                _ => {}
            }
        }
        let columns = column_entries.len() as u64;
        for (table, column, index) in column_entries {
            self.table_columns
                .entry(table)
                .or_default()
                .insert(column, index);
        }
        let names: std::collections::HashSet<String> = tables.keys().cloned().collect();
        let (links, summary) = link_models(&classes, &names, &prefixes, columns);
        for (qual, table) in links {
            if let Some(defs) = tables.get(&table) {
                self.model_tables.insert(qual, defs.clone());
            }
        }
        self.schema = Some(summary);
    }

    fn family_of(&self, sym: u32) -> &'static str {
        self.files[self.syms[sym as usize].file as usize].family
    }

    fn stem_of(&self, sym: u32) -> &str {
        &self.files[self.syms[sym as usize].file as usize].stem
    }

    /// Definitions named `name` in the same language family as `file`.
    /// Definitions nested in test DSL blocks (`let(:x)` inside
    /// `describe "..."`) are private to their file and never cross it.
    fn candidates(&self, name: &str, family: &str, file: u32) -> Vec<u32> {
        self.by_short
            .get(name)
            .map(|all| {
                all.iter()
                    .copied()
                    .filter(|&c| {
                        let sym = &self.syms[c as usize];
                        self.family_of(c) == family
                            && (sym.file == file || !sym.file_private)
                            && self.visible_from(file, c)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Production code never depends on a definition under a test tree: a
    /// spec support file that reopens `ApplicationWorker` to stub
    /// `perform_async` is not what `Worker.perform_async` calls.
    fn visible_from(&self, file: u32, candidate: u32) -> bool {
        self.files[file as usize].test
            || !self.files[self.syms[candidate as usize].file as usize].test
    }

    /// Namespace a reference inside `scope` is looked up from: the class or
    /// module around it, else the file's own module (Rust), else the top.
    fn namespace_of(&self, scope: u32) -> &str {
        let sym = &self.syms[scope as usize];
        if is_container_kind(&sym.kind) {
            return &sym.qual;
        }
        match sym.container {
            Some(container) => &self.syms[container as usize].qual,
            None => &self.files[sym.file as usize].namespace,
        }
    }

    /// Nearest class-like symbol at or around `scope`.
    fn class_scope(&self, scope: u32) -> Option<u32> {
        let sym = &self.syms[scope as usize];
        if is_container_kind(&sym.kind) {
            Some(scope)
        } else {
            sym.container
        }
    }

    /// Ruby-style constant lookup: try `rel` under every enclosing namespace
    /// from the innermost outwards, then at the top level.
    fn lexical_match(
        &self,
        from: u32,
        namespace: &str,
        absolute: bool,
        rel: &str,
        accept: impl Fn(u32) -> bool,
    ) -> Vec<u32> {
        let exact = |prefix: &str| -> Vec<u32> {
            let key = if prefix.is_empty() {
                rel.to_string()
            } else {
                format!("{prefix}::{rel}")
            };
            self.by_qual
                .get(&key)
                .map(|found| {
                    found
                        .iter()
                        .copied()
                        .filter(|&c| self.visible_from(from, c) && accept(c))
                        .collect()
                })
                .unwrap_or_default()
        };
        if absolute {
            return exact("");
        }
        let mut prefix = namespace;
        loop {
            let found = exact(prefix);
            if !found.is_empty() || prefix.is_empty() {
                return found;
            }
            prefix = prefix.rsplit_once("::").map(|(head, _)| head).unwrap_or("");
        }
    }

    fn suffix_match(&self, rel: &str, cands: &[u32]) -> Vec<u32> {
        cands
            .iter()
            .copied()
            .filter(|&c| is_path_suffix(&self.syms[c as usize].qual, rel))
            .collect()
    }

    /// Candidates whose file an import of `file` points at.
    fn imported(&self, file: u32, cands: &[u32]) -> Vec<u32> {
        let imports = &self.files[file as usize].imports;
        if imports.is_empty() {
            return Vec::new();
        }
        let mut memo = self.import_memo[file as usize]
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        cands
            .iter()
            .copied()
            .filter(|&c| {
                let target_file = self.syms[c as usize].file;
                *memo.entry(target_file).or_insert_with(|| {
                    let stem = &self.files[target_file as usize].stem;
                    imports.iter().any(|target| target.matches(stem))
                })
            })
            .collect()
    }

    /// Definitions of `exported` in the module one import points at, with the
    /// file the specifier names exactly (or its `index`) preferred over files
    /// merely below an imported directory.
    fn in_import_target(
        &self,
        file: u32,
        target: usize,
        exported: &str,
        container_only: bool,
    ) -> Vec<u32> {
        let node = &self.files[file as usize];
        let target = &node.imports[target];
        let found: Vec<u32> = self
            .candidates(exported, node.family, file)
            .into_iter()
            .filter(|&c| {
                (!container_only || is_container_kind(&self.syms[c as usize].kind))
                    && target.matches(self.stem_of(c))
            })
            .collect();
        if found.len() > 1 {
            let exact: Vec<u32> = found
                .iter()
                .copied()
                .filter(|&c| target.is_exact(self.stem_of(c)))
                .collect();
            if !exact.is_empty() {
                let top: Vec<u32> = exact
                    .iter()
                    .copied()
                    .filter(|&c| self.syms[c as usize].container.is_none())
                    .collect();
                return if top.is_empty() { exact } else { top };
            }
        }
        found
    }

    /// Resolve a constant path used from `scope` to class-like definitions.
    fn resolve_type(
        &self,
        scope: u32,
        namespace: &str,
        path: &str,
        exclude: Option<u32>,
    ) -> Vec<u32> {
        let absolute = path.starts_with("::");
        let rel = path.trim_start_matches("::");
        let name = rel.rsplit("::").next().unwrap_or(rel);
        let file = self.syms[scope as usize].file;
        let node = &self.files[file as usize];
        let cands: Vec<u32> = self
            .candidates(name, node.family, file)
            .into_iter()
            .filter(|&c| is_container_kind(&self.syms[c as usize].kind) && Some(c) != exclude)
            .collect();
        if let Some(module) = &node.module {
            let local: Vec<u32> = cands
                .iter()
                .copied()
                .filter(|&c| self.syms[c as usize].file == file)
                .collect();
            if !local.is_empty() {
                return local;
            }
            return match module.bindings.get(name) {
                Some((exported, target)) => self.in_import_target(file, *target, exported, true),
                None => Vec::new(),
            };
        }
        if cands.is_empty() {
            return cands;
        }
        let family = node.family;
        let ruby = family == "ruby";
        let found = self.lexical_match(file, namespace, absolute, rel, |c| {
            let kind = self.syms[c as usize].kind.as_str();
            (is_container_kind(kind) || (ruby && kind == "constant"))
                && self.family_of(c) == family
                && Some(c) != exclude
        });
        if !found.is_empty() || absolute {
            // Ruby constant lookup stops at the nearest scope defining the
            // name: `Billing::Import = injector` hides a top-level `module
            // Import` from code inside `Billing`, and is itself no class.
            let types: Vec<u32> = found
                .into_iter()
                .filter(|&c| is_container_kind(&self.syms[c as usize].kind))
                .collect();
            return self.collapse_reopened(types);
        }
        let found = self.suffix_match(rel, &cands);
        if !found.is_empty() {
            return self.collapse_reopened(found);
        }
        let local: Vec<u32> = cands
            .iter()
            .copied()
            .filter(|&c| self.syms[c as usize].file == file)
            .collect();
        if !local.is_empty() {
            return local;
        }
        let imported = self.imported(file, &cands);
        if !imported.is_empty() {
            return imported;
        }
        if cands.len() == 1 {
            return cands;
        }
        Vec::new()
    }

    /// Ruby reopens classes and modules: every `module Billing` wrapper in
    /// every file is the same constant. Keep one canonical definition per
    /// namespace path — the file Zeitwerk would autoload it from, outside
    /// test trees, a class over a module, the widest range.
    fn collapse_reopened(&self, found: Vec<u32>) -> Vec<u32> {
        if found.len() < 2 || self.family_of(found[0]) != "ruby" {
            return found;
        }
        let mut best: HashMap<&str, u32> = HashMap::new();
        let mut order: Vec<&str> = Vec::new();
        for &candidate in &found {
            let qual = self.syms[candidate as usize].qual.as_str();
            match best.get(qual) {
                None => {
                    best.insert(qual, candidate);
                    order.push(qual);
                }
                Some(&current) => {
                    if self.canonical_key(candidate) > self.canonical_key(current) {
                        best.insert(qual, candidate);
                    }
                }
            }
        }
        order.into_iter().map(|qual| best[qual]).collect()
    }

    fn canonical_key(
        &self,
        candidate: u32,
    ) -> (bool, bool, bool, i64, Reverse<usize>, Reverse<i64>) {
        let sym = &self.syms[candidate as usize];
        let file = &self.files[sym.file as usize];
        let autoload = autoload_path(&sym.qual);
        let autoloaded = file.stem == autoload
            || (file.stem.ends_with(autoload.as_str())
                && file.stem.as_bytes()[file.stem.len() - autoload.len() - 1] == b'/');
        (
            autoloaded,
            !is_test_path(&file.path),
            sym.kind == "class",
            sym.end - sym.line,
            Reverse(file.path.len()),
            Reverse(sym.id),
        )
    }

    /// Pick among several matches of one level: the one sharing the longest
    /// namespace prefix with the source wins; a tie stays ambiguous.
    fn pick(&self, source: u32, found: Vec<u32>, level: Confidence) -> Option<Resolution> {
        let found = self.collapse_reopened(found);
        match found.len() {
            0 => None,
            1 => Some(Resolution::new(level, found)),
            _ => {
                let namespace = self.namespace_of(source);
                let score = |c: u32| common_namespace_depth(namespace, &self.syms[c as usize].qual);
                let best = found.iter().map(|&c| score(c)).max().unwrap_or(0);
                let winners: Vec<u32> = found
                    .iter()
                    .copied()
                    .filter(|&c| score(c) == best)
                    .collect();
                if best > 0 && winners.len() == 1 {
                    Some(Resolution::new(level, winners))
                } else {
                    Some(Resolution::new(Confidence::Ambiguous, found))
                }
            }
        }
    }

    fn resolve_local(&self, file: u32, source: u32, cands: &[u32]) -> Option<Resolution> {
        let same: Vec<u32> = cands
            .iter()
            .copied()
            .filter(|&c| self.syms[c as usize].file == file)
            .collect();
        match same.len() {
            0 => None,
            1 => Some(Resolution::new(Confidence::Local, same)),
            _ => {
                let mut scope = Some(source);
                while let Some(current) = scope {
                    let hits: Vec<u32> = same
                        .iter()
                        .copied()
                        .filter(|&c| self.syms[c as usize].container == Some(current))
                        .collect();
                    if hits.len() == 1 {
                        return Some(Resolution::new(Confidence::Local, hits));
                    }
                    if hits.len() > 1 {
                        let singleton = self.in_singleton_context(source);
                        let flavoured: Vec<u32> = hits
                            .iter()
                            .copied()
                            .filter(|&c| {
                                self.syms[c as usize].name.starts_with("self.") == singleton
                            })
                            .collect();
                        if flavoured.len() == 1 {
                            return Some(Resolution::new(Confidence::Local, flavoured));
                        }
                        break;
                    }
                    scope = self.syms[current as usize].container;
                }
                let top: Vec<u32> = same
                    .iter()
                    .copied()
                    .filter(|&c| self.syms[c as usize].container.is_none())
                    .collect();
                if top.len() == 1 {
                    return Some(Resolution::new(Confidence::Local, top));
                }
                Some(Resolution::new(Confidence::Ambiguous, same))
            }
        }
    }

    /// Definitions inherited by `class`: the class itself (reopened in other
    /// files) and then its ancestors, nearest first. `singleton` says which
    /// flavour of method the call site reaches first (`Type.name` and calls
    /// inside `def self.x` reach singleton methods); the other flavour is
    /// only tried when the whole chain has none of the preferred one.
    fn resolve_in_hierarchy(
        &self,
        class: u32,
        source: u32,
        name: &str,
        singleton: bool,
    ) -> Option<Resolution> {
        let preferred = if singleton {
            format!("self.{name}")
        } else {
            name.to_string()
        };
        let fallback = if singleton {
            name.to_string()
        } else {
            format!("self.{name}")
        };
        self.walk_hierarchy(class, source, &preferred)
            .or_else(|| self.walk_hierarchy(class, source, &fallback))
    }

    fn walk_hierarchy(&self, class: u32, source: u32, member: &str) -> Option<Resolution> {
        let family = self.family_of(source);
        let file = self.syms[source as usize].file;
        let mut queue = VecDeque::from([(class, 0usize)]);
        let mut seen: HashSet<u32> = HashSet::from([class]);
        while let Some((current, depth)) = queue.pop_front() {
            let key = format!("{}::{}", self.syms[current as usize].qual, member);
            let hits: Vec<u32> = self
                .by_qual
                .get(&key)
                .map(|found| {
                    found
                        .iter()
                        .copied()
                        .filter(|&c| {
                            self.family_of(c) == family && c != source && self.visible_from(file, c)
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !hits.is_empty() {
                return self.pick(source, hits, Confidence::Scoped);
            }
            if depth >= MAX_ANCESTOR_DEPTH {
                continue;
            }
            if let Some(parents) = self.parents.get(&self.syms[current as usize].qual) {
                for &parent in parents {
                    if seen.insert(parent) {
                        queue.push_back((parent, depth + 1));
                    }
                }
            }
        }
        None
    }

    /// Whether code inside `source` runs at class level (`def self.x`, or a
    /// class body line), where a bare call reaches singleton methods.
    fn in_singleton_context(&self, source: u32) -> bool {
        let sym = &self.syms[source as usize];
        sym.name.starts_with("self.") || is_container_kind(&sym.kind)
    }

    /// Methods of the enclosing class and its ancestors first; a Rails
    /// model's columns only when no code in the chain defines the name.
    fn resolve_in_class_scope(&self, source: u32, name: &str) -> Option<Resolution> {
        let singleton = self.in_singleton_context(source);
        let class = self.class_scope(source)?;
        self.resolve_in_hierarchy(class, source, name, singleton)
            .or_else(|| self.resolve_column(class, source, name))
    }

    /// A column of the table the model `class` reads, for a reader or an
    /// attribute method (`status`, `status?`, `saved_change_to_status?`)
    /// called on an instance. Code in `def self.x` runs on the class, where
    /// column readers do not exist.
    fn resolve_column(&self, class: u32, source: u32, name: &str) -> Option<Resolution> {
        if self.syms[source as usize].name.starts_with("self.") {
            return None;
        }
        let tables = self.model_tables.get(&self.syms[class as usize].qual)?;
        for column in column_candidates(name) {
            let hits: Vec<u32> = tables
                .iter()
                .filter_map(|table| self.table_columns.get(table)?.get(column).copied())
                .collect();
            match hits.len() {
                0 => continue,
                1 => return Some(Resolution::new(Confidence::Scoped, hits)),
                _ => return Some(Resolution::new(Confidence::Ambiguous, hits)),
            }
        }
        None
    }

    /// Last resort by name alone. `weak` references (a receiver of unknown
    /// type, a string literal, a Ruby constant no lexical scope defines) can
    /// never be `unique`: the one in-repo definition may well not be what
    /// they denote.
    fn resolve_by_name(
        &self,
        file: u32,
        source: u32,
        cands: &[u32],
        weak: bool,
    ) -> Result<Resolution, DropReason> {
        if cands.is_empty() {
            return Err(DropReason::External);
        }
        let imported = self.imported(file, cands);
        if let Some(found) = self.pick(source, imported, Confidence::Import) {
            return Ok(found);
        }
        if cands.len() == 1 && !weak {
            return Ok(Resolution::new(Confidence::Unique, cands.to_vec()));
        }
        if cands.len() <= AMBIGUITY_CAP {
            return Ok(Resolution::new(Confidence::Ambiguous, cands.to_vec()));
        }
        Err(DropReason::TooAmbiguous)
    }

    fn resolve_on_type(
        &self,
        source: u32,
        name: &str,
        type_path: &str,
        cands: &[u32],
    ) -> Result<Resolution, DropReason> {
        let types = self.resolve_type(source, self.namespace_of(source), type_path, None);
        if let [class] = types.as_slice() {
            return self
                .resolve_in_hierarchy(*class, source, name, true)
                .ok_or(DropReason::ReceiverUnresolved);
        }
        if self.files[self.syms[source as usize].file as usize]
            .module
            .is_some()
        {
            return Err(DropReason::ReceiverUnresolved);
        }
        let rel = type_path.trim_start_matches("::");
        let hits: Vec<u32> = cands
            .iter()
            .copied()
            .filter(|&c| {
                self.syms[c as usize]
                    .container
                    .is_some_and(|k| is_path_suffix(&self.syms[k as usize].qual, rel))
            })
            .collect();
        self.pick(source, hits, Confidence::Scoped)
            .ok_or(DropReason::ReceiverUnresolved)
    }

    fn resolve_reference(
        &self,
        file: u32,
        source: u32,
        name: &str,
        line: i64,
        context: Option<&str>,
    ) -> Result<Resolution, DropReason> {
        let node = &self.files[file as usize];
        let ruby = node.family == "ruby";
        let usage = classify_usage(context, name, ruby);
        match usage {
            Usage::Namespace => return Err(DropReason::Namespace),
            Usage::Label => return Err(DropReason::Label),
            Usage::Prose => return Err(DropReason::Prose),
            _ => {}
        }
        let cands = self.candidates(name, node.family, file);
        if cands.iter().any(|&c| {
            let sym = &self.syms[c as usize];
            sym.file == file && sym.line == line
        }) {
            return Err(DropReason::Declaration);
        }
        if let Some(module) = &node.module {
            return self.resolve_in_module(file, source, name, usage, &cands, module);
        }
        if cands.is_empty() {
            // Column readers exist only at runtime: no code definition shares
            // the name, yet the model's table declares it.
            let on_instance = matches!(
                usage,
                Usage::Bare | Usage::Unseen | Usage::SelfReceiver | Usage::Symbol
            );
            return self
                .class_scope(source)
                .filter(|_| ruby && on_instance)
                .and_then(|class| self.resolve_column(class, source, name))
                .ok_or(DropReason::External);
        }
        let constant = name.chars().next().is_some_and(char::is_uppercase);
        match usage {
            Usage::Namespace | Usage::Label | Usage::Prose => unreachable!("handled above"),
            // A symbol names a method of the surrounding class (callbacks,
            // `send`, stubs); only the class and its ancestors can define it.
            Usage::Symbol => self
                .resolve_local(file, source, &cands)
                .or_else(|| self.resolve_in_class_scope(source, name))
                .ok_or(DropReason::External),
            Usage::Qualified { absolute, path } => {
                if let Some(outcome) = self.resolve_rust_path(file, source, path, name) {
                    return outcome;
                }
                let rel = if path.is_empty() {
                    name.to_string()
                } else {
                    format!("{path}::{name}")
                };
                let namespace = self.namespace_of(source);
                let family = node.family;
                let mut found = self.lexical_match(file, namespace, absolute, &rel, |c| {
                    self.family_of(c) == family
                });
                if found.is_empty() && !absolute {
                    found = self.suffix_match(&rel, &cands);
                }
                // The qualifier names a namespace no candidate lives in:
                // `Vec::new` is not the project's own `new`.
                self.pick(source, found, Confidence::Scoped)
                    .ok_or(DropReason::QualifiedUnresolved)
            }
            Usage::SelfReceiver => {
                if let Some(resolution) = self.resolve_local(file, source, &cands) {
                    return Ok(resolution);
                }
                if let Some(resolution) = self.resolve_in_class_scope(source, name) {
                    return Ok(resolution);
                }
                self.resolve_by_name(file, source, &cands, true)
            }
            Usage::TypeReceiver(type_path) => self.resolve_on_type(source, name, type_path, &cands),
            Usage::UnknownReceiver(_) => self.resolve_by_name(file, source, &cands, true),
            Usage::Bare | Usage::StringLiteral | Usage::Unseen => {
                let literal = usage == Usage::StringLiteral;
                if !literal {
                    if let Some(resolution) = self.resolve_local(file, source, &cands) {
                        return Ok(resolution);
                    }
                    if let Some(outcome) = self.resolve_rust_use(file, source, name) {
                        return outcome;
                    }
                }
                if ruby && constant {
                    // A class name in a string (`'Job'`, `class_name: 'Job'`)
                    // is how Rails names polymorphic and association types;
                    // a string that happens to spell a module is just data.
                    let family = node.family;
                    let namespace = self.namespace_of(source);
                    let found = self.lexical_match(file, namespace, false, name, |c| {
                        self.family_of(c) == family
                            && (!literal || self.syms[c as usize].kind == "class")
                    });
                    if let Some(resolution) = self.pick(source, found, Confidence::Scoped) {
                        return Ok(resolution);
                    }
                }
                if let Some(resolution) = self.resolve_in_class_scope(source, name) {
                    return Ok(resolution);
                }
                // A Ruby constant is only visible through lexical scope or
                // ancestry; one no enclosing namespace defines belongs to a
                // gem (`Rails`, `JSON`), not to an unrelated `Other::Rails`.
                if ruby && constant {
                    return Err(DropReason::External);
                }
                // A Ruby call on implicit `self` that neither the class nor its
                // resolved ancestors define comes from a gem or framework
                // (`merge`, `select`, `register`), whatever the project happens
                // to define under the same name elsewhere.
                let weak = literal || ruby || usage == Usage::Unseen;
                self.resolve_by_name(file, source, &cands, weak)
            }
        }
    }

    /// The Rust module code in `source` runs in: the inline `mod` block around
    /// it, else its file's module.
    fn rust_module_of(&self, source: u32) -> &str {
        let mut scope = Some(source);
        while let Some(current) = scope {
            let sym = &self.syms[current as usize];
            if sym.kind == "package" {
                return &sym.qual;
            }
            scope = sym.container;
        }
        &self.files[self.syms[source as usize].file as usize].namespace
    }

    /// Whether crate `krate` has a module or a definition at `path`.
    fn rust_defines(&self, krate: u32, path: &str) -> bool {
        self.rust.modules.contains(&(krate, path.to_string()))
            || self.by_qual.get(path).is_some_and(|found| {
                found.iter().any(|&c| {
                    self.files[self.syms[c as usize].file as usize].rust_crate == Some(krate)
                })
            })
    }

    /// The absolute `(crate, path)` a Rust path written in `module` of
    /// `krate` names: `crate::`, `self::` and `super::` walk the module tree,
    /// a first segment bound by a `use` of that module stands for its target,
    /// then come a child module or item of `module`, another crate of the
    /// project by its library name and the modules `module` globs. `None` for
    /// a path that leaves the project (`std::`, a dependency) or that cannot be
    /// followed.
    fn rust_absolute(
        &self,
        krate: u32,
        module: &str,
        segments: &[&str],
        depth: usize,
    ) -> Option<(u32, String)> {
        let (&first, rest) = segments.split_first()?;
        if depth > 4 {
            return None;
        }
        match first {
            "crate" => return Some((krate, join_path("", rest))),
            "self" => return Some((krate, join_path(module, rest))),
            "super" => {
                let mut base = module;
                let mut rest = segments;
                while let Some((&"super", tail)) = rest.split_first() {
                    if base.is_empty() {
                        return None;
                    }
                    base = base.rsplit_once("::").map_or("", |(head, _)| head);
                    rest = tail;
                }
                return Some((krate, join_path(base, rest)));
            }
            _ => {}
        }
        let scope = self.rust.scopes.get(&(krate, module.to_string()));
        if let Some(target) = scope.and_then(|scope| scope.bindings.get(first)) {
            let target: Vec<&str> = target.iter().map(String::as_str).collect();
            let (target_crate, base) = self.rust_absolute(krate, module, &target, depth + 1)?;
            return Some((target_crate, join_path(&base, rest)));
        }
        let local = join_path(module, &[first]);
        if self.rust_defines(krate, &local) {
            return Some((krate, join_path(&local, rest)));
        }
        if let Some(&other) = self.rust.by_name.get(first) {
            return Some((other, join_path("", rest)));
        }
        for glob in scope.map(|scope| scope.globs.as_slice()).unwrap_or(&[]) {
            let glob: Vec<&str> = glob.iter().map(String::as_str).collect();
            if let Some((glob_crate, base)) = self.rust_absolute(krate, module, &glob, depth + 1) {
                let candidate = join_path(&base, &[first]);
                if self.rust_defines(glob_crate, &candidate) {
                    return Some((glob_crate, join_path(&candidate, rest)));
                }
            }
        }
        None
    }

    /// Rust definitions at `path` of `krate` visible from `file`, following
    /// the `use` bindings and globs of the parent module when it only
    /// re-exports the name (`pub use test_paths::is_test_path;`).
    fn rust_lookup(&self, file: u32, krate: u32, path: &str, depth: usize) -> Vec<u32> {
        let hits: Vec<u32> = self
            .by_qual
            .get(path)
            .map(|found| {
                found
                    .iter()
                    .copied()
                    .filter(|&c| {
                        self.files[self.syms[c as usize].file as usize].rust_crate == Some(krate)
                            && self.visible_from(file, c)
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !hits.is_empty() || depth > 4 {
            return hits;
        }
        let (parent, last) = path.rsplit_once("::").unwrap_or(("", path));
        let Some(scope) = self.rust.scopes.get(&(krate, parent.to_string())) else {
            return Vec::new();
        };
        if let Some(target) = scope.bindings.get(last) {
            let target: Vec<&str> = target.iter().map(String::as_str).collect();
            return match self.rust_absolute(krate, parent, &target, 0) {
                Some((target_crate, target)) => {
                    self.rust_lookup(file, target_crate, &target, depth + 1)
                }
                None => Vec::new(),
            };
        }
        for glob in &scope.globs {
            let glob: Vec<&str> = glob.iter().map(String::as_str).collect();
            if let Some((glob_crate, base)) = self.rust_absolute(krate, parent, &glob, 0) {
                let found =
                    self.rust_lookup(file, glob_crate, &join_path(&base, &[last]), depth + 1);
                if !found.is_empty() {
                    return found;
                }
            }
        }
        Vec::new()
    }

    /// A Rust path `path::name` resolved the way the compiler does it.
    /// `None` leaves the reference to the name-based rules: the path may name
    /// something the index does not hold (an enum variant, a macro) or hold
    /// under another namespace (an `impl` in another module than its type).
    fn resolve_rust_path(
        &self,
        file: u32,
        source: u32,
        path: &str,
        name: &str,
    ) -> Option<Result<Resolution, DropReason>> {
        let krate = self.files[file as usize].rust_crate?;
        let module = self.rust_module_of(source);
        let mut segments: Vec<&str> = path.split("::").filter(|s| !s.is_empty()).collect();
        segments.push(name);
        match self.rust_absolute(krate, module, &segments, 0) {
            Some((target_crate, absolute)) => {
                let hits = self.rust_lookup(file, target_crate, &absolute, 0);
                self.pick(source, hits, Confidence::Scoped).map(Ok)
            }
            // `std::mem::take`, or `HashMap::new` under `use std::collections::HashMap`:
            // no definition of the project, whatever shares the last name.
            None if is_external_rust_root(segments[0])
                || self
                    .rust
                    .scopes
                    .get(&(krate, module.to_string()))
                    .is_some_and(|scope| scope.bindings.contains_key(segments[0])) =>
            {
                Some(Err(DropReason::QualifiedUnresolved))
            }
            None => None,
        }
    }

    /// A bare Rust name that a `use` of the module around `source` binds,
    /// directly or through a glob. A name bound to a path outside the project
    /// is no reference to the project's definition of that name.
    fn resolve_rust_use(
        &self,
        file: u32,
        source: u32,
        name: &str,
    ) -> Option<Result<Resolution, DropReason>> {
        let krate = self.files[file as usize].rust_crate?;
        let module = self.rust_module_of(source);
        let scope = self.rust.scopes.get(&(krate, module.to_string()))?;
        if let Some(target) = scope.bindings.get(name) {
            let target: Vec<&str> = target.iter().map(String::as_str).collect();
            return match self.rust_absolute(krate, module, &target, 0) {
                Some((target_crate, absolute)) => {
                    let hits = self.rust_lookup(file, target_crate, &absolute, 0);
                    self.pick(source, hits, Confidence::Import).map(Ok)
                }
                None => Some(Err(DropReason::External)),
            };
        }
        for glob in &scope.globs {
            let glob: Vec<&str> = glob.iter().map(String::as_str).collect();
            if let Some((glob_crate, base)) = self.rust_absolute(krate, module, &glob, 0) {
                let hits = self.rust_lookup(file, glob_crate, &join_path(&base, &[name]), 0);
                if let Some(resolution) = self.pick(source, hits, Confidence::Import) {
                    return Some(Ok(resolution));
                }
            }
        }
        None
    }

    /// Module-scoped languages (JavaScript / TypeScript): a name is either
    /// defined in the file or bound by one of its imports; nothing else can
    /// make it point at another file.
    fn resolve_in_module(
        &self,
        file: u32,
        source: u32,
        name: &str,
        usage: Usage<'_>,
        cands: &[u32],
        module: &ModuleImports,
    ) -> Result<Resolution, DropReason> {
        let through_namespace = |namespace: &str| -> Option<Result<Resolution, DropReason>> {
            let target = *module.namespaces.get(namespace)?;
            let found = self.in_import_target(file, target, name, false);
            Some(
                self.pick(source, found, Confidence::Import)
                    .ok_or(DropReason::External),
            )
        };
        match usage {
            Usage::Bare | Usage::Qualified { .. } | Usage::Unseen => {
                if let Some(resolution) = self.resolve_local(file, source, cands) {
                    return Ok(resolution);
                }
                let Some((exported, target)) = module.bindings.get(name) else {
                    return Err(DropReason::External);
                };
                let found = self.in_import_target(file, *target, exported, false);
                self.pick(source, found, Confidence::Import)
                    .ok_or(DropReason::External)
            }
            Usage::SelfReceiver => {
                if let Some(resolution) = self.resolve_local(file, source, cands) {
                    return Ok(resolution);
                }
                self.resolve_in_class_scope(source, name)
                    .ok_or(DropReason::ReceiverUnresolved)
            }
            Usage::TypeReceiver(type_path) => {
                if let Some(result) = through_namespace(type_path) {
                    return result;
                }
                self.resolve_on_type(source, name, type_path, cands)
            }
            Usage::UnknownReceiver(token) => {
                through_namespace(token).unwrap_or(Err(DropReason::ReceiverUnresolved))
            }
            Usage::StringLiteral | Usage::Symbol => Err(DropReason::External),
            Usage::Namespace | Usage::Label | Usage::Prose => {
                unreachable!("handled by the caller")
            }
        }
    }

    /// Same rule as [`db::find_owning_symbol`] — narrowest enclosing range,
    /// later start on ties, last-declared fallback only for files without any
    /// range, import and annotation lines skipped in favour of the definition
    /// around them.
    fn owner(&self, file: &FileNode, line: i64) -> Option<u32> {
        let mut best: Option<(i64, i64, u32)> = None;
        for &s in &file.symbols {
            let sym = &self.syms[s as usize];
            if sym.line > line {
                break;
            }
            if sym.end < line || !is_node_kind(&sym.kind) {
                continue;
            }
            let span = sym.end - sym.line;
            let better = match best {
                None => true,
                Some((best_span, best_line, _)) => {
                    span < best_span || (span == best_span && sym.line > best_line)
                }
            };
            if better {
                best = Some((span, sym.line, s));
            }
        }
        if let Some((_, _, owner)) = best {
            return Some(owner);
        }
        if file.has_ranges {
            return None;
        }
        file.symbols.iter().rev().copied().find(|&s| {
            let sym = &self.syms[s as usize];
            sym.line <= line && is_node_kind(&sym.kind)
        })
    }
}

/// The type a block reopens: a Rust `impl Type` / `impl Trait for Type`, a
/// Swift `Type+Extension`, an Objective-C `Type+Category`. Its members live in
/// that type's namespace, and the block name is no namespace of its own — nor,
/// despite the space in `impl Type`, a file-private DSL block.
fn reopened_type(name: &str) -> Option<&str> {
    let target = match name.strip_prefix("impl ") {
        Some(rest) => rest.rsplit_once(" for ").map_or(rest, |(_, target)| target),
        None => name
            .strip_suffix("+Extension")
            .or_else(|| name.strip_suffix("+Category"))?,
    };
    let path = target.split('<').next().unwrap_or(target);
    let last = path.rsplit("::").next().unwrap_or(path);
    let ident = last
        .rsplit(|c: char| c.is_whitespace() || c == '&' || c == '*')
        .next()
        .unwrap_or(last);
    let valid = ident
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
        && ident.chars().all(|c| c.is_alphanumeric() || c == '_');
    valid.then_some(ident)
}

/// File stem Zeitwerk expects for a constant path: `Billing::HTTPClient` ->
/// `billing/http_client`.
fn autoload_path(qual: &str) -> String {
    let mut out = String::with_capacity(qual.len() + 8);
    for (index, segment) in qual.split("::").enumerate() {
        if index > 0 {
            out.push('/');
        }
        let chars: Vec<char> = segment.chars().collect();
        for (position, &character) in chars.iter().enumerate() {
            if character.is_uppercase() && position > 0 {
                let previous = chars[position - 1];
                let next_lower = chars.get(position + 1).is_some_and(|c| c.is_lowercase());
                if previous.is_lowercase()
                    || previous.is_ascii_digit()
                    || (previous.is_uppercase() && next_lower)
                {
                    out.push('_');
                }
            }
            out.extend(character.to_lowercase());
        }
    }
    out
}

/// Number of leading `::` segments two namespace paths share.
fn common_namespace_depth(left: &str, right: &str) -> usize {
    left.split("::")
        .zip(right.split("::"))
        .take_while(|(a, b)| a == b && !a.is_empty())
        .count()
}

#[derive(Clone, Copy)]
struct EdgeAcc {
    confidence: Confidence,
    candidates: u32,
    refs: u32,
    line: i64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ConfidenceCount {
    pub confidence: String,
    pub edges: u64,
    pub references: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DropCount {
    pub reason: String,
    pub references: u64,
}

/// What `graph build` did, persisted as JSON in the metadata table.
#[derive(Clone, Debug, Default, Serialize)]
pub struct GraphBuildSummary {
    pub nodes: u64,
    pub edges: u64,
    pub resolved_edges: u64,
    pub references_seen: u64,
    pub references_linked: u64,
    pub by_confidence: Vec<ConfidenceCount>,
    pub dropped: Vec<DropCount>,
    pub ambiguity_cap: usize,
    pub dependents_depth: usize,
    pub elapsed_ms: u128,
    /// Rails `db/schema.rb` tables matched to models; absent without tables.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<SchemaSummary>,
}

/// Schema linking plus how much of the graph it resolved.
#[derive(Clone, Debug, Default, Serialize, serde::Deserialize)]
pub struct SchemaSummary {
    #[serde(flatten)]
    pub link: SchemaLinkSummary,
    pub column_edges: u64,
    pub column_references: u64,
}

/// References of one file: `(name, line, context)` rows.
type FileRefs = (i64, Vec<(String, i64, Option<String>)>);

/// References held in memory at once while files resolve in parallel.
const RESOLVE_GROUP_REFS: usize = 200_000;

/// What resolving one file's references contributes to the graph.
#[derive(Default)]
struct FileResolution {
    edges: HashMap<(u32, u32), EdgeAcc>,
    refs_by_level: HashMap<Confidence, u64>,
    dropped: HashMap<DropReason, u64>,
    references_seen: u64,
    column_references: u64,
}

fn resolve_file(
    builder: &Builder,
    file_id: i64,
    mut batch: Vec<(String, i64, Option<String>)>,
) -> FileResolution {
    let mut out = FileResolution::default();
    let Some(&file) = builder.file_index.get(&file_id) else {
        return out;
    };
    let file_node = &builder.files[file as usize];
    if file_node.vendor {
        return out;
    }
    batch.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
    batch.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    for (name, line, context) in batch {
        out.references_seen += 1;
        let Some(source) = builder.owner(file_node, line) else {
            *out.dropped.entry(DropReason::NoOwner).or_default() += 1;
            continue;
        };
        let outcome = builder
            .resolve_reference(file, source, &name, line, context.as_deref())
            .and_then(|mut resolution| {
                resolution.targets.retain(|&t| t != source);
                if resolution.targets.is_empty() {
                    Err(DropReason::SelfReference)
                } else if !resolution.confidence.is_resolved()
                    && resolution.targets.len() > AMBIGUITY_CAP
                {
                    Err(DropReason::TooAmbiguous)
                } else {
                    Ok(resolution)
                }
            });
        match outcome {
            Err(reason) => *out.dropped.entry(reason).or_default() += 1,
            Ok(resolution) => {
                *out.refs_by_level.entry(resolution.confidence).or_default() += 1;
                if resolution.confidence.is_resolved()
                    && builder.syms[resolution.targets[0] as usize].kind == "column"
                {
                    out.column_references += 1;
                }
                let candidates = if resolution.confidence.is_resolved() {
                    1
                } else {
                    resolution.targets.len() as u32
                };
                for target in resolution.targets {
                    let entry = out.edges.entry((source, target)).or_insert(EdgeAcc {
                        confidence: resolution.confidence,
                        candidates,
                        refs: 0,
                        line,
                    });
                    if resolution.confidence < entry.confidence {
                        entry.confidence = resolution.confidence;
                        entry.candidates = candidates;
                    }
                    entry.refs += 1;
                    entry.line = entry.line.min(line);
                }
            }
        }
    }
    // The memo only serves the file that is resolving.
    builder.import_memo[file as usize]
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    out
}

/// Build (or rebuild) the stored symbol graph from the current index.
pub fn build_symbol_graph(
    conn: &mut Connection,
    root: &Path,
    verbose: bool,
) -> Result<GraphBuildSummary> {
    let started = Instant::now();
    let fingerprint = db::index_fingerprint(conn)?;
    let builder = Builder::load(conn, root)?;
    if verbose {
        eprintln!(
            "graph: loaded {} files, {} symbols, {} names in {:?}",
            builder.files.len(),
            builder.syms.len(),
            builder.by_short.len(),
            started.elapsed()
        );
    }

    let mut edges: HashMap<(u32, u32), EdgeAcc> = HashMap::new();
    let mut refs_by_level: HashMap<Confidence, u64> = HashMap::new();
    let mut dropped: HashMap<DropReason, u64> = HashMap::new();
    let mut references_seen = 0u64;
    let mut column_references = 0u64;

    // Files resolve independently, so each is resolved on its own thread and
    // the results are folded in file order. Folding an edge keeps the lowest
    // confidence with the candidate count of the first reference that
    // reached it, exactly as one pass over the references in order does.
    let mut merge = |resolved: FileResolution| {
        references_seen += resolved.references_seen;
        column_references += resolved.column_references;
        for (level, count) in resolved.refs_by_level {
            *refs_by_level.entry(level).or_default() += count;
        }
        for (reason, count) in resolved.dropped {
            *dropped.entry(reason).or_default() += count;
        }
        for (key, acc) in resolved.edges {
            match edges.entry(key) {
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(acc);
                }
                std::collections::hash_map::Entry::Occupied(mut slot) => {
                    let entry = slot.get_mut();
                    if acc.confidence < entry.confidence {
                        entry.confidence = acc.confidence;
                        entry.candidates = acc.candidates;
                    }
                    entry.refs += acc.refs;
                    entry.line = entry.line.min(acc.line);
                }
            }
        }
    };
    let resolve_group = |group: Vec<FileRefs>| -> Vec<FileResolution> {
        group
            .into_par_iter()
            .map(|(file_id, batch)| resolve_file(&builder, file_id, batch))
            .collect()
    };

    let mut group: Vec<FileRefs> = Vec::new();
    let mut group_refs = 0usize;
    db::for_each_graph_ref(conn, |file_id, name, line, context| {
        if group.last().map(|(id, _)| *id) != Some(file_id) {
            if group_refs >= RESOLVE_GROUP_REFS {
                for resolved in resolve_group(std::mem::take(&mut group)) {
                    merge(resolved);
                }
                group_refs = 0;
            }
            group.push((file_id, Vec::new()));
        }
        if let Some((_, batch)) = group.last_mut() {
            batch.push((name.to_string(), line, context.map(str::to_string)));
        }
        group_refs += 1;
        Ok(())
    })?;
    for resolved in resolve_group(group) {
        merge(resolved);
    }

    if verbose {
        eprintln!(
            "graph: resolved {} edges from {} references in {:?}",
            edges.len(),
            references_seen,
            started.elapsed()
        );
    }

    let mut rows: Vec<SymbolEdgeRow> = edges
        .iter()
        .map(|(&(source, target), acc)| SymbolEdgeRow {
            source_id: builder.syms[source as usize].id,
            target_id: builder.syms[target as usize].id,
            confidence: acc.confidence.code(),
            candidates: acc.candidates,
            ref_count: acc.refs,
            line: acc.line,
        })
        .collect();
    rows.sort_by_key(|row| (row.source_id, row.target_id));

    let file_of: HashMap<i64, u32> = builder.syms.iter().map(|sym| (sym.id, sym.file)).collect();
    let metrics = compute_metrics(&rows, &file_of, DEPENDENTS_DEPTH);
    if verbose {
        eprintln!(
            "graph: computed metrics for {} nodes in {:?}",
            metrics.len(),
            started.elapsed()
        );
    }

    let column_ids: HashSet<i64> = builder
        .syms
        .iter()
        .filter(|sym| sym.kind == "column")
        .map(|sym| sym.id)
        .collect();
    let mut edges_by_level: HashMap<Confidence, u64> = HashMap::new();
    for row in &rows {
        *edges_by_level
            .entry(Confidence::from_code(row.confidence))
            .or_default() += 1;
    }
    let mut summary = GraphBuildSummary {
        nodes: metrics.len() as u64,
        edges: rows.len() as u64,
        resolved_edges: rows
            .iter()
            .filter(|row| Confidence::from_code(row.confidence).is_resolved())
            .count() as u64,
        references_seen,
        references_linked: refs_by_level.values().sum(),
        by_confidence: Confidence::ALL
            .iter()
            .map(|level| ConfidenceCount {
                confidence: level.as_str().to_string(),
                edges: edges_by_level.get(level).copied().unwrap_or(0),
                references: refs_by_level.get(level).copied().unwrap_or(0),
            })
            .collect(),
        dropped: {
            let mut entries: Vec<(DropReason, u64)> = dropped.into_iter().collect();
            entries.sort();
            entries
                .into_iter()
                .map(|(reason, count)| DropCount {
                    reason: reason.as_str().to_string(),
                    references: count,
                })
                .collect()
        },
        ambiguity_cap: AMBIGUITY_CAP,
        dependents_depth: DEPENDENTS_DEPTH,
        elapsed_ms: 0,
        schema: builder.schema.clone().map(|link| SchemaSummary {
            link,
            column_edges: rows
                .iter()
                .filter(|row| {
                    Confidence::from_code(row.confidence).is_resolved()
                        && column_ids.contains(&row.target_id)
                })
                .count() as u64,
            column_references,
        }),
    };
    summary.elapsed_ms = started.elapsed().as_millis();
    let summary_json = serde_json::to_string(&summary)?;
    db::store_symbol_graph(conn, &rows, &metrics, &fingerprint, &summary_json)?;
    summary.elapsed_ms = started.elapsed().as_millis();
    if verbose {
        eprintln!("graph: stored in {:?}", started.elapsed());
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_usage_reads_qualifiers_and_receivers() {
        let ruby = |line: &'static str, name: &str| classify_usage(Some(line), name, true);
        assert_eq!(
            ruby("::Billing::Models::Invoice.find(1)", "Invoice"),
            Usage::Qualified {
                absolute: true,
                path: "Billing::Models"
            }
        );
        assert_eq!(
            ruby("Billing::Invoice.find(1)", "Billing"),
            Usage::Namespace
        );
        assert_eq!(
            ruby("Billing::CreateService.call(params)", "call"),
            Usage::TypeReceiver("Billing::CreateService")
        );
        assert_eq!(
            ruby("user.update_profile(attrs)", "update_profile"),
            Usage::UnknownReceiver("user")
        );
        assert_eq!(ruby("self.refresh(now)", "refresh"), Usage::SelfReceiver);
        assert_eq!(
            ruby("  .where(active: true)", "where"),
            Usage::UnknownReceiver("")
        );
        assert_eq!(ruby("render(Invoice)", "Invoice"), Usage::Bare);
        assert_eq!(
            classify_usage(Some("[...items].map(f)"), "items", false),
            Usage::Bare
        );
    }

    #[test]
    fn classify_usage_separates_code_from_data() {
        let ruby = |line: &'static str, name: &str| classify_usage(Some(line), name, true);
        assert_eq!(
            ruby("required(:company).filled(type?: Company)", "type?"),
            Usage::Label
        );
        assert_eq!(
            ruby("item_type: 'Invoice',", "Invoice"),
            Usage::StringLiteral
        );
        assert_eq!(
            ruby("it 'creates the invoice (twice)' do", "invoice"),
            Usage::Prose
        );
        assert_eq!(
            ruby("total = sum(x) # see Invoice", "Invoice"),
            Usage::Prose
        );
        assert_eq!(
            ruby("log(\"#{Invoice.count} rows\")", "Invoice"),
            Usage::Bare
        );
        assert_eq!(
            ruby("type: 'Billing::Invoice'", "Billing"),
            Usage::Namespace
        );
        assert_eq!(
            ruby("allow(File).to receive(:exist?).and_return(true)", "exist?"),
            Usage::Symbol
        );
        assert_eq!(
            ruby("scope.except(:select).select('id')", "select"),
            Usage::UnknownReceiver("")
        );
        assert_eq!(ruby("  Invoice", "Payment"), Usage::Unseen);
        assert_eq!(
            ruby("before_save :normalize, if: :paid?", "paid?"),
            Usage::Symbol
        );
        assert_eq!(
            ruby("rows.map(&:total).sum", "total"),
            Usage::UnknownReceiver("")
        );
        assert_eq!(
            ruby("delegate :name, :email, to: :owner", "email"),
            Usage::UnknownReceiver("")
        );
        assert_eq!(
            ruby("delegate :name, :email, to: :owner", "owner"),
            Usage::Symbol
        );
        assert_eq!(
            classify_usage(Some("case KIND_INVOICE:"), "KIND_INVOICE", false),
            Usage::Bare
        );
    }

    #[test]
    fn js_imports_bind_local_names_to_modules() {
        let source = "import React, { useState } from 'react';\n\
                      import {\n  Button,\n  Icon as Glyph,\n} from 'shared/components';\n\
                      import type { Invoice } from './types';\n\
                      import * as api from '../api';\n\
                      const { fetchAll } = require('./legacy');\n";
        let mut targets = Vec::new();
        let imports = parse_module_imports("src/billing/List.tsx", source, &mut targets);
        let (exported, target) = &imports.bindings["Glyph"];
        assert_eq!(exported, "Icon");
        assert!(targets[*target].matches("frontend/shared/components/Icon/Icon"));
        let (_, invoice) = &imports.bindings["Invoice"];
        assert!(targets[*invoice].matches("src/billing/types"));
        assert!(targets[imports.namespaces["api"]].matches("src/api"));
        assert!(imports.bindings.contains_key("React"));
        assert!(imports.bindings.contains_key("fetchAll"));
    }

    #[test]
    fn autoload_path_follows_zeitwerk_inflection() {
        assert_eq!(autoload_path("Billing::HTTPClient"), "billing/http_client");
        assert_eq!(
            autoload_path("AjsJoin::CreateService"),
            "ajs_join/create_service"
        );
        assert_eq!(autoload_path("Api::V3::Jobs"), "api/v3/jobs");
    }

    #[test]
    fn import_targets_resolve_relative_and_directory_specifiers() {
        let target =
            ImportTarget::parse("src/features/list/List.tsx", "./components/Item").unwrap();
        assert!(target.matches("src/features/list/components/Item"));
        assert!(target.matches("src/features/list/components/Item/index"));
        assert!(!target.matches("src/features/list/components/ItemRow"));
        let up = ImportTarget::parse("spec/services/a_spec.rb", "../../app/services/base").unwrap();
        assert!(up.matches("app/services/base"));
        let bare = ImportTarget::parse("lib/tasks/x.rb", "billing/invoice").unwrap();
        assert!(bare.matches("lib/billing/invoice"));
        assert!(!bare.matches("lib/xbilling/invoice"));
    }

    #[test]
    fn reopened_type_names_the_type_a_block_extends() {
        assert_eq!(reopened_type("impl Point"), Some("Point"));
        assert_eq!(
            reopened_type("impl std::fmt::Display for geo::Point<T>"),
            Some("Point")
        );
        assert_eq!(
            reopened_type("impl Iterator for &'a mut Walker"),
            Some("Walker")
        );
        assert_eq!(reopened_type("Greeter+Extension"), Some("Greeter"));
        assert_eq!(reopened_type("NSString+Category"), Some("NSString"));
        assert_eq!(reopened_type("impl [u8]"), None);
        assert_eq!(reopened_type("Point"), None);
        assert_eq!(reopened_type("describe \"impl Point\""), None);
    }

    #[test]
    fn rust_impl_members_live_in_their_type_namespace() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_db(&conn).unwrap();
        for (id, path) in [(1, "src/point.rs"), (2, "src/main.rs")] {
            conn.execute(
                "INSERT INTO files (id, path, root_path, mtime, size) VALUES (?1, ?2, '', 0, 0)",
                rusqlite::params![id, path],
            )
            .unwrap();
        }
        let symbols: [(i64, &str, &str, i64, i64); 5] = [
            (1, "Point", "class", 1, 3),
            (1, "impl Point", "class", 5, 13),
            (1, "new", "function", 6, 8),
            (1, "norm", "function", 10, 12),
            (2, "main", "function", 1, 3),
        ];
        for (file, name, kind, line, end_line) in symbols {
            conn.execute(
                "INSERT INTO symbols (file_id, name, kind, line, end_line) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![file, name, kind, line, end_line],
            )
            .unwrap();
        }
        let builder = Builder::load(&conn, Path::new("/nonexistent")).unwrap();
        let find = |name: &str| {
            builder
                .syms
                .iter()
                .position(|sym| sym.name == name)
                .unwrap() as u32
        };
        let new = find("new");
        assert_eq!(builder.syms[new as usize].qual, "point::Point::new");
        assert!(!builder.syms[new as usize].file_private);
        assert_eq!(builder.syms[find("main") as usize].qual, "main");

        let main = find("main");
        let main_file = builder.syms[main as usize].file;
        let resolution = builder
            .resolve_reference(
                main_file,
                main,
                "new",
                2,
                Some("    let p = Point::new(1);"),
            )
            .unwrap();
        assert_eq!(resolution.confidence, Confidence::Scoped);
        assert_eq!(resolution.targets, vec![new]);

        let norm = find("norm");
        let point_file = builder.syms[norm as usize].file;
        let resolution = builder
            .resolve_reference(point_file, norm, "new", 11, Some("        Self::new(0)"))
            .unwrap();
        assert_eq!(resolution.targets, vec![new]);
    }

    #[test]
    fn path_helpers_respect_segment_boundaries() {
        assert!(is_path_suffix("A::B::C", "B::C"));
        assert!(!is_path_suffix("A::XB::C", "B::C"));
        assert_eq!(common_namespace_depth("A::B::C", "A::B::D"), 2);
        assert_eq!(common_namespace_depth("", "A"), 0);
    }

    fn owner_fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init_db(&conn).unwrap();
        // File 4 shares file 1's relative path under an attached root.
        let files = [
            (1, "app/ranged.rb", ""),
            (2, "lib/rangeless.kt", ""),
            (3, "app/ties.rb", ""),
            (4, "app/ranged.rb", "/elsewhere/shared"),
        ];
        for (id, path, root_path) in files {
            conn.execute(
                "INSERT INTO files (id, path, root_path, mtime, size) VALUES (?1, ?2, ?3, 0, 0)",
                rusqlite::params![id, path, root_path],
            )
            .unwrap();
        }
        let symbols: [(i64, &str, &str, i64, Option<i64>); 17] = [
            (1, "json", "import", 0, Some(0)),
            (1, "Outer", "class", 1, Some(30)),
            (1, "first", "function", 3, Some(9)),
            (1, "include(name: 1)", "annotation", 5, Some(7)),
            (1, "Inner", "class", 11, Some(25)),
            (1, "include Helpers", "annotation", 12, Some(12)),
            (1, "second", "function", 13, Some(18)),
            (1, "LIMIT", "constant", 20, Some(20)),
            (2, "Widget", "class", 1, None),
            (2, "render", "function", 5, None),
            (2, "Printable", "annotation", 11, None),
            (2, "paint", "function", 12, None),
            (3, "Twins", "class", 1, Some(12)),
            (3, "left", "function", 3, Some(6)),
            (3, "right", "function", 3, Some(6)),
            (4, "Shadow", "class", 1, Some(35)),
            (4, "shadowed", "function", 10, Some(12)),
        ];
        for (file, name, kind, line, end_line) in symbols {
            conn.execute(
                "INSERT INTO symbols (file_id, name, kind, line, end_line) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![file, name, kind, line, end_line],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn owner_matches_find_owning_symbol_line_for_line() {
        let conn = owner_fixture();
        let builder = Builder::load(&conn, Path::new("/nonexistent")).unwrap();
        for row in db::load_graph_files(&conn).unwrap() {
            let file = &builder.files[builder.file_index[&row.id] as usize];
            for line in 0..=35 {
                let ours = builder
                    .owner(file, line)
                    .map(|s| builder.syms[s as usize].name.clone());
                let expected =
                    db::find_owning_symbol(&conn, Some(row.root_path.as_str()), &row.path, line)
                        .unwrap()
                        .map(|symbol| symbol.name);
                if file.path == "app/ties.rb" && (3..=6).contains(&line) {
                    // Equal ranges: SQLite breaks the tie arbitrarily, the
                    // builder deterministically takes the first definition.
                    assert!(matches!(ours.as_deref(), Some("left" | "right")));
                    continue;
                }
                assert_eq!(ours, expected, "{}{}:{line}", row.root_path, file.path);
            }
        }
    }

    #[test]
    fn owner_skips_import_and_annotation_lines_for_the_enclosing_definition() {
        let conn = owner_fixture();
        let builder = Builder::load(&conn, Path::new("/nonexistent")).unwrap();
        let file = |path: &str| {
            builder
                .files
                .iter()
                .find(|file| file.path == path && file.symbols.len() > 2)
                .unwrap()
        };
        let owner = |path: &str, line: i64| {
            builder
                .owner(file(path), line)
                .map(|s| builder.syms[s as usize].name.as_str())
        };
        assert_eq!(owner("app/ranged.rb", 0), None);
        assert_eq!(owner("app/ranged.rb", 6), Some("first"));
        assert_eq!(owner("app/ranged.rb", 12), Some("Inner"));
        assert_eq!(owner("lib/rangeless.kt", 11), Some("render"));
    }
}
