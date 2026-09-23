use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn run(root: &Path, cache: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(root)
        .env("AST_INDEX_CACHE_DIR", cache)
        .env("AST_INDEX_DISABLE_GC", "1")
        .env("NO_COLOR", "1")
        .env_remove("AST_INDEX_DB_PATH")
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .args(args)
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

/// `leaf` is called from `alpha` and `beta`, both called from `top`. Each
/// level sits in a single file, so the order of the printed tree is fixed.
fn fixture() -> (TempDir, TempDir) {
    let project = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let lib = project.path().join("lib");
    fs::create_dir(&lib).unwrap();
    fs::write(
        lib.join("leaf.rb"),
        "class Leaf\n  def leaf\n    1\n  end\nend\n",
    )
    .unwrap();
    fs::write(
        lib.join("callers.rb"),
        concat!(
            "class Callers\n",
            "  def alpha\n",
            "    Leaf.new.leaf\n",
            "  end\n",
            "\n",
            "  def beta\n",
            "    Leaf.new.leaf\n",
            "  end\n",
            "end\n",
        ),
    )
    .unwrap();
    fs::write(
        lib.join("top.rb"),
        "class Top\n  def top\n    alpha()\n    beta()\n  end\nend\n",
    )
    .unwrap();
    (project, cache)
}

const FULL_TREE: &str = concat!(
    "Call tree for 'leaf':\n",
    "  leaf\n",
    "    ← alpha (lib/callers.rb:2)\n",
    "      ← top (lib/top.rb:2)\n",
    "    ← beta (lib/callers.rb:6)\n",
    "      ← top (recursive)\n",
);

#[test]
fn call_tree_prints_every_level_depth_first() {
    let (project, cache) = fixture();
    let output = run(project.path(), cache.path(), &["call-tree", "leaf"]);
    assert_eq!(stdout(&output), FULL_TREE);
}

#[test]
fn call_tree_attributes_through_the_index_the_same_way() {
    let (project, cache) = fixture();
    stdout(&run(project.path(), cache.path(), &["rebuild"]));
    let output = run(project.path(), cache.path(), &["call-tree", "leaf"]);
    assert_eq!(stdout(&output), FULL_TREE);
}

#[test]
fn call_tree_stops_at_the_requested_depth() {
    let (project, cache) = fixture();
    let output = run(
        project.path(),
        cache.path(),
        &["call-tree", "leaf", "--depth", "1"],
    );
    assert_eq!(
        stdout(&output),
        concat!(
            "Call tree for 'leaf':\n",
            "  leaf\n",
            "    ← alpha (lib/callers.rb:2)\n",
            "    ← beta (lib/callers.rb:6)\n",
        )
    );
}
