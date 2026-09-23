//! Auto-migration inspects every cache of a crowded base against a single
//! listing of `.leases`. These layouts pin the scan's decisions: owner intents
//! still apply per cache and per generation, and an unreadable intent fails
//! only its own cache.
//!
//! This target intentionally contains one test because cache and database
//! overrides are process-global.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use ast_index::db;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use tempfile::TempDir;

const OWNER_MANIFEST_NAME: &str = ".ast-index-owner-v1.json";
const GENERATION_MARKER_NAME: &str = ".ast-index-generation-v1.json";

struct CacheEnvironment {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl CacheEnvironment {
    fn isolated(cache_dir: &Path) -> Self {
        const KEYS: [&str; 5] = [
            "AST_INDEX_CACHE_DIR",
            "AST_INDEX_DB_PATH",
            "KOTLIN_INDEX_DB_PATH",
            "AST_INDEX_NO_CANONICALIZE",
            "AST_INDEX_CANONICALIZE_TIMEOUT_MS",
        ];
        let previous = KEYS
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();

        std::env::set_var("AST_INDEX_CACHE_DIR", cache_dir);
        std::env::remove_var("AST_INDEX_DB_PATH");
        std::env::remove_var("KOTLIN_INDEX_DB_PATH");
        std::env::remove_var("AST_INDEX_NO_CANONICALIZE");
        std::env::remove_var("AST_INDEX_CANONICALIZE_TIMEOUT_MS");

        Self { previous }
    }
}

impl Drop for CacheEnvironment {
    fn drop(&mut self) {
        for (key, value) in &self.previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

fn djb2(value: &str) -> String {
    let mut hash: u64 = 5381;
    for byte in value.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(u64::from(byte));
    }
    format!("{hash:x}")
}

fn owner(normalized_root: &str, raw_root: &str) -> Value {
    json!({
        "version": 1,
        "normalized_root": normalized_root,
        "raw_root": raw_root,
    })
}

fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn leases(cache_base: &Path) -> PathBuf {
    cache_base.join(".leases")
}

/// A current-format cache as ast-index leaves it: SQLite metadata, owner
/// manifest, generation marker, and lease files.
fn create_cache(
    cache_base: &Path,
    metadata_root: &str,
    manifest: &Value,
    generation: &str,
) -> PathBuf {
    let key = djb2(manifest["normalized_root"].as_str().unwrap());
    let cache_dir = cache_base.join(&key);
    fs::create_dir_all(&cache_dir).unwrap();
    let conn = Connection::open(cache_dir.join("index.db")).unwrap();
    conn.execute_batch("CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
        .unwrap();
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES ('project_root', ?1)",
        params![metadata_root],
    )
    .unwrap();
    drop(conn);
    write_json(&cache_dir.join(OWNER_MANIFEST_NAME), manifest);
    write_json(
        &cache_dir.join(GENERATION_MARKER_NAME),
        &json!({ "version": 1, "token": generation }),
    );
    fs::write(cache_dir.join("marker.txt"), &key).unwrap();
    fs::create_dir_all(leases(cache_base)).unwrap();
    for suffix in ["lock", "publish.lock"] {
        File::create(leases(cache_base).join(format!("{key}.{suffix}"))).unwrap();
    }
    cache_dir
}

fn intent_path(cache_base: &Path, key: &str, nonce: u32) -> PathBuf {
    leases(cache_base).join(format!("{key}.owner.4242.{nonce}.json"))
}

fn write_intent(cache_base: &Path, key: &str, nonce: u32, generation: &str, owner: Value) {
    write_json(
        &intent_path(cache_base, key, nonce),
        &json!({
            "version": 1,
            "cache_key": key,
            "generation": generation,
            "owner": owner,
        }),
    );
}

fn assert_untouched(cache_dir: &Path) {
    assert!(
        cache_dir.join("index.db").is_file() && cache_dir.join("marker.txt").is_file(),
        "unrelated cache {} was touched",
        cache_dir.display()
    );
}

#[test]
fn crowded_cache_scan_applies_owner_intents_per_cache_and_generation() {
    let temp = TempDir::new().unwrap();
    let cache_base = temp.path().join("cache");
    let _environment = CacheEnvironment::isolated(&cache_base);
    let project_dir = |name: &str| {
        let path = temp.path().join(name);
        fs::create_dir_all(&path).unwrap();
        path
    };
    let fresh_project = project_dir("fresh-project");
    let project = project_dir("project");
    let blocked_project = project_dir("blocked-project");
    let raw = project.to_string_lossy().into_owned();
    let mut unrelated = Vec::new();

    for index in 0..300_u32 {
        let root = format!("/crowded/unrelated/{index}");
        let cache_dir = create_cache(
            &cache_base,
            &root,
            &owner(&root, &root),
            &format!("{}-1-0", 1000 + index),
        );
        if index % 25 == 0 {
            write_intent(
                &cache_base,
                &djb2(&root),
                index,
                "1-1-1",
                owner(&root, &root),
            );
        }
        unrelated.push(cache_dir);
    }

    // An intent from a retired generation names the project; it must not
    // claim that cache for it.
    let retired_root = "/crowded/retired-claim";
    unrelated.push(create_cache(
        &cache_base,
        retired_root,
        &owner(retired_root, retired_root),
        "2000-1-0",
    ));
    write_intent(
        &cache_base,
        &djb2(retired_root),
        1,
        "1999-1-0",
        owner(retired_root, &raw),
    );

    // The manifest is authoritative: stale metadata naming a project does
    // not make this cache a candidate, so SQLite is never consulted.
    let shadow_root = "/crowded/shadow";
    unrelated.push(create_cache(
        &cache_base,
        &db::normalize_root_for_storage(&fresh_project),
        &owner(shadow_root, shadow_root),
        "2001-1-0",
    ));

    let live_root = "/crowded/live";
    unrelated.push(create_cache(
        &cache_base,
        live_root,
        &owner(live_root, live_root),
        "2002-1-0",
    ));
    let live_lease = OpenOptions::new()
        .read(true)
        .write(true)
        .open(leases(&cache_base).join(format!("{}.lock", djb2(live_root))))
        .unwrap();
    fs2::FileExt::lock_shared(&live_lease).unwrap();

    let publishing_root = "/crowded/publishing";
    let publishing = create_cache(
        &cache_base,
        publishing_root,
        &owner(publishing_root, publishing_root),
        "2003-1-0",
    );
    fs::write(publishing.join("index.db.publish-state-v1"), b"{}").unwrap();
    fs::write(publishing.join("index.db.swap"), b"").unwrap();
    unrelated.push(publishing);

    // Lease files and intents of caches that no longer exist.
    for index in 0..50_u32 {
        let gone_root = format!("/crowded/gone/{index}");
        let gone_key = djb2(&gone_root);
        for suffix in ["lock", "publish.lock"] {
            File::create(leases(&cache_base).join(format!("{gone_key}.{suffix}"))).unwrap();
        }
        write_intent(
            &cache_base,
            &gone_key,
            index,
            "3000-1-0",
            owner(&gone_root, &gone_root),
        );
    }
    fs::write(leases(&cache_base).join(".owner-manifest.4242.1.tmp"), b"").unwrap();

    // An unreadable intent fails only its own cache, which SQLite metadata
    // then proves unrelated; the shadow cache is still judged by its manifest.
    let broken_root = "/crowded/broken-intent";
    unrelated.push(create_cache(
        &cache_base,
        broken_root,
        &owner(broken_root, broken_root),
        "2004-1-0",
    ));
    let broken_intent = intent_path(&cache_base, &djb2(broken_root), 1);
    fs::write(&broken_intent, b"{not-json").unwrap();

    drop(db::open_db_leased(&fresh_project).unwrap());
    let fresh_target = cache_base.join(djb2(&db::normalize_root_for_storage(&fresh_project)));
    assert_eq!(
        db::get_db_path(&fresh_project).unwrap(),
        fresh_target.join("index.db")
    );
    assert!(!fresh_target.join("marker.txt").exists());
    for cache_dir in &unrelated {
        assert_untouched(cache_dir);
    }

    // Migration re-reads every intent in `.leases` and refuses to proceed
    // past an unreadable one, so the rest of the scenario runs without it.
    fs::remove_file(&broken_intent).unwrap();

    // The project's cache under an old identity: the manifest alone does not
    // name the project, the current-generation intent does.
    let old_root = format!("{raw}-old-normalized");
    let matching_key = djb2(&old_root);
    let matching = create_cache(
        &cache_base,
        &old_root,
        &owner(&old_root, "/crowded/elsewhere"),
        "4000-1-0",
    );
    write_intent(
        &cache_base,
        &matching_key,
        1,
        "4000-1-0",
        owner(&old_root, &raw),
    );

    let normalized = db::normalize_root_for_storage(&project);
    let target = cache_base.join(djb2(&normalized));
    drop(db::open_db_leased(&project).unwrap());
    assert_eq!(db::get_db_path(&project).unwrap(), target.join("index.db"));
    assert!(
        !matching.exists(),
        "the matching cache must move to the project key"
    );
    assert_eq!(
        fs::read_to_string(target.join("marker.txt")).unwrap(),
        matching_key
    );
    let installed: Value =
        serde_json::from_slice(&fs::read(target.join(OWNER_MANIFEST_NAME)).unwrap()).unwrap();
    assert_eq!(installed["normalized_root"], normalized);
    assert!(installed["known_roots"]
        .as_array()
        .unwrap()
        .iter()
        .any(|root| root.as_str() == Some(old_root.as_str())));
    for cache_dir in &unrelated {
        assert_untouched(cache_dir);
    }
    fs2::FileExt::unlock(&live_lease).unwrap();
    drop(live_lease);

    // An unreadable intent on a cache whose metadata names the project stops
    // resolution instead of creating a second index beside it.
    let blocked_normalized = db::normalize_root_for_storage(&blocked_project);
    let blocked_old_root = format!("{}-old-normalized", blocked_project.to_string_lossy());
    let blocked = create_cache(
        &cache_base,
        &blocked_normalized,
        &owner(&blocked_old_root, "/crowded/elsewhere-blocked"),
        "5000-1-0",
    );
    fs::write(
        intent_path(&cache_base, &djb2(&blocked_old_root), 1),
        b"{not-json",
    )
    .unwrap();

    let error = match db::open_db_leased(&blocked_project) {
        Ok(_) => panic!("an unreadable owner intent was skipped for a matching cache"),
        Err(error) => error,
    };
    assert!(
        format!("{error:#}").contains("failed to validate cache owner"),
        "unexpected error: {error:#}"
    );
    assert_untouched(&blocked);
    assert!(!cache_base
        .join(djb2(&blocked_normalized))
        .join("index.db")
        .exists());
    for cache_dir in &unrelated {
        assert_untouched(cache_dir);
    }
}
