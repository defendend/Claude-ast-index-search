//! Index-based search commands
//!
//! Commands for searching through the code index:
//! - search: Full-text search across files and symbols
//! - symbol: Find symbol by name
//! - class: Find class by name
//! - implementations: Find implementations of interface/class
//! - hierarchy: Show class hierarchy
//! - usages: Find symbol usages (indexed or grep-based)

use std::path::Path;

use anyhow::Result;
use colored::Colorize;
use regex::Regex;

use super::{
    print_truncation_notice, relative_path, search_files_page, Page, Pagination, PathResolver,
    PAGINATED_JSON_SCHEMA_VERSION,
};
use crate::db::{self, SearchScope};

fn symbol_display_name(symbol: &db::SearchResult) -> &str {
    symbol.display_name()
}

fn auto_pattern_from_name<'a>(
    name: Option<&'a str>,
    pattern: Option<&'a str>,
) -> (Option<&'a str>, Option<&'a str>) {
    if pattern.is_some() {
        return (name, pattern);
    }

    match name {
        Some(n) if n.contains('*') || n.contains('?') => (None, Some(n)),
        _ => (name, pattern),
    }
}

/// Full-text search across files, symbols, and file contents
pub fn cmd_search(
    root: &Path,
    query: &str,
    kind_filter: Option<&str>,
    limit: usize,
    format: &str,
    scope: &SearchScope,
    fuzzy: bool,
) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db_leased(root)?;

    // Split query by comma for OR semantics: "email,mail" searches both terms
    let terms: Vec<&str> = query
        .split(',')
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect();
    // Collect results from all terms, deduplicating
    let mut content_matches: Vec<(String, usize, String)> = vec![];

    let mut seen_content = std::collections::HashSet::new();
    let files_total = db::count_files_with_roots_terms_scoped(&conn, &terms, scope)?;
    let symbols_total =
        db::count_search_symbol_terms_scoped(&conn, &terms, kind_filter, scope, fuzzy)?;
    let refs_total = db::count_search_ref_terms_scoped(&conn, &terms, scope)?;

    let probe_limit = limit.saturating_add(1);
    // One OR query per indexed category fills the page with unique rows while
    // keeping memory bounded to the requested page plus one probe row.
    let files = db::find_files_with_roots_terms_scoped(&conn, &terms, probe_limit, scope)?;
    let mut symbols =
        db::search_symbol_terms_scoped(&conn, &terms, kind_filter, probe_limit, scope, fuzzy)?;
    let ref_matches = db::search_ref_terms_scoped(&conn, &terms, probe_limit, scope)?;

    // 4. Search in file contents (grep)
    let pattern = if terms.len() > 1 {
        terms
            .iter()
            .map(|t| regex::escape(t))
            .collect::<Vec<_>>()
            .join("|")
    } else {
        regex::escape(query)
    };

    let content_page = search_files_page(
        root,
        &pattern,
        &super::grep::ALL_SOURCE_EXTENSIONS,
        limit,
        |path, line_num, line| {
            let rel_path = super::relative_path(root, path);
            // Apply scope filter for grep results
            if let Some(prefix) = scope.dir_prefix {
                if !rel_path.starts_with(prefix) {
                    return None;
                }
            }
            if let Some(in_file) = scope.in_file {
                if !rel_path.contains(in_file) {
                    return None;
                }
            }
            if let Some(module) = scope.module {
                if !rel_path.starts_with(module) {
                    return None;
                }
            }
            let content: String = line.trim().chars().take(100).collect();
            let key = format!("{}:{}", rel_path, line_num);
            if seen_content.insert(key) {
                Some((rel_path, line_num, content))
            } else {
                None
            }
        },
    )?;
    content_matches = content_page.items;

    let resolver = PathResolver::try_from_conn(root, &conn)?;
    // Apply --subtree / --local filters before resolving paths so we don't
    // do extra work on rows the user will throw away.
    let files: Vec<String> = files
        .into_iter()
        .filter(|f| resolver.matches_filter(f.root_path.as_deref()))
        .map(|file| resolver.resolve_with_root(&file.path, file.root_path.as_deref()))
        .collect();
    symbols.retain(|s| resolver.matches_filter(s.root_path.as_deref()));
    for s in &mut symbols {
        s.path = resolver.resolve_with_root(&s.path, s.root_path.as_deref());
    }
    for m in &mut content_matches {
        m.0 = resolver.resolve(&m.0);
    }

    let files_page = Page::new(files, files_total, limit);
    let symbols_page = Page::new(symbols, symbols_total, limit);
    let refs_page = Page::new(ref_matches, refs_total, limit);
    let content_pagination =
        Pagination::new(content_page.pagination.total, content_matches.len(), limit);

    if format == "json" {
        let result = serde_json::json!({
            "schema_version": PAGINATED_JSON_SCHEMA_VERSION,
            "files": files_page.items,
            "symbols": symbols_page.items,
            "references": refs_page.items.iter().map(|(name, count)| {
                serde_json::json!({"name": name, "usage_count": count})
            }).collect::<Vec<_>>(),
            "content_matches": content_matches.iter().map(|(p, l, c)| {
                serde_json::json!({"path": p, "line": l, "content": c})
            }).collect::<Vec<_>>(),
            "pagination": {
                "files": files_page.pagination,
                "symbols": symbols_page.pagination,
                "references": refs_page.pagination,
                "content_matches": content_pagination,
            }
        });
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    // Output results
    println!("{}", format!("Search results for '{}':", query).bold());

    if !files_page.items.is_empty() {
        println!(
            "\n{}",
            format!(
                "Files by path (showing {} of {}):",
                files_page.pagination.returned, files_page.pagination.total
            )
            .cyan()
        );
        for path in &files_page.items {
            println!("  {}", path);
        }
        print_truncation_notice(files_page.pagination);
    }

    if !symbols_page.items.is_empty() {
        println!(
            "\n{}",
            format!(
                "Symbols (showing {} of {}):",
                symbols_page.pagination.returned, symbols_page.pagination.total
            )
            .cyan()
        );
        for s in &symbols_page.items {
            println!(
                "  {} [{}]: {}:{}",
                symbol_display_name(s).cyan(),
                s.kind,
                s.path,
                s.line
            );
        }
        print_truncation_notice(symbols_page.pagination);
    }

    if !refs_page.items.is_empty() {
        println!(
            "\n{}",
            format!(
                "References (showing {} of {}):",
                refs_page.pagination.returned, refs_page.pagination.total
            )
            .cyan()
        );
        for (name, count) in &refs_page.items {
            println!("  {} — used in {} places", name.cyan(), count);
        }
        print_truncation_notice(refs_page.pagination);
    }

    if !content_matches.is_empty() {
        println!(
            "\n{}",
            format!(
                "Content matches (showing {} of {}):",
                content_pagination.returned, content_pagination.total
            )
            .cyan()
        );
        for (path, line_num, content) in &content_matches {
            println!("  {}:{}", path.cyan(), line_num);
            println!("    {}", content.dimmed());
        }
        print_truncation_notice(content_pagination);
    }

    if files_page.items.is_empty()
        && symbols_page.items.is_empty()
        && refs_page.items.is_empty()
        && content_matches.is_empty()
    {
        println!("  No results found.");
    }

    Ok(())
}

