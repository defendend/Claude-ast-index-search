use std::fs;

use ast_index::commands::WordIndex;
use ast_index::db;
use ast_index::indexer::{self, content_words, literal_word_runs};
use tempfile::TempDir;

fn indexed_project(files: &[(&str, &str)]) -> (TempDir, rusqlite::Connection) {
    let dir = TempDir::new().unwrap();
    for (path, content) in files {
        fs::write(dir.path().join(path), content).unwrap();
    }
    if db::db_exists(dir.path()) {
        db::delete_db(dir.path()).unwrap();
    }
    let mut conn = db::open_db(dir.path()).unwrap();
    db::init_db(&conn).unwrap();
    indexer::index_directory(&mut conn, dir.path(), false, false).unwrap();
    (dir, conn)
}

#[test]
fn every_run_of_a_literal_lies_in_a_word_of_any_text_containing_it() {
    let texts = [
        "call(user.id) # current_user",
        "Applicant::MergeService.call!",
        "строка «Заявка_1» — done",
        "a->b; c::d",
    ];
    for text in texts {
        let words = content_words(text);
        let words: Vec<&str> = words.split('\n').collect();
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        for (i, &(start, _)) in chars.iter().enumerate() {
            for &(end, _) in chars[i..].iter().skip(1) {
                let literal = &text[start..end];
                for run in literal_word_runs(literal) {
                    assert!(
                        words.iter().any(|word| word.contains(&run)),
                        "{run:?} of {literal:?} in {text:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn prefilter_skips_only_unchanged_indexed_files_without_the_literal() {
    let (dir, conn) = indexed_project(&[
        ("alpha.rb", "def alpha\n  helper(1)\nend\n"),
        ("beta.rb", "def beta\n  alpha\nend\n"),
    ]);
    let root = dir.path();
    let words = WordIndex::load(root, &conn)
        .unwrap()
        .expect("index keeps words");

    let beta = words.prefilter(&["beta"]).unwrap();
    assert!(!beta.may_contain(&root.join("alpha.rb")));
    assert!(beta.may_contain(&root.join("beta.rb")));

    let either = words.prefilter(&["beta", "helper"]).unwrap();
    assert!(either.may_contain(&root.join("alpha.rb")));

    // A literal without word characters cannot be ruled out anywhere.
    assert!(words.prefilter(&["->"]).is_none());

    // A file edited after indexing is searched again, as is a new one.
    fs::write(root.join("alpha.rb"), "def alpha\n  beta_helper(1)\nend\n").unwrap();
    assert!(beta.may_contain(&root.join("alpha.rb")));
    fs::write(root.join("gamma.rb"), "def gamma; end\n").unwrap();
    assert!(beta.may_contain(&root.join("gamma.rb")));
}
