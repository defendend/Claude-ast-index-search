//! Perl-specific commands
//!
//! Commands for working with Perl codebases:
//! - perl_exports: Find @EXPORT/@EXPORT_OK definitions
//! - perl_subs: Find subroutine definitions
//! - perl_pod: Find POD documentation
//! - perl_tests: Find test assertions
//! - perl_imports: Find use/require statements

use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use colored::Colorize;

use super::{search_files_limited, relative_path};

/// Find Perl @EXPORT and @EXPORT_OK definitions
pub fn cmd_perl_exports(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();

    // Search for @EXPORT and @EXPORT_OK definitions
    let pattern = r"our\s+@EXPORT|our\s+@EXPORT_OK|@EXPORT\s*=|@EXPORT_OK\s*=";

    let mut results: Vec<(String, usize, String)> = vec![];

    search_files_limited(root, pattern, &["pm"], limit, |path, line_num, line| {
        if let Some(q) = query {
            if !line.to_lowercase().contains(&q.to_lowercase()) {
                return;
            }
        }

        let rel_path = relative_path(root, path);
        let content: String = line.trim().chars().take(100).collect();
        results.push((rel_path, line_num, content));
    })?;

    println!(
        "{}",
        format!("Perl exports ({}):", results.len()).bold()
    );

    for (path, line_num, content) in &results {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find Perl subroutine definitions
pub fn cmd_perl_subs(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();

    // Search for sub definitions
    let pattern = r"^\s*sub\s+\w+";

    let mut results: Vec<(String, usize, String)> = vec![];

    search_files_limited(root, pattern, &["pm", "pl", "t"], limit, |path, line_num, line| {
        if let Some(q) = query {
            if !line.to_lowercase().contains(&q.to_lowercase()) {
                return;
            }
        }

        let rel_path = relative_path(root, path);
        let content: String = line.trim().chars().take(80).collect();
        results.push((rel_path, line_num, content));
    })?;

    println!(
        "{}",
        format!("Perl subroutines ({}):", results.len()).bold()
    );

    for (path, line_num, content) in &results {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find POD documentation sections
pub fn cmd_perl_pod(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();

    // Search for POD documentation sections
    // =head1, =head2, =head3, =head4, =item, =over, =back, =pod, =cut, =begin, =end
    let pattern = r"^=(head[1-4]|item|over|back|pod|cut|begin|end|for)\b";

    let mut results: Vec<(String, usize, String)> = vec![];

    search_files_limited(root, pattern, &["pm", "pl", "pod"], limit, |path, line_num, line| {
        if let Some(q) = query {
            if !line.to_lowercase().contains(&q.to_lowercase()) {
                return;
            }
        }

        let rel_path = relative_path(root, path);
        let content: String = line.trim().chars().take(100).collect();
        results.push((rel_path, line_num, content));
    })?;

    println!(
        "{}",
        format!("POD documentation ({}):", results.len()).bold()
    );

    for (path, line_num, content) in &results {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find Perl test assertions (Test::More, Test::Simple)
pub fn cmd_perl_tests(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();

    // Search for Test::More and Test::Simple assertions
    // ok(), is(), isnt(), like(), unlike(), cmp_ok(), is_deeply(), diag(), pass(), fail()
    // subtest, plan, done_testing, SKIP, TODO
    let pattern = r"\b(ok|is|isnt|like|unlike|cmp_ok|is_deeply|diag|pass|fail|subtest|plan|done_testing|SKIP|TODO)\s*[\(\{]";

    let mut results: Vec<(String, usize, String)> = vec![];

    search_files_limited(root, pattern, &["t", "pm", "pl"], limit, |path, line_num, line| {
        if let Some(q) = query {
            if !line.to_lowercase().contains(&q.to_lowercase()) {
                return;
            }
        }

        let rel_path = relative_path(root, path);
        let content: String = line.trim().chars().take(100).collect();
        results.push((rel_path, line_num, content));
    })?;

    println!(
        "{}",
        format!("Perl tests ({}):", results.len()).bold()
    );

    for (path, line_num, content) in &results {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find Perl use/require statements
pub fn cmd_perl_imports(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();

    // Search for use/require statements
    let pattern = r"^\s*(use|require)\s+[A-Za-z]";

    let mut results: Vec<(String, usize, String)> = vec![];

    search_files_limited(root, pattern, &["pm", "pl", "t"], limit, |path, line_num, line| {
        // Skip 'use strict', 'use warnings', 'use constant', 'use base', 'use parent'
        let trimmed = line.trim();
        if trimmed.starts_with("use strict") ||
           trimmed.starts_with("use warnings") ||
           trimmed.starts_with("use constant") ||
           trimmed.starts_with("use base") ||
           trimmed.starts_with("use parent") ||
           trimmed.starts_with("use utf8") ||
           trimmed.starts_with("use v5") ||
           trimmed.starts_with("use 5.") {
            return;
        }

        if let Some(q) = query {
            if !line.to_lowercase().contains(&q.to_lowercase()) {
                return;
            }
        }

        let rel_path = relative_path(root, path);
        let content: String = line.trim().chars().take(100).collect();
        results.push((rel_path, line_num, content));
    })?;

    println!(
        "{}",
        format!("Perl imports ({}):", results.len()).bold()
    );

    for (path, line_num, content) in &results {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}