/// Find symbol by name or glob pattern
pub fn cmd_symbol(
    root: &Path,
    name: Option<&str>,
    pattern: Option<&str>,
    kind: Option<&str>,
    limit: usize,
    format: &str,
    scope: &SearchScope,
    fuzzy: bool,
) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let (name, pattern) = auto_pattern_from_name(name, pattern);

    if name.is_none() && pattern.is_none() {
        println!("{}", "Either a symbol name or --pattern is required.".red());
        return Ok(());
    }

    let conn = db::open_db_leased(root)?;
    let (mut symbols, total) = if let Some(pat) = pattern {
        let like_pattern = db::glob_to_like(pat);
        let total = db::count_symbols_by_pattern_scoped(&conn, &like_pattern, kind, scope, false)?;
        (
            db::find_symbols_by_pattern(&conn, &like_pattern, kind, limit, scope)?,
            total,
        )
    } else {
        let name = name.unwrap();
        if fuzzy && kind.is_none() {
            let total = db::count_symbols_fuzzy_scoped(&conn, name, None, scope, false)?;
            let matches =
                db::search_symbols_for_command(&conn, name, None, limit, scope, true, false)?;
            (matches, total)
        } else {
            let total = db::count_symbols_by_name_scoped(&conn, name, kind, scope, false)?;
            (
                db::find_symbols_by_name_scoped(&conn, name, kind, limit, scope)?,
                total,
            )
        }
    };

    let resolver = PathResolver::try_from_conn(root, &conn)?;
    symbols.retain(|s| resolver.matches_filter(s.root_path.as_deref()));
    for s in &mut symbols {
        s.path = resolver.resolve_with_root(&s.path, s.root_path.as_deref());
    }

    let page = Page::new(symbols, total, limit);
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&page)?);
        return Ok(());
    }

    let query_str = pattern.unwrap_or(name.unwrap_or(""));
    let kind_str = kind.map(|k| format!(" ({})", k)).unwrap_or_default();
    println!(
        "{}",
        format!(
            "Symbols matching '{}'{} (showing {} of {}):",
            query_str, kind_str, page.pagination.returned, page.pagination.total
        )
        .bold()
    );

    for s in &page.items {
        println!(
            "  {} [{}]: {}:{}",
            symbol_display_name(s).cyan(),
            s.kind,
            s.path,
            s.line
        );
        if let Some(sig) = &s.signature {
            let truncated: String = sig.chars().take(70).collect();
            println!("    {}", truncated.dimmed());
        }
    }

    if page.items.is_empty() {
        println!("  No symbols found.");
    }
    print_truncation_notice(page.pagination);

    Ok(())
}

