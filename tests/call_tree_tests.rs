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

/// `leaf` is called from an RSpec `let` block, from Ruby methods whose names
/// end in `!`, `?` and `=`, and from the body of a namespaced class. Every
/// one of those callers is called in turn, and a line elsewhere reads like a
/// call of the `let` block by its indexed name.
fn dsl_fixture() -> (TempDir, TempDir) {
    let project = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let files = [
        ("lib/leaf.rb", "class Leaf\n  def leaf\n    1\n  end\nend\n"),
        (
            "spec/leaf_spec.rb",
            "describe Leaf do\n  let(:fields) do\n    Leaf.new.leaf\n  end\nend\n",
        ),
        (
            "lib/dsl_helper.rb",
            "class DslHelper\n  def build\n    self.let(:fields).tap { |f| f }\n  end\nend\n",
        ),
        (
            "lib/record.rb",
            concat!(
                "class Record\n",
                "  def save!\n",
                "    Leaf.new.leaf\n",
                "  end\n",
                "\n",
                "  def valid?\n",
                "    Leaf.new.leaf\n",
                "  end\n",
                "\n",
                "  def name=(value)\n",
                "    Leaf.new.leaf\n",
                "  end\n",
                "end\n",
            ),
        ),
        (
            "lib/billing/invoice.rb",
            "class Billing::Invoice\n  Leaf.new.leaf\nend\n",
        ),
        (
            "lib/app.rb",
            concat!(
                "class App\n",
                "  def persist\n",
                "    Record.new.save!\n",
                "  end\n",
                "\n",
                "  def check\n",
                "    Record.new.valid?\n",
                "  end\n",
                "\n",
                "  def rename\n",
                "    Record.new.name=(\"x\")\n",
                "  end\n",
                "\n",
                "  def bill\n",
                "    Billing::Invoice.new\n",
                "  end\n",
                "end\n",
            ),
        ),
    ];
    for (path, content) in files {
        let path = project.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    (project, cache)
}

#[test]
fn call_tree_shows_dsl_blocks_but_expands_only_identifiers() {
    let (project, cache) = dsl_fixture();
    stdout(&run(project.path(), cache.path(), &["rebuild"]));
    let output = run(project.path(), cache.path(), &["call-tree", "leaf"]);
    assert_eq!(
        stdout(&output),
        concat!(
            "Call tree for 'leaf':\n",
            "  leaf\n",
            "    ← Billing::Invoice (lib/billing/invoice.rb:1)\n",
            "      ← bill (lib/app.rb:14)\n",
            "    ← save! (lib/record.rb:2)\n",
            "      ← persist (lib/app.rb:2)\n",
            "    ← valid? (lib/record.rb:6)\n",
            "      ← check (lib/app.rb:6)\n",
            "    ← name= (lib/record.rb:10)\n",
            "      ← rename (lib/app.rb:10)\n",
            "    ← let(:fields) (spec/leaf_spec.rb:2)\n",
        )
    );
}

/// Twelve files with three callers of `leaf` each, after a file that defines
/// `leaf` ten times over and before a spec file that calls it too.
fn wide_fixture() -> (TempDir, TempDir) {
    let project = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let lib = project.path().join("lib");
    let spec = project.path().join("spec");
    fs::create_dir(&lib).unwrap();
    fs::create_dir(&spec).unwrap();
    let definitions: String = (0..10)
        .map(|index| format!("class Leaf{index}\n  def leaf\n    {index}\n  end\nend\n"))
        .collect();
    fs::write(lib.join("a_definitions.rb"), definitions).unwrap();
    for file in 0..12 {
        let methods: String = (0..3)
            .map(|method| format!("  def c{file:02}_{method}\n    Leaf0.new.leaf\n  end\n"))
            .collect();
        fs::write(
            lib.join(format!("callers_{file:02}.rb")),
            format!("class Callers{file:02}\n{methods}end\n"),
        )
        .unwrap();
    }
    fs::write(
        spec.join("leaf_spec.rb"),
        "class LeafSpec\n  def check_leaf\n    Leaf0.new.leaf\n  end\nend\n",
    )
    .unwrap();
    (project, cache)
}

#[test]
fn call_tree_takes_the_first_callers_in_path_order_every_time() {
    let (project, cache) = wide_fixture();
    stdout(&run(project.path(), cache.path(), &["rebuild"]));
    let args = ["call-tree", "leaf", "--depth", "1", "--limit", "4"];
    let expected = concat!(
        "Call tree for 'leaf':\n",
        "  leaf\n",
        "    ← c00_0 (lib/callers_00.rb:2)\n",
        "    ← c00_1 (lib/callers_00.rb:5)\n",
        "    ← c00_2 (lib/callers_00.rb:8)\n",
        "    ← c01_0 (lib/callers_01.rb:2)\n",
    );
    for _ in 0..5 {
        assert_eq!(stdout(&run(project.path(), cache.path(), &args)), expected);
    }
}

#[test]
fn call_tree_spends_the_limit_on_calls_inside_the_file_filter() {
    let (project, cache) = wide_fixture();
    stdout(&run(project.path(), cache.path(), &["rebuild"]));
    let output = run(
        project.path(),
        cache.path(),
        &[
            "call-tree",
            "leaf",
            "--depth",
            "1",
            "--limit",
            "1",
            "--in-file",
            "spec/",
        ],
    );
    assert_eq!(
        stdout(&output),
        concat!(
            "Call tree for 'leaf':\n",
            "  leaf\n",
            "    ← check_leaf (spec/leaf_spec.rb:2)\n",
        )
    );
}
