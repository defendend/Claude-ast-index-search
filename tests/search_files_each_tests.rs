use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use ast_index::commands::{search_files_limited, search_files_limited_each};
use tempfile::TempDir;

type Hits = BTreeSet<(String, usize, String)>;

fn separately(root: &Path, pattern: &str, limit: usize) -> Hits {
    let mut hits = Hits::new();
    search_files_limited(root, pattern, &["rb"], limit, |path, line_num, line| {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        hits.insert((name, line_num, line.to_string()));
    })
    .unwrap();
    hits
}

fn together(root: &Path, patterns: &[(String, String)], limit: usize) -> Vec<Hits> {
    let candidates = patterns
        .iter()
        .map(|(pattern, _)| format!("(?:{pattern})"))
        .collect::<Vec<_>>()
        .join("|");
    let mut hits = vec![Hits::new(); patterns.len()];
    search_files_limited_each(
        root,
        &candidates,
        patterns,
        &["rb"],
        limit,
        |index, path, line_num, line| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            hits[index].insert((name, line_num, line.to_string()));
        },
    )
    .unwrap();
    hits
}

fn pattern(literal: &str) -> (String, String) {
    (
        format!(r"\.{}\(", regex::escape(literal)),
        literal.to_string(),
    )
}

// One test on purpose: it points the cache at a temporary directory through
// the process environment, which parallel tests in this binary would race on.
#[test]
fn one_walk_serves_each_pattern_like_its_own_search() {
    let cache = TempDir::new().unwrap();
    std::env::set_var("AST_INDEX_CACHE_DIR", cache.path());
    std::env::remove_var("AST_INDEX_DB_PATH");
    std::env::remove_var("KOTLIN_INDEX_DB_PATH");

    let project = TempDir::new().unwrap();
    let root = project.path();
    fs::write(
        root.join("a.rb"),
        "x.foo(1)\ny.bar(2)\nz.foo(3); w.bar(4)\nfoo(5)\n",
    )
    .unwrap();
    fs::write(root.join("b.rb"), "q.bar(6)\nr.baz(7)\n").unwrap();
    // A non-UTF-8 line ends a search at the first line it matches, so `foo`
    // loses `c.foo(12)` while `bar`, which never matched that line, keeps
    // `d.bar(13)`.
    let mut bytes = b"a.foo(10)\nb.bar(11)\nx.foo(0) \xff\n".to_vec();
    bytes.extend_from_slice(b"c.foo(12)\nd.bar(13)\n");
    fs::write(root.join("c.rb"), bytes).unwrap();
    fs::write(root.join("skipped.txt"), "t.foo(99)\n").unwrap();

    let patterns = vec![pattern("foo"), pattern("bar"), pattern("missing")];
    let wide = together(root, &patterns, 100);
    for (index, (pattern, _)) in patterns.iter().enumerate() {
        assert_eq!(wide[index], separately(root, pattern, 100), "{pattern}");
    }
    assert_eq!(wide[0].len(), 3);
    assert_eq!(wide[1].len(), 5);
    assert!(wide[2].is_empty());

    // Each pattern spends its own budget: a pattern with plenty of matches
    // stops at the limit while the others still get theirs.
    let narrow = together(root, &patterns, 2);
    assert_eq!(narrow[0].len(), 2);
    assert_eq!(narrow[1].len(), 2);
    assert!(narrow[0].is_subset(&wide[0]));
    assert!(narrow[1].is_subset(&wide[1]));
    assert!(narrow[2].is_empty());

    assert!(together(root, &patterns, 0).iter().all(BTreeSet::is_empty));
}