/// Find class by name or glob pattern (classes, interfaces, objects, enums)
pub fn cmd_class(
    root: &Path,
    name: Option<&str>,
    pattern: Option<&str>,
    limit: usize,
    format: &str,
    scope: &SearchScope,
    fuzzy: bool,
) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let (name, pattern) = auto_pattern_from_name(name, pattern);

    if name.is_none() && pattern.is_none() {
        println!("{}", "Either a class name or --pattern is required.".red());
        return Ok(());
    }

    let conn = db::open_db_leased(root)?;

    let (mut results, total): (Vec<db::SearchResult>, usize) = if let Some(pat) = pattern {
        let like_pattern = db::glob_to_like(pat);
        let total = db::count_symbols_by_pattern_scoped(&conn, &like_pattern, None, scope, true)?;
        (
            db::find_class_like_pattern(&conn, &like_pattern, limit, scope)?,
            total,
        )
    } else {
        let name = name.unwrap();
        if fuzzy {
            let total = db::count_symbols_fuzzy_scoped(&conn, name, None, scope, true)?;
            let results =
                db::search_symbols_for_command(&conn, name, None, limit, scope, true, true)?;
            (results, total)
        } else {
            let total = db::count_class_like_scoped(&conn, name, scope)?;
            (
                db::find_class_like_scoped(&conn, name, limit, scope)?,
                total,
            )
        }
    };

    let resolver = PathResolver::try_from_conn(root, &conn)?;
    results.retain(|s| resolver.matches_filter(s.root_path.as_deref()));
    for s in &mut results {
        s.path = resolver.resolve_with_root(&s.path, s.root_path.as_deref());
    }

    let page = Page::new(results, total, limit);
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&page)?);
        return Ok(());
    }

    let query_str = pattern.unwrap_or(name.unwrap_or(""));
    println!(
        "{}",
        format!(
            "Classes matching '{}' (showing {} of {}):",
            query_str, page.pagination.returned, page.pagination.total
        )
        .bold()
    );

    for s in &page.items {
        println!(
            "  {} [{}]: {}:{}",
            symbol_display_name(s).cyan(),
            s.kind,
            s.path,
            s.line
        );
    }

    if page.items.is_empty() {
        println!("  No classes found.");
    }
    print_truncation_notice(page.pagination);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::auto_pattern_from_name;

    #[test]
    fn auto_pattern_keeps_explicit_pattern() {
        let (name, pattern) = auto_pattern_from_name(Some("Client"), Some("foo*"));
        assert_eq!(name, Some("Client"));
        assert_eq!(pattern, Some("foo*"));
    }

    #[test]
    fn auto_pattern_promotes_star_name() {
        let (name, pattern) = auto_pattern_from_name(Some("AcceptanceOperationInitiator::*"), None);
        assert_eq!(name, None);
        assert_eq!(pattern, Some("AcceptanceOperationInitiator::*"));
    }

    #[test]
    fn auto_pattern_promotes_question_name() {
        let (name, pattern) = auto_pattern_from_name(Some("Client?"), None);
        assert_eq!(name, None);
        assert_eq!(pattern, Some("Client?"));
    }

    #[test]
    fn auto_pattern_leaves_exact_name_alone() {
        let (name, pattern) = auto_pattern_from_name(Some("kAntifraud"), None);
        assert_eq!(name, Some("kAntifraud"));
        assert_eq!(pattern, None);
    }
}

