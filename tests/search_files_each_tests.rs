use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use ast_index::commands::{project_source_files, search_files_limited, search_files_limited_each};
use tempfile::TempDir;

type Hit = (String, usize, String);

fn name(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn separately(root: &Path, pattern: &str, limit: usize) -> BTreeSet<Hit> {
    let mut hits = BTreeSet::new();
    search_files_limited(root, pattern, &["rb"], limit, |path, line_num, line| {
        hits.insert((name(root, path), line_num, line.to_string()));
    })
    .unwrap();
    hits
}

fn together_keeping(
    root: &Path,
    files: &[PathBuf],
    patterns: &[(String, String)],
    limit: usize,
    keep: impl Fn(usize, &Path, &str) -> bool + Sync,
) -> Vec<Vec<Hit>> {
    let candidates = patterns
        .iter()
        .map(|(pattern, _)| format!("(?:{pattern})"))
        .collect::<Vec<_>>()
        .join("|");
    let mut hits = vec![Vec::new(); patterns.len()];
    search_files_limited_each(
        files,
        &candidates,
        patterns,
        limit,
        keep,
        |index, path, line_num, line| {
            hits[index].push((name(root, path), line_num, line.to_string()));
        },
    )
    .unwrap();
    hits
}

fn together(
    root: &Path,
    files: &[PathBuf],
    patterns: &[(String, String)],
    limit: usize,
) -> Vec<Vec<Hit>> {
    together_keeping(root, files, patterns, limit, |_, _, _| true)
}

fn pattern(literal: &str) -> (String, String) {
    (
        format!(r"\.{}\(", regex::escape(literal)),
        literal.to_string(),
    )
}

fn hit(file: &str, line_num: usize, line: &str) -> Hit {
    (file.to_string(), line_num, line.to_string())
}

// One test on purpose: it points the cache at a temporary directory through
// the process environment, which parallel tests in this binary would race on.
#[test]
fn one_pass_gives_each_pattern_its_first_lines_in_path_order() {
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

    let files = project_source_files(root, &["rb"]).unwrap();
    let names: Vec<String> = files.iter().map(|path| name(root, path)).collect();
    assert_eq!(names, ["a.rb", "b.rb", "c.rb"]);

    let patterns = vec![pattern("foo"), pattern("bar"), pattern("missing")];
    let wide = together(root, &files, &patterns, 100);
    for (index, (pattern, _)) in patterns.iter().enumerate() {
        let set: BTreeSet<Hit> = wide[index].iter().cloned().collect();
        assert_eq!(set, separately(root, pattern, 100), "{pattern}");
        let mut sorted = wide[index].clone();
        sorted.sort();
        assert_eq!(wide[index], sorted, "{pattern} not in path order");
    }
    assert_eq!(wide[0].len(), 3);
    assert_eq!(wide[1].len(), 5);
    assert!(wide[2].is_empty());

    // Each pattern spends its own budget on its first lines: a pattern with
    // plenty of matches stops at the limit while the others still get theirs.
    let narrow = together(root, &files, &patterns, 2);
    assert_eq!(
        narrow[0],
        [
            hit("a.rb", 1, "x.foo(1)"),
            hit("a.rb", 3, "z.foo(3); w.bar(4)")
        ]
    );
    assert_eq!(
        narrow[1],
        [
            hit("a.rb", 2, "y.bar(2)"),
            hit("a.rb", 3, "z.foo(3); w.bar(4)")
        ]
    );
    assert!(narrow[2].is_empty());

    // Lines `keep` turns down do not count against the budget.
    let kept = together_keeping(root, &files, &patterns, 2, |index, _, line| {
        index != 0 || !line.starts_with('x')
    });
    assert_eq!(
        kept[0],
        [
            hit("a.rb", 3, "z.foo(3); w.bar(4)"),
            hit("c.rb", 1, "a.foo(10)")
        ]
    );
    assert_eq!(kept[1], narrow[1]);

    assert!(together(root, &files, &patterns, 0)
        .iter()
        .all(Vec::is_empty));
    assert!(together(root, &[], &patterns, 2).iter().all(Vec::is_empty));

    // Many files searched in parallel still give the same first lines, the
    // ones earliest in path order, on every run.
    let many = TempDir::new().unwrap();
    for dir in 0..8 {
        let dir_path = many.path().join(format!("d{dir}"));
        fs::create_dir(&dir_path).unwrap();
        for file in 0..40 {
            let body: String = (0..30)
                .map(|line| format!("s{line}.foo({dir}{file})\n"))
                .collect();
            fs::write(dir_path.join(format!("f{file:02}.rb")), body).unwrap();
        }
    }
    let files = project_source_files(many.path(), &["rb"]).unwrap();
    assert_eq!(files.len(), 320);
    let mut sorted = files.clone();
    sorted.sort();
    assert_eq!(files, sorted);
    let first = together(many.path(), &files, &patterns[..1], 100);
    let expected: Vec<Hit> = (0..4)
        .flat_map(|file| {
            (0..30).map(move |line| {
                (
                    format!("d0/f{file:02}.rb"),
                    line + 1,
                    format!("s{line}.foo(0{file})"),
                )
            })
        })
        .take(100)
        .collect();
    assert_eq!(first[0], expected);
    for _ in 0..20 {
        assert_eq!(together(many.path(), &files, &patterns[..1], 100), first);
    }
}
