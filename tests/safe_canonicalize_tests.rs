//! Regression: `db::safe_canonicalize` must not hang on dead/missing paths
//! and must honour the AST_INDEX_NO_CANONICALIZE bypass.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ast_index::db;
use tempfile::TempDir;

#[test]
fn returns_canonical_path_for_real_directory() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path();
    let result = db::safe_canonicalize(path);
    // canonical form of a real dir must equal what std::fs::canonicalize gives.
    assert_eq!(result, path.canonicalize().unwrap());
}

#[test]
fn returns_raw_path_when_target_missing() {
    // Non-existent path — canonicalize() returns Err quickly, we expect the
    // raw path back, not a panic or a hang.
    let path = PathBuf::from("/this/path/should/never/exist/ast-index-test");
    let start = Instant::now();
    let result = db::safe_canonicalize(&path);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "safe_canonicalize on missing path took too long: {:?}",
        start.elapsed()
    );
    assert_eq!(result, path);
}

#[test]
fn no_canonicalize_env_bypasses_completely() {
    // With the bypass env set we must skip the syscall altogether and return
    // the raw path even when the target is a perfectly valid directory.
    let tmp = TempDir::new().unwrap();
    let path = tmp.path();

    // Set env for this test only. Reset on drop to avoid leaking into other
    // tests in the same process.
    struct EnvGuard(&'static str, Option<String>);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.1.take() {
                Some(v) => std::env::set_var(self.0, v),
                None => std::env::remove_var(self.0),
            }
        }
    }
    let _guard = EnvGuard(
        "AST_INDEX_NO_CANONICALIZE",
        std::env::var("AST_INDEX_NO_CANONICALIZE").ok(),
    );
    std::env::set_var("AST_INDEX_NO_CANONICALIZE", "1");

    let result = db::safe_canonicalize(path);
    assert_eq!(result, path.to_path_buf(), "expected raw path with bypass");
}