/// Find implementations of interface/class
pub fn cmd_implementations(
    root: &Path,
    parent: &str,
    limit: usize,
    format: &str,
    scope: &SearchScope,
) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db_leased(root)?;
    let total = db::count_implementations_scoped(&conn, parent, scope)?;
    let mut impls = db::find_implementations_scoped(&conn, parent, limit, scope)?;

    let resolver = PathResolver::try_from_conn(root, &conn)?;
    impls.retain(|s| resolver.matches_filter(s.root_path.as_deref()));
    for s in &mut impls {
        s.path = resolver.resolve_with_root(&s.path, s.root_path.as_deref());
    }

    let page = Page::new(impls, total, limit);
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&page)?);
        return Ok(());
    }

    println!(
        "{}",
        format!(
            "Implementations of '{}' (showing {} of {}):",
            parent, page.pagination.returned, page.pagination.total
        )
        .bold()
    );

    for s in &page.items {
        println!(
            "  {} [{}]: {}:{}",
            symbol_display_name(s).cyan(),
            s.kind,
            s.path,
            s.line
        );
    }

    if page.items.is_empty() {
        println!("  No implementations found.");
    }
    print_truncation_notice(page.pagination);

    Ok(())
}

/// Show cross-references: definitions, imports, usages
pub fn cmd_refs(root: &Path, symbol: &str, limit: usize, format: &str) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db_leased(root)?;
    let no_scope = SearchScope::none();
    let definitions_total = db::count_symbols_by_name_scoped(&conn, symbol, None, &no_scope, true)?;
    let imports_total = db::count_imports_scoped(&conn, symbol, &no_scope)?;
    let usages_total = db::count_references_scoped(&conn, symbol, &no_scope)?;
    let mut definitions = db::find_definitions_scoped(&conn, symbol, limit, &no_scope)?;
    let mut imports = db::find_imports_scoped(&conn, symbol, limit, &no_scope)?;
    let mut usages = db::find_references_scoped(&conn, symbol, limit, &no_scope)?;

    let resolver = PathResolver::try_from_conn(root, &conn)?;
    definitions.retain(|s| resolver.matches_filter(s.root_path.as_deref()));
    imports.retain(|s| resolver.matches_filter(s.root_path.as_deref()));
    usages.retain(|r| resolver.matches_filter(r.root_path.as_deref()));
    for s in &mut definitions {
        s.path = resolver.resolve_with_root(&s.path, s.root_path.as_deref());
    }
    for s in &mut imports {
        s.path = resolver.resolve_with_root(&s.path, s.root_path.as_deref());
    }
    for r in &mut usages {
        r.path = resolver.resolve_with_root(&r.path, r.root_path.as_deref());
    }

    let definitions_page = Page::new(definitions, definitions_total, limit);
    let imports_page = Page::new(imports, imports_total, limit);
    let usages_page = Page::new(usages, usages_total, limit);

    if format == "json" {
        let result = serde_json::json!({
            "schema_version": PAGINATED_JSON_SCHEMA_VERSION,
            "definitions": definitions_page.items,
            "imports": imports_page.items,
            "usages": usages_page.items,
            "pagination": {
                "definitions": definitions_page.pagination,
                "imports": imports_page.pagination,
                "usages": usages_page.pagination,
            },
        });
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    println!("{}", format!("Cross-references for '{}':", symbol).bold());

    if !definitions_page.items.is_empty() {
        println!(
            "\n  {}",
            format!(
                "Definitions (showing {} of {}):",
                definitions_page.pagination.returned, definitions_page.pagination.total
            )
            .cyan()
        );
        for s in &definitions_page.items {
            println!(
                "    {} [{}]: {}:{}",
                symbol_display_name(s).cyan(),
                s.kind,
                s.path,
                s.line
            );
        }
        print_truncation_notice(definitions_page.pagination);
    }

    if !imports_page.items.is_empty() {
        println!(
            "\n  {}",
            format!(
                "Imports (showing {} of {}):",
                imports_page.pagination.returned, imports_page.pagination.total
            )
            .cyan()
        );
        for s in &imports_page.items {
            println!("    {}:{}", s.path.cyan(), s.line);
            if let Some(sig) = &s.signature {
                println!("      {}", sig.dimmed());
            }
        }
        print_truncation_notice(imports_page.pagination);
    }

    if !usages_page.items.is_empty() {
        println!(
            "\n  {}",
            format!(
                "Usages (showing {} of {}):",
                usages_page.pagination.returned, usages_page.pagination.total
            )
            .cyan()
        );
        for r in &usages_page.items {
            println!("    {}:{}", r.path.cyan(), r.line);
            if let Some(ctx) = &r.context {
                let truncated: String = ctx.chars().take(80).collect();
                println!("      {}", truncated.dimmed());
            }
        }
        print_truncation_notice(usages_page.pagination);
    }

    if definitions_page.items.is_empty()
        && imports_page.items.is_empty()
        && usages_page.items.is_empty()
    {
        println!("  No references found.");
    }

    Ok(())
}

/// Show class hierarchy (parents and children)
pub fn cmd_hierarchy(root: &Path, name: &str, limit: usize, scope: &SearchScope) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db_leased(root)?;

    // Find the class/interface/package
    let classes = db::find_symbols_by_name(&conn, name, Some("class"), 1)?;
    let interfaces = db::find_symbols_by_name(&conn, name, Some("interface"), 1)?;
    let packages = db::find_symbols_by_name(&conn, name, Some("package"), 1)?;
    let protocols = db::find_symbols_by_name(&conn, name, Some("protocol"), 1)?;

    let target = classes
        .first()
        .or(interfaces.first())
        .or(packages.first())
        .or(protocols.first());

    if target.is_none() {
        println!("{}", format!("Class '{}' not found.", name).red());
        return Ok(());
    }

    println!("{}", format!("Hierarchy for '{}':", name).bold());

    // Find parents
    let mut stmt = conn.prepare(
        "SELECT i.parent_name, i.kind FROM inheritance i JOIN symbols s ON i.child_id = s.id WHERE s.name = ?1 OR s.qualified_name = ?2",
    )?;
    let parents: Vec<(String, String)> = stmt
        .query_map([target.unwrap().name.as_str(), name], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<Result<_, _>>()?;

    if !parents.is_empty() {
        println!("\n  {}", "Parents:".cyan());
        for (parent, kind) in &parents {
            println!("    {} ({})", parent, kind);
        }
    }

    // Find children (with optional scope filtering). Pre-scope total comes
    // from a COUNT(*) so we can warn when display is truncated.
    let total = db::count_implementations(&conn, name)?;
    let mut children: Vec<db::SearchResult> = if scope.is_empty() {
        db::find_implementations(&conn, name, limit)?
    } else {
        let all = db::find_implementations(&conn, name, total.max(limit))?;
        all.into_iter()
            .filter(|s| {
                if let Some(in_file) = scope.in_file {
                    if !s.path.contains(in_file) {
                        return false;
                    }
                }
                if let Some(module) = scope.module {
                    if !s.path.starts_with(module) {
                        return false;
                    }
                }
                if let Some(prefix) = scope.dir_prefix {
                    if !s.path.starts_with(prefix) {
                        return false;
                    }
                }
                true
            })
            .take(limit)
            .collect()
    };
    let resolver = PathResolver::try_from_conn(root, &conn)?;
    children.retain(|c| resolver.matches_filter(c.root_path.as_deref()));
    for c in &mut children {
        c.path = resolver.resolve_with_root(&c.path, c.root_path.as_deref());
    }
    if !children.is_empty() {
        let header = if scope.is_empty() && total > children.len() {
            format!("Children ({} of {} shown):", children.len(), total)
        } else if !scope.is_empty() && children.len() == limit {
            format!(
                "Children (showing {}, more may exist within scope):",
                children.len()
            )
        } else {
            format!("Children ({}):", children.len())
        };
        println!("\n  {}", header.cyan());
        for c in &children {
            println!("    {} [{}]: {}", symbol_display_name(c), c.kind, c.path);
        }
        if scope.is_empty() && total > children.len() {
            println!(
                "\n  {} use {} to see all (e.g. --limit {})",
                "Truncated.".yellow(),
                "--limit <N>".yellow(),
                total
            );
        }
    }

    Ok(())
}

/// Find symbol usages (indexed or grep-based)
pub fn cmd_usages(
    root: &Path,
    symbol: &str,
    limit: usize,
    format: &str,
    scope: &SearchScope,
) -> Result<()> {
    // Try to use index first
    let _cache_lease = db::acquire_project_lease(root)?;
    let db_path = db::get_db_path(root)?;
    if db_path.exists() {
        let conn = db::open_db_leased(root)?;

        // Check if refs table has data
        let refs_count = db::count_references_scoped(&conn, symbol, scope)?;

        if refs_count > 0 {
            // Use indexed references with scope filtering
            let total = db::count_references_scoped(&conn, symbol, scope)?;
            let mut refs = db::find_references_scoped(&conn, symbol, limit, scope)?;
            let resolver = PathResolver::try_from_conn(root, &conn)?;
            refs.retain(|r| resolver.matches_filter(r.root_path.as_deref()));
            for r in &mut refs {
                r.path = resolver.resolve_with_root(&r.path, r.root_path.as_deref());
            }

            let page = Page::new(refs, total, limit);
            if format == "json" {
                println!("{}", serde_json::to_string_pretty(&page)?);
                return Ok(());
            }

            println!(
                "{}",
                format!(
                    "Usages of '{}' (showing {} of {}):",
                    symbol, page.pagination.returned, page.pagination.total
                )
                .bold()
            );

            for r in &page.items {
                println!("  {}:{}", r.path.cyan(), r.line);
                if let Some(ctx) = &r.context {
                    let truncated: String = ctx.chars().take(80).collect();
                    println!("    {}", truncated);
                }
            }

            if page.items.is_empty() {
                println!("  No usages found in index.");
            }
            print_truncation_notice(page.pagination);

            return Ok(());
        }
    }

    // Fallback to grep-based search
    let pattern = format!(r"\b{}\b", regex::escape(symbol));
    let def_pattern = Regex::new(&format!(
        r"(class|interface|object|fun|val|var|typealias)\s+{}\b",
        regex::escape(symbol)
    ))?;

    let page = search_files_page(
        root,
        &pattern,
        &["kt", "java"],
        limit,
        |path, line_num, line| {
            // Skip definitions
            if def_pattern.is_match(line) {
                return None;
            }

            let rel_path = relative_path(root, path);
            // Apply scope filter for grep results
            if let Some(in_file) = scope.in_file {
                if !rel_path.contains(in_file) {
                    return None;
                }
            }
            if let Some(module) = scope.module {
                if !rel_path.starts_with(module) {
                    return None;
                }
            }
            if let Some(prefix) = scope.dir_prefix {
                if !rel_path.starts_with(prefix) {
                    return None;
                }
            }
            let content: String = line.trim().chars().take(80).collect();
            Some((rel_path, line_num, content))
        },
    )?;

    if format == "json" {
        let items: Vec<_> = page
            .items
            .iter()
            .map(|(p, l, c)| serde_json::json!({"path": p, "line": l, "content": c}))
            .collect();
        let result = serde_json::json!({
            "schema_version": PAGINATED_JSON_SCHEMA_VERSION,
            "items": items,
            "pagination": page.pagination,
        });
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    println!(
        "{}",
        format!(
            "Usages of '{}' (showing {} of {}):",
            symbol, page.pagination.returned, page.pagination.total
        )
        .bold()
    );

    for (path, line_num, content) in &page.items {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    if page.items.is_empty() {
        println!("  No usages found.");
    }
    print_truncation_notice(page.pagination);

    Ok(())
}
