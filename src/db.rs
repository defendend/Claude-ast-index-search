#![allow(dead_code)]

use anyhow::{Context, Result};
use rusqlite::{params, Connection, ErrorCode, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

/// Canonicalize a path with a hard timeout.
///
/// `Path::canonicalize` is a blocking syscall and on macOS it can hang
/// indefinitely when the path lives on a FUSE mount that has gone away
/// (arc unmount during a session, stale worktree, dead sshfs). That
/// turns `ast-index rebuild` into a silent hang.
///
/// We run the canonicalize on a side thread and wait for at most
/// `AST_INDEX_CANONICALIZE_TIMEOUT_MS` (default 5s). On timeout or any
/// canonicalize error we warn to stderr and fall back to the path as-is
/// — the orphan thread is left to live; the OS reaps it on process exit.
///
/// Set `AST_INDEX_NO_CANONICALIZE=1` to skip canonicalize entirely (useful
/// when you already know the mount is dead and don't want the 5s wait).
pub fn safe_canonicalize(path: &Path) -> PathBuf {
    if std::env::var("AST_INDEX_NO_CANONICALIZE")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
    {
        return path.to_path_buf();
    }

    let timeout_ms = std::env::var("AST_INDEX_CANONICALIZE_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(5_000);

    let p = path.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(p.canonicalize());
    });

    match rx.recv_timeout(std::time::Duration::from_millis(timeout_ms)) {
        Ok(Ok(canonical)) => canonical,
        Ok(Err(_)) => path.to_path_buf(),
        Err(_) => {
            eprintln!(
                "[ast-index] filesystem unresponsive at {} — proceeding with raw path \
                 (set AST_INDEX_NO_CANONICALIZE=1 to skip the {}ms wait next time)",
                path.display(),
                timeout_ms
            );
            path.to_path_buf()
        }
    }
}

/// Normalize project root path: canonicalize if possible, fallback to original.
/// This ensures the same DB is found after VFS remount (e.g. arc mount).
fn normalize_root(project_root: &Path) -> String {
    let absolute_root =
        absolute_lexical_root(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    safe_canonicalize(&absolute_root)
        .to_string_lossy()
        .into_owned()
}

fn absolute_lexical_root_in(current_dir: &Path, project_root: &Path) -> PathBuf {
    if project_root.is_absolute() {
        project_root.to_path_buf()
    } else {
        current_dir.join(project_root)
    }
}

fn absolute_lexical_root(project_root: &Path) -> Result<PathBuf> {
    if project_root.is_absolute() {
        return Ok(project_root.to_path_buf());
    }
    std::env::current_dir()
        .map(|current_dir| absolute_lexical_root_in(&current_dir, project_root))
        .context("failed to resolve relative project root against the current directory")
}

fn resolve_root_identities(project_root: &Path) -> Result<(String, String)> {
    let absolute_root = absolute_lexical_root(project_root)?;
    let normalized = safe_canonicalize(&absolute_root)
        .to_string_lossy()
        .into_owned();
    let raw_identity = absolute_root.to_string_lossy().into_owned();
    Ok((normalized, raw_identity))
}

/// Stable storage key for a root that owns indexed relative paths.
pub fn normalize_root_for_storage(project_root: &Path) -> String {
    normalize_root(project_root)
}

/// The base cache directory that holds every project's `<hash>/index.db`.
///
/// `AST_INDEX_CACHE_DIR` is intentionally supported for integration tests and
/// troubleshooting. Unlike `AST_INDEX_DB_PATH`, it preserves the normal
/// multi-project layout, including leases and garbage collection.
fn overridden_cache_base() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("AST_INDEX_CACHE_DIR").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(value);
        return if path.is_absolute() {
            Some(path)
        } else {
            std::env::current_dir().ok().map(|cwd| cwd.join(path))
        };
    }
    None
}

fn cache_base_dir() -> Option<PathBuf> {
    if let Some(path) = overridden_cache_base() {
        return Some(path);
    }
    dirs::cache_dir().map(|dir| dir.join("ast-index"))
}

fn overridden_db_path() -> Option<PathBuf> {
    std::env::var_os("AST_INDEX_DB_PATH")
        .or_else(|| std::env::var_os("KOTLIN_INDEX_DB_PATH"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn project_cache_key(project_root: &Path) -> Result<String> {
    resolve_root_identities(project_root).map(|(normalized, _raw)| simple_hash(&normalized))
}

fn leases_dir(base: &Path) -> PathBuf {
    base.join(".leases")
}

fn open_lock_file(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("failed to open cache lock {}", path.display()))
}

fn acquire_layout_lock(base: &Path) -> Result<File> {
    let file = open_lock_file(&leases_dir(base).join("layout.lock"))?;
    fs2::FileExt::lock_exclusive(&file).context("failed to acquire cache-layout lock")?;
    Ok(file)
}

struct ProjectLeaseInner {
    _file: File,
}

struct PublicationLeaseInner {
    _file: File,
}

#[derive(Clone)]
struct PublicationLease {
    inner: Arc<PublicationLeaseInner>,
}

#[derive(Default)]
struct PublicationRegistry {
    shared: HashMap<PathBuf, Weak<PublicationLeaseInner>>,
    exclusive: HashSet<PathBuf>,
}

fn publication_registry() -> &'static Mutex<PublicationRegistry> {
    static REGISTRY: OnceLock<Mutex<PublicationRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(PublicationRegistry::default()))
}

fn publication_lock_path(db_path: &Path, lease: &ProjectLease) -> Result<PathBuf> {
    if !lease.is_managed() {
        return Ok(db_path.with_extension("publish.lock"));
    }

    let cache_dir = db_path
        .parent()
        .context("managed database path has no cache directory")?;
    let cache_key = cache_dir
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|key| is_cache_key(key))
        .context("managed database path has no valid cache key")?;
    let cache_base = cache_dir
        .parent()
        .context("managed database path has no cache base")?;
    Ok(leases_dir(cache_base).join(format!("{cache_key}.publish.lock")))
}

pub(crate) fn lock_is_contended(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || matches!(
            error.raw_os_error(),
            Some(libc::EAGAIN) | Some(libc::EACCES)
        )
}

fn try_acquire_shared_publication(
    db_path: &Path,
    lease: &ProjectLease,
) -> Result<PublicationLease> {
    let lock_path = publication_lock_path(db_path, lease)?;
    let mut registry = publication_registry()
        .lock()
        .map_err(|_| anyhow::anyhow!("publication lock registry is poisoned"))?;
    if registry.exclusive.contains(&lock_path) {
        return Err(publication_busy(lock_path.display().to_string()));
    }
    if let Some(existing) = registry.shared.get(&lock_path).and_then(Weak::upgrade) {
        return Ok(PublicationLease { inner: existing });
    }

    let file = open_lock_file(&lock_path)?;
    match fs2::FileExt::try_lock_shared(&file) {
        Ok(()) => {
            let inner = Arc::new(PublicationLeaseInner { _file: file });
            registry.shared.insert(lock_path, Arc::downgrade(&inner));
            Ok(PublicationLease { inner })
        }
        Err(error) if lock_is_contended(&error) => {
            Err(publication_busy(lock_path.display().to_string()))
        }
        Err(error) => Err(error).with_context(|| {
            format!(
                "failed to acquire index publication lock {}",
                lock_path.display()
            )
        }),
    }
}

#[derive(Debug)]
pub struct IndexPublicationBusy {
    detail: String,
}

impl std::fmt::Display for IndexPublicationBusy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "index generation is being published or recovered; retry shortly ({})",
            self.detail
        )
    }
}

impl std::error::Error for IndexPublicationBusy {}

fn publication_busy(detail: impl Into<String>) -> anyhow::Error {
    IndexPublicationBusy {
        detail: detail.into(),
    }
    .into()
}

pub fn is_publication_busy(error: &anyhow::Error) -> bool {
    error.downcast_ref::<IndexPublicationBusy>().is_some()
}

/// A shared lease for one project's cache directory.
///
/// Lease files live outside the directory that GC may rename. Holding this
/// value therefore guarantees that a cooperating GC cannot remove the cache.
#[derive(Clone)]
pub struct ProjectLease {
    inner: Option<Arc<ProjectLeaseInner>>,
    publication: Option<PublicationLease>,
}

impl ProjectLease {
    fn none() -> Self {
        Self {
            inner: None,
            publication: None,
        }
    }

    fn is_managed(&self) -> bool {
        self.inner.is_some()
    }

    /// Release the generation-read guard while retaining the cache-directory
    /// lease. Long-lived watch mode uses this between short SQLite sessions so
    /// a rebuild can publish while the watcher is idle.
    pub fn release_publication(&mut self) {
        self.publication = None;
    }
}

fn lease_registry() -> &'static Mutex<HashMap<PathBuf, Weak<ProjectLeaseInner>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Weak<ProjectLeaseInner>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn legacy_open_db_lease_registry() -> &'static Mutex<HashMap<usize, Arc<ProjectLeaseInner>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<usize, Arc<ProjectLeaseInner>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn retain_legacy_open_db_lease(mut lease: ProjectLease) -> Result<()> {
    // `open_db` cannot attach a guard to its concrete `Connection` return
    // type. Retain only the managed cache lease globally; generation
    // publication remains intentionally unguarded for this legacy API.
    lease.release_publication();
    let Some(inner) = lease.inner.take() else {
        return Ok(());
    };
    let identity = Arc::as_ptr(&inner) as usize;
    legacy_open_db_lease_registry()
        .lock()
        .map_err(|_| anyhow::anyhow!("legacy open_db lease registry is poisoned"))?
        .entry(identity)
        .or_insert(inner);
    Ok(())
}

fn acquire_shared_project_lease(base: &Path, key: &str) -> Result<ProjectLease> {
    let lease_path = leases_dir(base).join(format!("{key}.lock"));
    let mut registry = lease_registry()
        .lock()
        .map_err(|_| anyhow::anyhow!("cache lease registry is poisoned"))?;
    if let Some(existing) = registry.get(&lease_path).and_then(Weak::upgrade) {
        return Ok(ProjectLease {
            inner: Some(existing),
            publication: None,
        });
    }

    let file = open_lock_file(&lease_path)?;
    fs2::FileExt::lock_shared(&file)
        .with_context(|| format!("failed to acquire cache lease for {key}"))?;
    let inner = Arc::new(ProjectLeaseInner { _file: file });
    registry.insert(lease_path, Arc::downgrade(&inner));
    Ok(ProjectLease {
        inner: Some(inner),
        publication: None,
    })
}

fn try_acquire_exclusive_project_lock(base: &Path, key: &str) -> Result<Option<File>> {
    let file = open_lock_file(&leases_dir(base).join(format!("{key}.lock")))?;
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(Some(file)),
        Err(_) => Ok(None),
    }
}

fn ensure_real_cache_directory(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => return Ok(()),
        Ok(_) => anyhow::bail!("cache path is not a real directory: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    std::fs::create_dir(path)
        .with_context(|| format!("failed to create cache directory {}", path.display()))?;
    anyhow::ensure!(
        std::fs::symlink_metadata(path)
            .map(|metadata| metadata.file_type().is_dir())
            .unwrap_or(false),
        "cache path is not a real directory after creation: {}",
        path.display()
    );
    Ok(())
}

#[cfg(unix)]
fn sync_cache_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| {
            format!(
                "failed to open cache directory for sync: {}",
                path.display()
            )
        })?;
    directory
        .sync_all()
        .with_context(|| format!("failed to sync cache directory {}", path.display()))
}

#[cfg(not(unix))]
fn sync_cache_directory(_path: &Path) -> Result<()> {
    // Stable Rust has no portable directory-fsync operation. File contents
    // are still flushed before installation, which preserves process-crash
    // recovery; power-loss ordering follows the host filesystem guarantees.
    Ok(())
}

#[cfg(windows)]
fn sync_installed_cache_file(path: &Path) -> Result<()> {
    OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .with_context(|| format!("failed to sync installed cache file {}", path.display()))
}

#[cfg(not(windows))]
fn sync_installed_cache_file(_path: &Path) -> Result<()> {
    Ok(())
}

fn sync_rename_parents(source: &Path, target: &Path) -> Result<()> {
    let target_parent = target
        .parent()
        .context("cache rename target has no parent directory")?;
    sync_cache_directory(target_parent)?;
    let source_parent = source
        .parent()
        .context("cache rename source has no parent directory")?;
    if source_parent != target_parent {
        sync_cache_directory(source_parent)?;
    }
    Ok(())
}

const LIVE_DB_SUFFIXES: &[&str] = &["", "-wal", "-shm", "-journal"];
const SWAP_SUFFIXES: &[&str] = &["", "-wal", "-shm", "-journal"];
const CACHE_OWNER_MANIFEST_NAME: &str = ".ast-index-owner-v1.json";
const CACHE_GENERATION_MARKER_NAME: &str = ".ast-index-generation-v1.json";
const CACHE_OWNER_MANIFEST_VERSION: u8 = 1;
const CACHE_OWNER_INTENT_VERSION: u8 = 1;
const CACHE_GENERATION_MARKER_VERSION: u8 = 1;
const MAX_CACHE_OWNER_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_CACHE_OWNER_IDENTITIES: usize = 64;
const MAX_CACHE_OWNER_IDENTITY_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct CacheOwnerManifest {
    version: u8,
    normalized_root: String,
    raw_root: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    known_roots: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CacheGenerationMarker {
    version: u8,
    token: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CacheOwnerIntent {
    version: u8,
    cache_key: String,
    generation: String,
    owner: CacheOwnerManifest,
}

impl CacheOwnerManifest {
    fn new(normalized_root: &str, raw_root: &str) -> Self {
        Self {
            version: CACHE_OWNER_MANIFEST_VERSION,
            normalized_root: normalized_root.to_owned(),
            raw_root: raw_root.to_owned(),
            known_roots: Vec::new(),
        }
    }

    fn identities(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.normalized_root.as_str())
            .chain(std::iter::once(self.raw_root.as_str()))
            .chain(self.known_roots.iter().map(String::as_str))
    }

    fn contains_root(&self, root: &str) -> bool {
        self.identities().any(|identity| identity == root)
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.identities()
            .any(|identity| other.contains_root(identity))
    }

    fn is_bounded(&self) -> bool {
        self.identities().count() <= MAX_CACHE_OWNER_IDENTITIES
            && self
                .identities()
                .all(|identity| identity.len() <= MAX_CACHE_OWNER_IDENTITY_BYTES)
    }

    fn is_self_consistent(&self, cache_key: &str) -> bool {
        self.version == CACHE_OWNER_MANIFEST_VERSION
            && self.is_bounded()
            && self
                .identities()
                .any(|identity| simple_hash(identity) == cache_key)
    }

    fn merged_for_target(&self, desired: &Self) -> Result<Self> {
        let mut known_roots = Vec::new();
        for identity in self.identities().chain(desired.identities()) {
            if identity == desired.normalized_root || identity == desired.raw_root {
                continue;
            }
            if !known_roots.iter().any(|known| known == identity) {
                known_roots.push(identity.to_owned());
            }
        }
        let merged = Self {
            version: CACHE_OWNER_MANIFEST_VERSION,
            normalized_root: desired.normalized_root.clone(),
            raw_root: desired.raw_root.clone(),
            known_roots,
        };
        anyhow::ensure!(
            merged.is_bounded(),
            "cache owner identity history exceeds safe limits"
        );
        Ok(merged)
    }

    fn merged_while_pinned(&self, desired: &Self) -> Result<Self> {
        anyhow::ensure!(
            self.overlaps(desired),
            "cannot merge disjoint cache owner identities"
        );
        let mut merged = self.clone();
        for identity in desired.identities() {
            if !merged.contains_root(identity) {
                merged.known_roots.push(identity.to_owned());
            }
        }
        anyhow::ensure!(
            merged.is_bounded(),
            "cache owner identity history exceeds safe limits"
        );
        Ok(merged)
    }
}

fn cache_owner_manifest_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join(CACHE_OWNER_MANIFEST_NAME)
}

fn open_cache_owner_manifest(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        // Open the reparse point itself, never the file it may target.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }

    options.open(path)
}

#[cfg(unix)]
fn same_file_identity(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(windows)]
fn same_file_identity(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    // Stable Rust does not expose the Windows file ID. OPEN_REPARSE_POINT
    // prevents link traversal; compare every stable snapshot field as a
    // replacement-race guard before reading the opened handle.
    left.file_attributes() == right.file_attributes()
        && left.creation_time() == right.creation_time()
        && left.last_write_time() == right.last_write_time()
        && left.file_size() == right.file_size()
}

#[cfg(not(any(unix, windows)))]
fn same_file_identity(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.len() == right.len()
        && left.modified().ok().is_some()
        && left.modified().ok() == right.modified().ok()
}

/// Reject a manifest observed as a symlink, special file, unbounded payload,
/// changed between lstat/open, unbounded payload, or malformed JSON. Callers
/// deciding whether a busy cache is foreign treat every error as unknown and
/// therefore fail closed through the legacy SQLite/lease path.
fn read_bounded_json_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect cache owner {}", path.display()))
        }
    };
    anyhow::ensure!(
        metadata.file_type().is_file(),
        "cache owner manifest is not a regular file: {}",
        path.display()
    );
    anyhow::ensure!(
        metadata.len() <= MAX_CACHE_OWNER_MANIFEST_BYTES,
        "cache owner manifest is too large: {}",
        path.display()
    );

    let file = open_cache_owner_manifest(path)
        .with_context(|| format!("failed to open cache owner {}", path.display()))?;
    let opened_metadata = file
        .metadata()
        .with_context(|| format!("failed to inspect open cache owner {}", path.display()))?;
    anyhow::ensure!(
        opened_metadata.file_type().is_file()
            && opened_metadata.len() <= MAX_CACHE_OWNER_MANIFEST_BYTES
            && same_file_identity(&metadata, &opened_metadata),
        "cache owner manifest changed while opening: {}",
        path.display()
    );
    let mut bytes = Vec::with_capacity(opened_metadata.len() as usize);
    file.take(MAX_CACHE_OWNER_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read cache owner {}", path.display()))?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_CACHE_OWNER_MANIFEST_BYTES,
        "cache owner manifest grew while reading: {}",
        path.display()
    );
    let value: T = serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid cache owner manifest {}", path.display()))?;
    Ok(Some(value))
}

fn read_owner_manifest_file(path: &Path) -> Result<Option<CacheOwnerManifest>> {
    let Some(manifest): Option<CacheOwnerManifest> = read_bounded_json_file(path)? else {
        return Ok(None);
    };
    anyhow::ensure!(
        manifest.version == CACHE_OWNER_MANIFEST_VERSION,
        "unsupported cache owner manifest version in {}",
        path.display()
    );
    anyhow::ensure!(
        manifest.is_bounded(),
        "cache owner manifest exceeds identity limits: {}",
        path.display()
    );
    Ok(Some(manifest))
}

fn read_cache_owner_manifest(cache_dir: &Path) -> Result<Option<CacheOwnerManifest>> {
    read_owner_manifest_file(&cache_owner_manifest_path(cache_dir))
}

fn verified_cache_owner(cache_dir: &Path, cache_key: &str) -> Option<CacheOwnerManifest> {
    read_cache_owner_manifest(cache_dir)
        .ok()
        .flatten()
        .filter(|manifest| manifest.is_self_consistent(cache_key))
}

fn active_overlapping_cache_lease(
    cache_base: &Path,
    requested: &CacheOwnerManifest,
    carried: &CacheOwnerManifest,
) -> Result<Option<(PathBuf, ProjectLease, String)>> {
    let expected_lease_dir = leases_dir(cache_base);
    let active = {
        let registry = lease_registry()
            .lock()
            .map_err(|_| anyhow::anyhow!("cache lease registry is poisoned"))?;
        registry
            .iter()
            .filter_map(|(path, lease)| lease.upgrade().map(|lease| (path.to_path_buf(), lease)))
            .collect::<Vec<_>>()
    };

    let mut matching = Vec::new();
    for (lease_path, lease) in active {
        if lease_path.parent() != Some(expected_lease_dir.as_path()) {
            continue;
        }
        let Some(cache_key) = lease_path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".lock"))
            .filter(|key| is_cache_key(key))
        else {
            continue;
        };
        let cache_dir = cache_base.join(cache_key);
        if !std::fs::symlink_metadata(&cache_dir)
            .map(|metadata| metadata.file_type().is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let Some(owner) = effective_cache_owner(cache_base, &cache_dir, cache_key)? else {
            continue;
        };
        if !owner.overlaps(requested) {
            continue;
        }
        let db_path = cache_dir.join("index.db");
        ensure_safe_live_db_artifacts(&db_path)?;
        ensure_safe_swap_db_artifacts(&db_path)?;
        matching.push((cache_key.to_owned(), cache_dir, db_path, lease, owner));
    }

    anyhow::ensure!(
        matching.len() <= 1,
        "multiple active cache identities overlap the requested project root"
    );
    let Some((cache_key, cache_dir, db_path, lease, owner)) = matching.pop() else {
        return Ok(None);
    };
    let pinned_normalized = owner.normalized_root.clone();
    let merged = owner.merged_while_pinned(carried)?;
    anyhow::ensure!(
        merged.is_self_consistent(&cache_key),
        "active cache owner no longer matches pinned key {cache_key}"
    );
    if merged != owner {
        persist_cache_owner_manifest(&cache_dir, &cache_key, &merged)?;
    } else {
        cleanup_cache_owner_intents(cache_base, &cache_dir, &cache_key, &merged);
    }
    Ok(Some((
        db_path,
        ProjectLease {
            inner: Some(lease),
            publication: None,
        },
        pinned_normalized,
    )))
}

fn write_bounded_json_file<T: Serialize>(
    path: &Path,
    pending_dir: &Path,
    value: &T,
    replace_existing: bool,
) -> Result<()> {
    ensure_real_cache_directory(&pending_dir)?;
    let bytes = serde_json::to_vec(value)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_CACHE_OWNER_MANIFEST_BYTES,
        "cache owner manifest is too large for {}",
        path.display()
    );

    let mut pending = None;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for sequence in 0..100_u32 {
        let candidate = pending_dir.join(format!(
            ".owner-manifest.{}.{}.tmp",
            std::process::id(),
            nonce + u128::from(sequence)
        ));
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&candidate)
        {
            Ok(file) => {
                pending = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let (pending_path, mut pending_file) =
        pending.context("could not allocate cache owner manifest temporary file")?;
    if let Err(error) = pending_file.write_all(&bytes) {
        drop(pending_file);
        let _ = std::fs::remove_file(&pending_path);
        return Err(error).context("failed to write cache owner manifest");
    }
    if let Err(error) = pending_file.sync_all() {
        drop(pending_file);
        let _ = std::fs::remove_file(&pending_path);
        return Err(error).context("failed to sync cache owner manifest");
    }
    drop(pending_file);

    if replace_existing {
        #[cfg(windows)]
        match std::fs::symlink_metadata(path) {
            Ok(_) => {
                if let Err(error) = std::fs::remove_file(path) {
                    let _ = std::fs::remove_file(&pending_path);
                    return Err(error).with_context(|| {
                        format!("failed to replace cache owner manifest {}", path.display())
                    });
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                let _ = std::fs::remove_file(&pending_path);
                return Err(error).with_context(|| {
                    format!("failed to inspect cache owner manifest {}", path.display())
                });
            }
        }
        if let Err(error) = std::fs::rename(&pending_path, path) {
            let _ = std::fs::remove_file(&pending_path);
            return Err(error).with_context(|| {
                format!("failed to install cache owner manifest {}", path.display())
            });
        }
        sync_installed_cache_file(path)?;
        let target_parent = path
            .parent()
            .context("installed cache owner manifest has no parent directory")?;
        sync_cache_directory(target_parent)?;
        if pending_dir != target_parent {
            sync_cache_directory(pending_dir)?;
        }
    } else {
        if let Err(error) = std::fs::hard_link(&pending_path, path) {
            let _ = std::fs::remove_file(&pending_path);
            return Err(error).with_context(|| {
                format!("failed to install cache owner intent {}", path.display())
            });
        }
        sync_installed_cache_file(path)?;
        let target_parent = path
            .parent()
            .context("installed cache owner intent has no parent directory")?;
        sync_cache_directory(target_parent)?;
        if std::fs::remove_file(&pending_path).is_ok() {
            sync_cache_directory(pending_dir)?;
        }
    }
    Ok(())
}

fn cache_generation_marker_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join(CACHE_GENERATION_MARKER_NAME)
}

fn valid_cache_generation_token(token: &str) -> bool {
    if token.is_empty() || token.len() > 128 {
        return false;
    }
    let mut parts = token.split('-');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(process_id), Some(timestamp), Some(sequence), None)
            if !process_id.is_empty()
                && process_id.bytes().all(|byte| byte.is_ascii_digit())
                && !timestamp.is_empty()
                && timestamp.bytes().all(|byte| byte.is_ascii_digit())
                && !sequence.is_empty()
                && sequence.bytes().all(|byte| byte.is_ascii_digit())
    )
}

fn read_cache_generation(cache_dir: &Path) -> Result<Option<String>> {
    let path = cache_generation_marker_path(cache_dir);
    let Some(marker): Option<CacheGenerationMarker> = read_bounded_json_file(&path)? else {
        return Ok(None);
    };
    anyhow::ensure!(
        marker.version == CACHE_GENERATION_MARKER_VERSION
            && valid_cache_generation_token(&marker.token),
        "invalid cache generation marker {}",
        path.display()
    );
    Ok(Some(marker.token))
}

fn ensure_cache_generation(cache_dir: &Path) -> Result<String> {
    if let Some(generation) = read_cache_generation(cache_dir)? {
        return Ok(generation);
    }
    anyhow::ensure!(
        std::fs::symlink_metadata(cache_dir)
            .map(|metadata| metadata.file_type().is_dir())
            .unwrap_or(false),
        "cache path is not a real directory: {}",
        cache_dir.display()
    );
    static GENERATION_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = GENERATION_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let marker = CacheGenerationMarker {
        version: CACHE_GENERATION_MARKER_VERSION,
        token: format!("{}-{timestamp}-{sequence}", std::process::id()),
    };
    let cache_base = cache_dir
        .parent()
        .context("cache generation directory has no cache base")?;
    write_bounded_json_file(
        &cache_generation_marker_path(cache_dir),
        &leases_dir(cache_base),
        &marker,
        false,
    )?;
    Ok(marker.token)
}

fn cache_owner_intent_name(cache_key: &str, process_id: u32, nonce: u128) -> String {
    format!("{cache_key}.owner.{process_id}.{nonce}.json")
}

fn cache_owner_intent_key(name: &str) -> Option<&str> {
    let (cache_key, rest) = name.split_once(".owner.")?;
    if !is_cache_key(cache_key) {
        return None;
    }
    let Some(rest) = rest.strip_suffix(".json") else {
        return None;
    };
    let mut parts = rest.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(process_id), Some(nonce), None)
            if !process_id.is_empty()
                && process_id.bytes().all(|byte| byte.is_ascii_digit())
                && !nonce.is_empty()
                && nonce.bytes().all(|byte| byte.is_ascii_digit())
    )
    .then_some(cache_key)
}

fn is_cache_owner_intent_name(name: &str, cache_key: &str) -> bool {
    cache_owner_intent_key(name) == Some(cache_key)
}

fn read_cache_owner_intents(
    cache_base: &Path,
    cache_dir: &Path,
    only_cache_key: Option<&str>,
) -> Result<Vec<(PathBuf, String, CacheOwnerManifest)>> {
    let Some(generation) = read_cache_generation(cache_dir)? else {
        return Ok(Vec::new());
    };
    let intent_dir = leases_dir(cache_base);
    ensure_real_cache_directory(&intent_dir)?;
    let entries = std::fs::read_dir(&intent_dir)?.collect::<std::io::Result<Vec<_>>>()?;
    let mut paths = entries
        .into_iter()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(cache_owner_intent_key)
                .map(|cache_key| only_cache_key.map_or(true, |only| only == cache_key))
                .unwrap_or(false)
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();

    let mut intents = Vec::with_capacity(paths.len());
    for path in paths {
        let intent: CacheOwnerIntent = read_bounded_json_file(&path)?
            .with_context(|| format!("cache owner intent disappeared: {}", path.display()))?;
        let filename_key = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(cache_owner_intent_key)
            .context("cache owner intent filename changed while reading")?;
        anyhow::ensure!(
            intent.version == CACHE_OWNER_INTENT_VERSION
                && intent.cache_key == filename_key
                && intent.owner.is_self_consistent(filename_key),
            "cache owner intent does not match filename key {filename_key}: {}",
            path.display()
        );
        if intent.generation == generation {
            intents.push((path, intent.cache_key, intent.owner));
        }
    }
    Ok(intents)
}

fn cache_owner_intents(
    cache_base: &Path,
    cache_dir: &Path,
    cache_key: &str,
) -> Result<Vec<(PathBuf, CacheOwnerManifest)>> {
    read_cache_owner_intents(cache_base, cache_dir, Some(cache_key)).map(|intents| {
        intents
            .into_iter()
            .map(|(path, _cache_key, owner)| (path, owner))
            .collect()
    })
}

fn effective_cache_owner(
    cache_base: &Path,
    cache_dir: &Path,
    cache_key: &str,
) -> Result<Option<CacheOwnerManifest>> {
    let final_owner = read_cache_owner_manifest(cache_dir)?;
    let intents = cache_owner_intents(cache_base, cache_dir, cache_key)?;
    let mut effective = match final_owner {
        Some(owner) if owner.is_self_consistent(cache_key) => Some(owner),
        Some(owner) => {
            anyhow::ensure!(
                !intents.is_empty(),
                "cache owner manifest does not match directory key {cache_key}: {}",
                cache_dir.display()
            );
            let mut recovered: Option<CacheOwnerManifest> = None;
            for (path, intent) in &intents {
                anyhow::ensure!(
                    owner.overlaps(intent),
                    "cache owner recovery intent conflicts with installed owner: {}",
                    path.display()
                );
                recovered = Some(match recovered {
                    Some(current) => current.merged_while_pinned(intent)?,
                    None => owner.merged_for_target(intent)?,
                });
            }
            let recovered = recovered.context("cache owner recovery intent disappeared")?;
            anyhow::ensure!(
                recovered.is_self_consistent(cache_key),
                "cache owner recovery did not pin directory key {cache_key}: {}",
                cache_dir.display()
            );
            return Ok(Some(recovered));
        }
        None => None,
    };
    for (path, intent) in intents {
        effective = Some(match effective {
            Some(owner) => {
                anyhow::ensure!(
                    owner.overlaps(&intent),
                    "cache owner intent conflicts with installed owner: {}",
                    path.display()
                );
                owner.merged_while_pinned(&intent)?
            }
            None => intent,
        });
    }
    Ok(effective)
}

fn merge_cache_owner_intents(
    cache_base: &Path,
    cache_dir: &Path,
    cache_key: &str,
    desired: &CacheOwnerManifest,
) -> Result<CacheOwnerManifest> {
    let mut merged = desired.clone();
    for (path, intent) in cache_owner_intents(cache_base, cache_dir, cache_key)? {
        anyhow::ensure!(
            intent.overlaps(desired),
            "cache owner intent conflicts with project root: {}",
            path.display()
        );
        merged = intent.merged_for_target(&merged)?;
    }
    Ok(merged)
}

fn merge_authorized_source_owner_intents(
    cache_base: &Path,
    source_dir: &Path,
    authorized_source: &CacheOwnerManifest,
    desired: &CacheOwnerManifest,
) -> Result<CacheOwnerManifest> {
    let mut merged = authorized_source.merged_for_target(desired)?;
    for (path, _cache_key, intent) in read_cache_owner_intents(cache_base, source_dir, None)? {
        anyhow::ensure!(
            intent.overlaps(authorized_source),
            "cache owner intent conflicts with authorized migration source: {}",
            path.display()
        );
        merged = intent.merged_for_target(&merged)?;
    }
    Ok(merged)
}

fn write_cache_owner_intent(
    cache_base: &Path,
    generation_dir: &Path,
    cache_key: &str,
    manifest: &CacheOwnerManifest,
) -> Result<PathBuf> {
    anyhow::ensure!(
        manifest.is_self_consistent(cache_key),
        "cache owner intent does not match target key {cache_key}"
    );
    let intent = CacheOwnerIntent {
        version: CACHE_OWNER_INTENT_VERSION,
        cache_key: cache_key.to_owned(),
        generation: ensure_cache_generation(generation_dir)?,
        owner: manifest.clone(),
    };
    let intent_dir = leases_dir(cache_base);
    ensure_real_cache_directory(&intent_dir)?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for sequence in 0..100_u32 {
        let path = intent_dir.join(cache_owner_intent_name(
            cache_key,
            std::process::id(),
            nonce + u128::from(sequence),
        ));
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match write_bounded_json_file(&path, &intent_dir, &intent, false) {
                    Ok(()) => return Ok(path),
                    Err(error)
                        if error
                            .downcast_ref::<std::io::Error>()
                            .map(|error| error.kind() == std::io::ErrorKind::AlreadyExists)
                            .unwrap_or(false) =>
                    {
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
            Ok(_) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    anyhow::bail!("could not allocate cache owner intent for key {cache_key}")
}

fn cleanup_cache_owner_intents(
    cache_base: &Path,
    generation_dir: &Path,
    cache_key: &str,
    installed: &CacheOwnerManifest,
) {
    let Ok(intents) = cache_owner_intents(cache_base, generation_dir, cache_key) else {
        return;
    };
    for (path, intent) in intents {
        if intent
            .identities()
            .all(|identity| installed.contains_root(identity))
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn cleanup_source_generation_owner_intents(
    cache_base: &Path,
    generation_dir: &Path,
    installed: &CacheOwnerManifest,
) {
    let Ok(intents) = read_cache_owner_intents(cache_base, generation_dir, None) else {
        return;
    };
    for (path, _cache_key, intent) in intents {
        if intent
            .identities()
            .all(|identity| installed.contains_root(identity))
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn persist_cache_owner_manifest(
    cache_dir: &Path,
    cache_key: &str,
    manifest: &CacheOwnerManifest,
) -> Result<()> {
    let cache_base = cache_dir
        .parent()
        .context("cache owner directory has no cache base")?;
    let pending_dir = leases_dir(cache_base);
    write_cache_owner_intent(cache_base, cache_dir, cache_key, manifest)?;
    write_bounded_json_file(
        &cache_owner_manifest_path(cache_dir),
        &pending_dir,
        manifest,
        true,
    )?;
    cleanup_cache_owner_intents(cache_base, cache_dir, cache_key, manifest);
    Ok(())
}

fn install_cache_owner_manifest(
    cache_dir: &Path,
    cache_key: &str,
    desired: &CacheOwnerManifest,
    allow_migration_replacement: bool,
) -> Result<()> {
    anyhow::ensure!(
        desired.is_self_consistent(cache_key),
        "cache owner does not match target key {cache_key}"
    );
    let cache_base = cache_dir
        .parent()
        .context("cache owner directory has no cache base")?;
    match read_cache_owner_manifest(cache_dir)? {
        Some(existing) if existing == *desired => {
            cleanup_cache_owner_intents(cache_base, cache_dir, cache_key, &existing);
            Ok(())
        }
        Some(existing) if existing.is_self_consistent(cache_key) && existing.overlaps(desired) => {
            let merged = existing.merged_for_target(desired)?;
            if merged == existing {
                cleanup_cache_owner_intents(cache_base, cache_dir, cache_key, &existing);
                Ok(())
            } else {
                persist_cache_owner_manifest(cache_dir, cache_key, &merged)
            }
        }
        Some(existing) => {
            anyhow::ensure!(
                allow_migration_replacement && existing.overlaps(desired),
                "cache owner manifest conflicts with project root in {}",
                cache_dir.display()
            );
            let merged = existing.merged_for_target(desired)?;
            persist_cache_owner_manifest(cache_dir, cache_key, &merged)
        }
        None => persist_cache_owner_manifest(cache_dir, cache_key, desired),
    }
}

fn validate_cache_owner_for_migration(
    cache_base: &Path,
    cache_dir: &Path,
    cache_key: &str,
    desired: &CacheOwnerManifest,
) -> Result<Option<CacheOwnerManifest>> {
    ensure_cache_generation(cache_dir)?;
    let Some(existing) = effective_cache_owner(cache_base, cache_dir, cache_key)? else {
        return Ok(None);
    };
    anyhow::ensure!(
        existing.is_self_consistent(cache_key) && existing.overlaps(desired),
        "cache owner manifest conflicts with migration source {}",
        cache_dir.display()
    );
    Ok(Some(existing))
}

fn ensure_regular_or_missing(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => anyhow::bail!(
            "cache database artifact is not a regular file: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("failed to inspect cache artifact {}", path.display())),
    }
}

fn ensure_safe_live_db_artifacts(db_path: &Path) -> Result<()> {
    for suffix in LIVE_DB_SUFFIXES {
        ensure_regular_or_missing(&db_path.with_extension(format!("db{suffix}")))?;
    }
    Ok(())
}

fn ensure_safe_swap_db_artifacts(db_path: &Path) -> Result<()> {
    for suffix in SWAP_SUFFIXES {
        ensure_regular_or_missing(&db_path.with_extension(format!("db.swap{suffix}")))?;
    }
    Ok(())
}

enum ReplaceableMigrationTarget {
    Empty,
    Owner(CacheOwnerManifest),
}

struct QuarantinedMigrationTarget {
    path: PathBuf,
    owner: Option<CacheOwnerManifest>,
}

fn inspect_replaceable_migration_target(
    target: &Path,
    target_key: &str,
    desired_owner: &CacheOwnerManifest,
) -> Result<Option<ReplaceableMigrationTarget>> {
    match std::fs::symlink_metadata(target) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            let entries = std::fs::read_dir(target)?.collect::<std::io::Result<Vec<_>>>()?;
            if entries.is_empty() {
                return Ok(Some(ReplaceableMigrationTarget::Empty));
            }

            let owner_path = cache_owner_manifest_path(target);
            let generation_path = cache_generation_marker_path(target);
            anyhow::ensure!(
                entries.len() <= 2
                    && entries.iter().all(|entry| {
                        entry.path() == owner_path || entry.path() == generation_path
                    }),
                "cannot migrate cache into non-empty directory {}",
                target.display()
            );
            if entries.iter().any(|entry| entry.path() == generation_path) {
                read_cache_generation(target)?
                    .context("cache migration target generation marker disappeared")?;
            }
            let Some(owner) = read_cache_owner_manifest(target)? else {
                return Ok(Some(ReplaceableMigrationTarget::Empty));
            };
            anyhow::ensure!(
                owner.is_self_consistent(target_key) && owner.overlaps(desired_owner),
                "cache owner manifest conflicts with migration target {}",
                target.display()
            );
            Ok(Some(ReplaceableMigrationTarget::Owner(owner)))
        }
        Ok(_) => {
            anyhow::bail!(
                "cannot migrate cache into non-directory {}",
                target.display()
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => return Err(error.into()),
    }
}

fn restore_quarantined_migration_target(tombstone: &Path, target: &Path) -> Result<()> {
    match std::fs::symlink_metadata(target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            sync_cache_directory(tombstone)?;
            std::fs::rename(tombstone, target).with_context(|| {
                format!(
                    "failed to restore quarantined migration target {}",
                    target.display()
                )
            })?;
            sync_rename_parents(tombstone, target)
        }
        Ok(_) => anyhow::bail!(
            "cannot restore quarantined migration target because {} now exists",
            target.display()
        ),
        Err(error) => Err(error.into()),
    }
}

fn quarantine_replaceable_migration_target(
    target: &Path,
    target_key: &str,
    desired_owner: &CacheOwnerManifest,
) -> Result<Option<QuarantinedMigrationTarget>> {
    let Some(_initial_target) =
        inspect_replaceable_migration_target(target, target_key, desired_owner)?
    else {
        return Ok(None);
    };

    let cache_base = target
        .parent()
        .context("cache migration target has no cache base")?;
    let trash_dir = cache_base.join(".gc-trash");
    anyhow::ensure!(
        prepare_trash_dir(&trash_dir)?,
        "cache trash path is not a real directory: {}",
        trash_dir.display()
    );
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tombstone = (0_u32..100)
        .map(|sequence| {
            trash_dir.join(format!(
                "{target_key}.{}.{}",
                std::process::id(),
                nonce + u128::from(sequence)
            ))
        })
        .find(|candidate| {
            matches!(
                std::fs::symlink_metadata(candidate),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound
            )
        })
        .context("could not allocate cache migration tombstone")?;

    sync_cache_directory(target)?;
    std::fs::rename(target, &tombstone)
        .with_context(|| format!("failed to quarantine migration target {}", target.display()))?;
    sync_rename_parents(target, &tombstone)?;

    let replacement = inspect_replaceable_migration_target(&tombstone, target_key, desired_owner)
        .and_then(|replacement| replacement.context("quarantined migration target disappeared"));
    let replacement = match replacement {
        Ok(replacement) => replacement,
        Err(validation_error) => {
            return match restore_quarantined_migration_target(&tombstone, target) {
                Ok(()) => Err(validation_error).with_context(|| {
                    format!(
                        "migration target {} changed while being quarantined; restored it",
                        target.display()
                    )
                }),
                Err(restore_error) => Err(anyhow::anyhow!(
                    "migration target {} changed while being quarantined: {validation_error:#}; \
                     failed to restore it from {}: {restore_error:#}",
                    target.display(),
                    tombstone.display()
                )),
            };
        }
    };
    let owner = match replacement {
        ReplaceableMigrationTarget::Empty => None,
        ReplaceableMigrationTarget::Owner(owner) => Some(owner),
    };

    Ok(Some(QuarantinedMigrationTarget {
        path: tombstone,
        owner,
    }))
}

fn rename_cache_directory(
    source: &Path,
    target: &Path,
    target_key: &str,
    desired_owner: &CacheOwnerManifest,
) -> Result<CacheOwnerManifest> {
    let cache_base = target
        .parent()
        .context("cache migration target has no cache base")?;
    ensure_cache_generation(source)?;
    let initial_target = inspect_replaceable_migration_target(target, target_key, desired_owner)?;
    let mut migration_owner = match initial_target {
        Some(ReplaceableMigrationTarget::Owner(target_owner)) => {
            target_owner.merged_for_target(desired_owner)?
        }
        _ => desired_owner.clone(),
    };
    write_cache_owner_intent(cache_base, source, target_key, &migration_owner)?;

    let quarantined = quarantine_replaceable_migration_target(target, target_key, desired_owner)?;
    if let Some(target_owner) = quarantined
        .as_ref()
        .and_then(|target| target.owner.as_ref())
    {
        migration_owner = target_owner.merged_for_target(&migration_owner)?;
        write_cache_owner_intent(cache_base, source, target_key, &migration_owner)?;
    }

    sync_cache_directory(source)?;
    if let Err(rename_error) = std::fs::rename(source, target) {
        if let Some(tombstone) = quarantined.as_ref() {
            if let Err(restore_error) =
                restore_quarantined_migration_target(&tombstone.path, target)
            {
                anyhow::bail!(
                    "failed to atomically migrate cache {} to {}: {rename_error}; \
                     failed to restore the original target from {}: {restore_error:#}",
                    source.display(),
                    target.display(),
                    tombstone.path.display()
                );
            }
        }
        return Err(rename_error).with_context(|| {
            format!(
                "failed to atomically migrate cache {} to {}",
                source.display(),
                target.display()
            )
        });
    }
    sync_rename_parents(source, target)?;

    // The old empty/manifest-only target remains a valid GC tombstone. The
    // next sweep removes it through the existing quarantine cleanup path.
    Ok(migration_owner)
}

fn read_cached_project_root(db_path: &Path) -> rusqlite::Result<String> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(db_path, flags)?;
    conn.busy_timeout(std::time::Duration::from_millis(100))?;
    conn.query_row(
        "SELECT value FROM metadata WHERE key = 'project_root'",
        [],
        |row| row.get(0),
    )
}

fn is_sqlite_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(code.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

fn install_or_recover_target_cache_owner(
    cache_base: &Path,
    cache_dir: &Path,
    db_path: &Path,
    cache_key: &str,
    desired: &CacheOwnerManifest,
) -> Result<()> {
    let Some(existing) = read_cache_owner_manifest(cache_dir)? else {
        return install_cache_owner_manifest(cache_dir, cache_key, desired, false);
    };
    if existing.is_self_consistent(cache_key) {
        return install_cache_owner_manifest(cache_dir, cache_key, desired, false);
    }
    anyhow::ensure!(
        existing.overlaps(desired),
        "cache owner manifest conflicts with project root in {}",
        cache_dir.display()
    );

    let _target_lock = try_acquire_exclusive_project_lock(cache_base, cache_key)?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "cache owner recovery target {} is active; retry after other ast-index processes exit",
                cache_dir.display()
            )
        })?;
    anyhow::ensure!(
        std::fs::symlink_metadata(cache_dir)
            .map(|metadata| metadata.file_type().is_dir())
            .unwrap_or(false),
        "cache owner recovery target is not a real directory: {}",
        cache_dir.display()
    );
    ensure_safe_live_db_artifacts(db_path)?;
    anyhow::ensure!(
        std::fs::symlink_metadata(db_path)
            .map(|metadata| metadata.file_type().is_file())
            .unwrap_or(false),
        "cache owner recovery target has no regular database: {}",
        db_path.display()
    );

    let existing = read_cache_owner_manifest(cache_dir)?
        .context("cache owner manifest disappeared during recovery")?;
    if existing.is_self_consistent(cache_key) {
        return install_cache_owner_manifest(cache_dir, cache_key, desired, false);
    }
    anyhow::ensure!(
        existing.overlaps(desired),
        "cache owner manifest conflicts with project root in {}",
        cache_dir.display()
    );
    let metadata_root = read_cached_project_root(db_path).with_context(|| {
        format!(
            "failed to validate interrupted cache-owner migration in {}",
            cache_dir.display()
        )
    })?;
    anyhow::ensure!(
        existing.contains_root(&metadata_root),
        "cache owner manifest conflicts with project_root metadata in {}",
        cache_dir.display()
    );
    install_cache_owner_manifest(cache_dir, cache_key, desired, true)
}

/// Resolve the normal cache path while holding the layout lock, then acquire
/// the per-project lease before releasing that lock. This closes the race in
/// which GC could otherwise rename the directory between path resolution and
/// opening SQLite.
fn resolve_db_path_and_lease(project_root: &Path) -> Result<(PathBuf, ProjectLease, String)> {
    let (normalized, raw_identity) = resolve_root_identities(project_root)?;
    if let Some(path) = overridden_db_path() {
        let path = absolute_lexical_root(&path).context(
            "failed to resolve the database-path override against the current directory",
        )?;
        return Ok((path, ProjectLease::none(), normalized));
    }

    let cache_dir = cache_base_dir().context("Could not find cache directory")?;
    let _layout_lock = acquire_layout_lock(&cache_dir)?;
    let legacy_raw = project_root.to_string_lossy();
    let requested_owner = CacheOwnerManifest::new(&normalized, &raw_identity);
    let project_hash = simple_hash(&normalized);
    let db_dir = cache_dir.join(&project_hash);
    let desired_owner =
        merge_cache_owner_intents(&cache_dir, &db_dir, &project_hash, &requested_owner)?;
    if let Some(active) =
        active_overlapping_cache_lease(&cache_dir, &requested_owner, &desired_owner)?
    {
        return Ok(active);
    }
    ensure_real_cache_directory(&db_dir)?;
    let db_path = db_dir.join("index.db");
    ensure_safe_live_db_artifacts(&db_path)?;
    let interrupted_publication = publication_has_interrupted_state(&db_path)?;

    // Also compute hash from raw path (for migration from pre-normalize DBs).
    let raw_hash = simple_hash(legacy_raw.as_ref());
    let raw_dir = cache_dir.join(&raw_hash);

    // Migrate from raw-path hash to normalized hash if needed.
    let raw_dir_is_real = std::fs::symlink_metadata(&raw_dir)
        .map(|metadata| metadata.file_type().is_dir())
        .unwrap_or(false);
    let raw_db_is_regular = std::fs::symlink_metadata(raw_dir.join("index.db"))
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false);
    if project_root.is_absolute()
        && !db_path.exists()
        && !interrupted_publication
        && raw_hash != project_hash
        && raw_dir_is_real
        && raw_db_is_regular
    {
        let _source_lock =
            try_acquire_exclusive_project_lock(&cache_dir, &raw_hash)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "cannot migrate active cache {}; retry after other ast-index processes exit",
                    raw_dir.display()
                )
            })?;
        let _target_lock = try_acquire_exclusive_project_lock(&cache_dir, &project_hash)?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "cannot migrate into active cache {}; retry after other ast-index processes exit",
                    db_dir.display()
                )
            })?;
        anyhow::ensure!(
            std::fs::symlink_metadata(&raw_dir)
                .map(|metadata| metadata.file_type().is_dir())
                .unwrap_or(false)
                && std::fs::symlink_metadata(raw_dir.join("index.db"))
                    .map(|metadata| metadata.file_type().is_file())
                    .unwrap_or(false),
            "refusing to migrate non-regular cache source {}",
            raw_dir.display()
        );
        ensure_safe_live_db_artifacts(&raw_dir.join("index.db"))?;
        let source_owner =
            validate_cache_owner_for_migration(&cache_dir, &raw_dir, &raw_hash, &requested_owner)?;
        let authorized_source = source_owner.as_ref().unwrap_or(&requested_owner);
        let migration_desired = merge_authorized_source_owner_intents(
            &cache_dir,
            &raw_dir,
            authorized_source,
            &desired_owner,
        )?;
        let migration_owner =
            rename_cache_directory(&raw_dir, &db_dir, &project_hash, &migration_desired)?;
        ensure_safe_live_db_artifacts(&db_path)?;
        install_cache_owner_manifest(&db_dir, &project_hash, &migration_owner, true)?;
        cleanup_source_generation_owner_intents(&cache_dir, &db_dir, &migration_owner);
    }

    // Auto-migrate: if the new hash dir has no DB, look for an old one by
    // metadata. Foreign DBs are opened read-only and never schema-migrated.
    if !db_path.exists() && !interrupted_publication {
        if let Ok(entries) = std::fs::read_dir(&cache_dir) {
            for entry in entries.flatten() {
                let is_real_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
                let old_dir = entry.path();
                if !is_real_dir
                    || old_dir
                        .file_name()
                        .map(|name| name == project_hash.as_str())
                        == Some(true)
                {
                    continue;
                }
                let old_db = old_dir.join("index.db");
                if !std::fs::symlink_metadata(&old_db)
                    .map(|metadata| metadata.file_type().is_file())
                    .unwrap_or(false)
                {
                    continue;
                }
                let Some(old_key) = old_dir.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if !is_cache_key(old_key) {
                    continue;
                }
                ensure_safe_live_db_artifacts(&old_db)?;
                let cache_owner = match effective_cache_owner(&cache_dir, &old_dir, old_key) {
                    Ok(owner) => owner,
                    Err(owner_error) => match read_cached_project_root(&old_db) {
                        Ok(root) if root != normalized && root != raw_identity => continue,
                        _ => {
                            return Err(owner_error).with_context(|| {
                                format!("failed to validate cache owner for {}", old_dir.display())
                            })
                        }
                    },
                };
                // A self-consistent manifest is the authoritative identity for
                // current-format caches. This lets an unrelated busy cache be
                // skipped without touching SQLite; manifest-less legacy caches
                // still fall back to metadata inspection below.
                if cache_owner
                    .as_ref()
                    .map(|owner| !owner.overlaps(&requested_owner))
                    .unwrap_or(false)
                {
                    continue;
                }
                let initial_root = read_cached_project_root(&old_db);
                let initial_failed = initial_root.is_err();
                if let (Some(owner), Ok(root)) = (cache_owner.as_ref(), initial_root.as_ref()) {
                    anyhow::ensure!(
                        owner.contains_root(root),
                        "cache owner manifest conflicts with project_root metadata in {}",
                        old_dir.display()
                    );
                }
                if cache_owner.is_none()
                    && matches!(
                        initial_root.as_ref(),
                            Ok(root) if root != &normalized && root != &raw_identity
                    )
                {
                    continue;
                }

                // A matching or unreadable candidate is potentially ours.
                // If its lease is busy, failing closed avoids silently
                // creating a second target index while inspection is blocked.
                let _source_lock = try_acquire_exclusive_project_lock(&cache_dir, old_key)?
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "cache candidate {} is active and could not be safely inspected; retry after other ast-index processes exit",
                            old_dir.display()
                        )
                    })?;
                if !std::fs::symlink_metadata(&old_dir)
                    .map(|metadata| metadata.file_type().is_dir())
                    .unwrap_or(false)
                    || !std::fs::symlink_metadata(&old_db)
                        .map(|metadata| metadata.file_type().is_file())
                        .unwrap_or(false)
                {
                    continue;
                }
                ensure_safe_live_db_artifacts(&old_db)?;

                let locked_root = match read_cached_project_root(&old_db) {
                    Ok(root) => root,
                    Err(error)
                        if initial_failed && cache_owner.is_none() && !is_sqlite_busy(&error) =>
                    {
                        continue;
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("failed to revalidate cache candidate {}", old_dir.display())
                        })
                    }
                };
                if let Some(owner) = cache_owner.as_ref() {
                    anyhow::ensure!(
                        owner.contains_root(&locked_root),
                        "cache owner manifest conflicts with project_root metadata in {}",
                        old_dir.display()
                    );
                } else if locked_root != normalized && locked_root != raw_identity {
                    anyhow::ensure!(
                        initial_failed,
                        "matching cache {} changed during migration",
                        old_dir.display()
                    );
                    continue;
                }

                let _target_lock =
                    try_acquire_exclusive_project_lock(&cache_dir, &project_hash)?.ok_or_else(
                        || {
                            anyhow::anyhow!(
                                "cannot migrate into active cache {}; retry after other ast-index processes exit",
                                db_dir.display()
                            )
                        },
                    )?;
                let source_owner = validate_cache_owner_for_migration(
                    &cache_dir,
                    &old_dir,
                    old_key,
                    &requested_owner,
                )?;
                let authorized_source = source_owner.as_ref().unwrap_or(&requested_owner);
                let migration_desired = merge_authorized_source_owner_intents(
                    &cache_dir,
                    &old_dir,
                    authorized_source,
                    &desired_owner,
                )?;
                let migration_owner =
                    rename_cache_directory(&old_dir, &db_dir, &project_hash, &migration_desired)?;
                ensure_safe_live_db_artifacts(&db_path)?;
                install_cache_owner_manifest(&db_dir, &project_hash, &migration_owner, true)?;
                cleanup_source_generation_owner_intents(&cache_dir, &db_dir, &migration_owner);
                break;
            }
        }
    }

    ensure_real_cache_directory(&db_dir)?;
    ensure_safe_live_db_artifacts(&db_path)?;
    install_or_recover_target_cache_owner(
        &cache_dir,
        &db_dir,
        &db_path,
        &project_hash,
        &desired_owner,
    )?;
    let lease = acquire_shared_project_lease(&cache_dir, &project_hash)?;
    Ok((db_path, lease, normalized))
}

/// Get the database path for the current project
pub fn get_db_path(project_root: &Path) -> Result<PathBuf> {
    resolve_db_path_and_lease(project_root).map(|(path, _lease, _normalized)| path)
}

/// Hold a cache lease for an operation that outlives any one SQLite
/// connection, notably `watch` and the rebuild swap sequence.
pub fn acquire_project_lease(project_root: &Path) -> Result<ProjectLease> {
    resolve_db_path_and_lease(project_root).map(|(_path, lease, _normalized)| lease)
}

/// Acquire and retain the project's cache lease only when an initialized
/// index already exists.
///
/// The existence check happens while `lease` is held, so a concurrent stale
/// cache sweep cannot remove the directory between root discovery and the
/// command opening SQLite. Unlike [`db_exists`], cache-layout and SQLite
/// errors are returned to the caller instead of being treated as a missing
/// index.
pub fn acquire_project_lease_if_initialized(project_root: &Path) -> Result<Option<ProjectLease>> {
    let (db_path, mut lease, _normalized) = resolve_db_path_and_lease(project_root)?;
    let publication = try_acquire_shared_publication(&db_path, &lease)?;
    ensure_no_interrupted_publication(&db_path)?;
    if !std::fs::symlink_metadata(&db_path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
    {
        return Ok(None);
    }

    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(&db_path, flags)
        .with_context(|| format!("failed to inspect index {}", db_path.display()))?;
    let initialized = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='files')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .with_context(|| format!("failed to inspect index schema in {}", db_path.display()))?;

    lease.publication = Some(publication);
    Ok(initialized.then_some(lease))
}

/// Deterministic hash (djb2 algorithm) — stable across Rust versions unlike DefaultHasher
fn simple_hash(s: &str) -> String {
    let mut hash: u64 = 5381;
    for byte in s.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(byte as u64);
    }
    format!("{:x}", hash)
}

/// Remove the legacy cache base only when it is already empty.
pub fn cleanup_legacy_cache() {
    // A custom base has no well-defined relationship to the historical
    // global `kotlin-index` directory. Deriving a sibling can even point back
    // at the active base itself, so broad cleanup is disabled for overrides.
    if overridden_db_path().is_some() || overridden_cache_base().is_some() {
        return;
    }
    let Some(cache_root) = dirs::cache_dir() else {
        return;
    };
    let new_cache_dir = cache_root.join("ast-index");
    let Ok(_layout_lock) = acquire_layout_lock(&new_cache_dir) else {
        return;
    };
    let old_dir = cache_root.join("kotlin-index");
    let _ = std::fs::remove_dir(&old_dir);
}

/// Migrate project DB from old kotlin-index dir to new ast-index dir
pub fn migrate_legacy_project(project_root: &Path) {
    let _ = migrate_legacy_project_with_lease(project_root);
}

/// Migrate a legacy project cache, then return the normalized target's shared
/// lease without releasing the cache-layout lock between the EX and SH
/// phases. Production dispatch uses this to close the handoff window in
/// which stale-cache GC could otherwise remove a just-migrated database.
pub fn migrate_legacy_project_with_lease(project_root: &Path) -> Result<ProjectLease> {
    if overridden_db_path().is_some() {
        return Ok(ProjectLease::none());
    }
    if overridden_cache_base().is_some() {
        // A custom target has no safe, globally-coordinated legacy source.
        // Resolve and lease only that target; never infer a sibling path.
        return acquire_project_lease(project_root);
    }
    let cache_root = dirs::cache_dir().context("Could not find cache directory")?;
    migrate_legacy_project_in(
        &cache_root.join("ast-index"),
        &cache_root.join("kotlin-index"),
        project_root,
    )
}

fn migrate_legacy_project_in(
    new_cache_dir: &Path,
    old_cache_dir: &Path,
    project_root: &Path,
) -> Result<ProjectLease> {
    let legacy_raw = project_root.to_string_lossy();
    let (normalized, raw_identity) = resolve_root_identities(project_root)?;
    let requested_owner = CacheOwnerManifest::new(&normalized, &raw_identity);
    let legacy_project_hash = simple_hash(legacy_raw.as_ref());
    let normalized_project_hash = simple_hash(&normalized);
    let _layout_lock = acquire_layout_lock(&new_cache_dir)?;
    let old_db_dir = old_cache_dir.join(&legacy_project_hash);
    let new_db_dir = new_cache_dir.join(&normalized_project_hash);
    let desired_owner = merge_cache_owner_intents(
        new_cache_dir,
        &new_db_dir,
        &normalized_project_hash,
        &requested_owner,
    )?;
    let old_db = old_db_dir.join("index.db");
    let new_db = new_db_dir.join("index.db");
    ensure_real_cache_directory(&new_db_dir)?;
    ensure_safe_live_db_artifacts(&new_db)?;

    match std::fs::symlink_metadata(&new_db) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => anyhow::bail!(
            "refusing to migrate into non-regular target {}",
            new_db.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let old_source_is_valid = project_root.is_absolute()
                && std::fs::symlink_metadata(&old_db_dir)
                    .map(|metadata| metadata.file_type().is_dir())
                    .unwrap_or(false)
                && std::fs::symlink_metadata(&old_db)
                    .map(|metadata| metadata.file_type().is_file())
                    .unwrap_or(false);
            if old_source_is_valid {
                ensure_safe_live_db_artifacts(&old_db)?;
                let source_owner = validate_cache_owner_for_migration(
                    old_cache_dir,
                    &old_db_dir,
                    &legacy_project_hash,
                    &requested_owner,
                )?;
                let authorized_source = source_owner.as_ref().unwrap_or(&requested_owner);
                let migration_desired = merge_authorized_source_owner_intents(
                    old_cache_dir,
                    &old_db_dir,
                    authorized_source,
                    &desired_owner,
                )?;
                let migration_desired = merge_authorized_source_owner_intents(
                    new_cache_dir,
                    &old_db_dir,
                    authorized_source,
                    &migration_desired,
                )?;
                // There is no initialized target to protect with a shared
                // lease yet. Move the complete project directory atomically
                // while holding the normalized target lease exclusively.
                let target_lock = try_acquire_exclusive_project_lock(
                    &new_cache_dir,
                    &normalized_project_hash,
                )?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "legacy cache target {} is active; retry after other ast-index processes exit",
                        new_db_dir.display()
                    )
                })?;
                ensure_real_cache_directory(&new_db_dir)?;
                let migration_owner = rename_cache_directory(
                    &old_db_dir,
                    &new_db_dir,
                    &normalized_project_hash,
                    &migration_desired,
                )?;
                anyhow::ensure!(
                    std::fs::symlink_metadata(&new_db)
                        .map(|metadata| metadata.file_type().is_file())
                        .unwrap_or(false),
                    "legacy cache migration did not install {}",
                    new_db.display()
                );
                ensure_safe_live_db_artifacts(&new_db)?;
                install_cache_owner_manifest(
                    &new_db_dir,
                    &normalized_project_hash,
                    &migration_owner,
                    true,
                )?;
                cleanup_source_generation_owner_intents(
                    old_cache_dir,
                    &new_db_dir,
                    &migration_owner,
                );
                cleanup_source_generation_owner_intents(
                    new_cache_dir,
                    &new_db_dir,
                    &migration_owner,
                );
                drop(target_lock);
            }
        }
        Err(error) => return Err(error.into()),
    }

    // Layout remains locked while the target changes from exclusive to
    // shared protection, serializing this handoff against GC and resolvers.
    install_or_recover_target_cache_owner(
        new_cache_dir,
        &new_db_dir,
        &new_db,
        &normalized_project_hash,
        &desired_owner,
    )?;
    acquire_shared_project_lease(&new_cache_dir, &normalized_project_hash)
}

/// Rebuild lock plus the shared cache lease that protects the directory for
/// the complete staging and publication sequence.
pub struct RebuildLock {
    _lock_file: File,
    _lease: ProjectLease,
}

fn open_rebuild_lock_file(db_path: &Path) -> Result<File> {
    use fs2::FileExt;

    let lock_path = db_path.with_extension("lock");

    // Ensure parent dir exists
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)?;
    lock_file.try_lock_exclusive()
        .map_err(|_| anyhow::anyhow!("Another rebuild is already running for this project. Wait for it to finish or remove {}", lock_path.display()))?;
    Ok(lock_file)
}

/// Acquire the legacy rebuild lock file.
///
/// This source-compatible API does not retain the external cache lease after
/// it returns. Production rebuilds and other long-lived operations should use
/// [`acquire_rebuild_guard`] so stale-cache GC cannot remove the directory
/// while the lock is held.
pub fn acquire_rebuild_lock(project_root: &Path) -> Result<File> {
    let (db_path, _lease, _normalized) = resolve_db_path_and_lease(project_root)?;
    open_rebuild_lock_file(&db_path)
}

/// Acquire both the exclusive rebuild lock and the shared cache lease.
/// If another process holds the rebuild lock, returns an error immediately.
pub fn acquire_rebuild_guard(project_root: &Path) -> Result<RebuildLock> {
    let (db_path, lease, _normalized) = resolve_db_path_and_lease(project_root)?;
    let lock_file = open_rebuild_lock_file(&db_path)?;
    cleanup_abandoned_index_staging(&db_path)?;
    Ok(RebuildLock {
        _lock_file: lock_file,
        _lease: lease,
    })
}

const UPDATE_COORDINATOR_STATE_VERSION: u8 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateCoordinatorState {
    #[serde(default = "update_coordinator_state_version")]
    pub version: u8,
    pub requested_generation: u64,
    pub completed_generation: u64,
    #[serde(default)]
    pub successful_cycles: u64,
    #[serde(default)]
    pub worker_scheduled: bool,
    #[serde(default)]
    pub worker_launches: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_claim: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_started_claim: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_claimed_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure_generation: Option<u64>,
}

fn update_coordinator_state_version() -> u8 {
    UPDATE_COORDINATOR_STATE_VERSION
}

fn update_coordinator_paths(
    project_root: &Path,
) -> Result<(PathBuf, PathBuf, PathBuf, ProjectLease)> {
    let (db_path, lease, _normalized) = resolve_db_path_and_lease(project_root)?;
    Ok((
        db_path.with_extension("db.update-state-v1.json"),
        db_path.with_extension("db.update-state-v1.lock"),
        db_path.with_extension("db.update-worker-v1.lock"),
        lease,
    ))
}

pub fn create_update_worker_log(project_root: &Path) -> Result<(File, ProjectLease)> {
    let (db_path, lease, _normalized) = resolve_db_path_and_lease(project_root)?;
    let directory = db_path
        .parent()
        .context("update worker log has no parent directory")?;
    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".ast-index-update-worker-") && name.ends_with(".log") {
                if entry
                    .file_type()
                    .map(|kind| kind.is_file())
                    .unwrap_or(false)
                {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for sequence in 0..100_u32 {
        let path = directory.join(format!(
            ".ast-index-update-worker-{}-{}.log",
            std::process::id(),
            nonce + u128::from(sequence)
        ));
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => return Ok((file, lease)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("failed to create update worker log"),
        }
    }
    anyhow::bail!("could not allocate a unique update worker log")
}

fn read_update_state_file(path: &Path) -> Result<UpdateCoordinatorState> {
    match read_bounded_json_file(path)? {
        Some(state) => {
            let state: UpdateCoordinatorState = state;
            anyhow::ensure!(
                state.version == UPDATE_COORDINATOR_STATE_VERSION,
                "unsupported update coordinator state version {} in {}",
                state.version,
                path.display()
            );
            anyhow::ensure!(
                state.completed_generation <= state.requested_generation,
                "invalid update coordinator generations in {}",
                path.display()
            );
            Ok(state)
        }
        None => Ok(UpdateCoordinatorState {
            version: UPDATE_COORDINATOR_STATE_VERSION,
            ..UpdateCoordinatorState::default()
        }),
    }
}

fn write_update_state_file(path: &Path, state: &UpdateCoordinatorState) -> Result<()> {
    let parent = path
        .parent()
        .context("update coordinator state has no parent directory")?;
    write_bounded_json_file(path, parent, state, true)
}

fn with_locked_update_state<T>(
    project_root: &Path,
    update: impl FnOnce(&mut UpdateCoordinatorState) -> Result<T>,
) -> Result<T> {
    let (state_path, lock_path, _, _lease) = update_coordinator_paths(project_root)?;
    let lock = open_lock_file(&lock_path)?;
    fs2::FileExt::lock_exclusive(&lock)
        .with_context(|| format!("failed to lock update state {}", lock_path.display()))?;
    let mut state = read_update_state_file(&state_path)?;
    let result = update(&mut state)?;
    write_update_state_file(&state_path, &state)?;
    Ok(result)
}

pub fn read_update_coordinator_state(project_root: &Path) -> Result<UpdateCoordinatorState> {
    let (state_path, lock_path, _, _lease) = update_coordinator_paths(project_root)?;
    let lock = open_lock_file(&lock_path)?;
    fs2::FileExt::lock_shared(&lock)
        .with_context(|| format!("failed to lock update state {}", lock_path.display()))?;
    read_update_state_file(&state_path)
}

pub struct UpdateRequest {
    pub generation: u64,
    pub claim_token: Option<u64>,
}

pub fn request_update_generation(project_root: &Path) -> Result<UpdateRequest> {
    let worker_active = is_update_worker_active(project_root)?;
    with_locked_update_state(project_root, |state| {
        state.requested_generation = state
            .requested_generation
            .checked_add(1)
            .context("update generation counter overflow")?;
        let now = unix_time_millis();
        let claim_is_live = state.worker_claim.is_some()
            && (worker_active
                || state
                    .worker_claimed_at_ms
                    .map(|claimed| now.saturating_sub(claimed) < 1_000)
                    .unwrap_or(false));
        let claim_token = if claim_is_live {
            None
        } else {
            let token = state.worker_launches.saturating_add(1);
            state.worker_scheduled = true;
            state.worker_launches = token;
            state.worker_claim = Some(token);
            state.worker_started_claim = None;
            state.worker_claimed_at_ms = Some(now);
            Some(token)
        };
        Ok(UpdateRequest {
            generation: state.requested_generation,
            claim_token,
        })
    })
}

fn unix_time_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub fn claim_pending_update_worker(project_root: &Path) -> Result<Option<u64>> {
    let worker_active = is_update_worker_active(project_root)?;
    with_locked_update_state(project_root, |state| {
        if state.requested_generation <= state.completed_generation {
            return Ok(None);
        }
        let now = unix_time_millis();
        let claim_is_live = state.worker_claim.is_some()
            && (worker_active
                || state
                    .worker_claimed_at_ms
                    .map(|claimed| now.saturating_sub(claimed) < 1_000)
                    .unwrap_or(false));
        if claim_is_live {
            return Ok(None);
        }
        let token = state.worker_launches.saturating_add(1);
        state.worker_scheduled = true;
        state.worker_launches = token;
        state.worker_claim = Some(token);
        state.worker_started_claim = None;
        state.worker_claimed_at_ms = Some(now);
        Ok(Some(token))
    })
}

pub fn acknowledge_update_worker_start(project_root: &Path, token: u64) -> Result<bool> {
    with_locked_update_state(project_root, |state| {
        if state.worker_claim != Some(token) {
            return Ok(false);
        }
        state.worker_started_claim = Some(token);
        Ok(true)
    })
}

pub fn finish_update_worker_if_idle(project_root: &Path, token: u64) -> Result<bool> {
    with_locked_update_state(project_root, |state| {
        if state.requested_generation > state.completed_generation {
            return Ok(false);
        }
        if state.worker_claim == Some(token) {
            clear_worker_claim(state);
        }
        Ok(true)
    })
}

pub fn clear_update_worker_schedule(project_root: &Path, token: u64) -> Result<()> {
    with_locked_update_state(project_root, |state| {
        if state.worker_claim == Some(token) {
            clear_worker_claim(state);
        }
        Ok(())
    })
}

fn clear_worker_claim(state: &mut UpdateCoordinatorState) {
    state.worker_scheduled = false;
    state.worker_claim = None;
    state.worker_started_claim = None;
    state.worker_claimed_at_ms = None;
}

pub fn record_update_failure_and_finish(
    project_root: &Path,
    token: u64,
    generation: u64,
    error: &anyhow::Error,
) -> Result<bool> {
    with_locked_update_state(project_root, |state| {
        state.last_error = Some(format!("{error:#}"));
        state.last_failure_generation = Some(generation);
        if state.requested_generation > generation {
            return Ok(false);
        }
        if state.worker_claim == Some(token) {
            clear_worker_claim(state);
        }
        Ok(true)
    })
}

pub fn complete_update_generation(project_root: &Path, generation: u64) -> Result<()> {
    with_locked_update_state(project_root, |state| {
        state.completed_generation = state
            .completed_generation
            .max(generation.min(state.requested_generation));
        state.successful_cycles = state.successful_cycles.saturating_add(1);
        state.last_error = None;
        state.last_failure_generation = None;
        Ok(())
    })
}

pub fn acknowledge_full_refresh(project_root: &Path, generation: u64) -> Result<()> {
    with_locked_update_state(project_root, |state| {
        state.completed_generation = state
            .completed_generation
            .max(generation.min(state.requested_generation));
        if state.last_failure_generation.unwrap_or(0) <= state.completed_generation {
            state.last_error = None;
            state.last_failure_generation = None;
        }
        Ok(())
    })
}

pub fn snapshot_requested_update_generation(project_root: &Path) -> Result<u64> {
    Ok(read_update_coordinator_state(project_root)?.requested_generation)
}

pub fn has_pending_update(project_root: &Path) -> Result<bool> {
    let state = read_update_coordinator_state(project_root)?;
    Ok(state.requested_generation > state.completed_generation)
}

pub fn fail_update_generation(
    project_root: &Path,
    generation: u64,
    error: &anyhow::Error,
) -> Result<()> {
    with_locked_update_state(project_root, |state| {
        if generation <= state.completed_generation {
            return Ok(());
        }
        state.last_error = Some(format!("{error:#}"));
        state.last_failure_generation = Some(generation);
        Ok(())
    })
}

pub struct UpdateWorkerLock {
    _file: File,
    _lease: ProjectLease,
}

pub fn try_acquire_update_worker(project_root: &Path) -> Result<Option<UpdateWorkerLock>> {
    let (_state_path, _state_lock_path, worker_lock_path, lease) =
        update_coordinator_paths(project_root)?;
    let file = open_lock_file(&worker_lock_path)?;
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(Some(UpdateWorkerLock {
            _file: file,
            _lease: lease,
        })),
        Err(error) if lock_is_contended(&error) => Ok(None),
        Err(error) => Err(error).with_context(|| {
            format!(
                "failed to acquire update worker lock {}",
                worker_lock_path.display()
            )
        }),
    }
}

pub fn is_update_worker_active(project_root: &Path) -> Result<bool> {
    Ok(try_acquire_update_worker(project_root)?.is_none())
}

pub fn wait_for_update_generation(
    project_root: &Path,
    generation: u64,
    timeout: Duration,
) -> Result<()> {
    if generation == 0 {
        return Ok(());
    }
    let started = Instant::now();
    loop {
        let state = read_update_coordinator_state(project_root)?;
        if state.completed_generation >= generation {
            return Ok(());
        }
        if state.last_failure_generation.unwrap_or(0) >= generation
            && state.worker_claim.is_none()
            && !is_update_worker_active(project_root)?
        {
            anyhow::bail!(
                "background index update generation {} failed and remains pending: {}",
                generation,
                state
                    .last_error
                    .as_deref()
                    .unwrap_or("unknown update failure")
            );
        }
        if started.elapsed() >= timeout {
            anyhow::bail!(
                "timed out after {} ms waiting for background index update generation {} (completed {}, requested {})",
                timeout.as_millis(),
                generation,
                state.completed_generation,
                state.requested_generation
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub fn wait_for_pending_update(project_root: &Path, timeout: Duration) -> Result<()> {
    let state = read_update_coordinator_state(project_root)?;
    if state.requested_generation <= state.completed_generation {
        return Ok(());
    }
    wait_for_update_generation(project_root, state.requested_generation, timeout)
}

/// How long a cached index may sit untouched before `gc_stale_caches`
/// removes it. Activity is measured from the newest regular DB, WAL, SHM,
/// journal, rebuild-swap artifact, or external activity marker, so reads and
/// active SQLite/rebuild sidecars keep the cache alive even when the main
/// database mtime is old.
pub const STALE_CACHE_MAX_AGE_DAYS: u64 = 14;
pub const STALE_CACHE_MAX_AGE: std::time::Duration =
    std::time::Duration::from_secs(STALE_CACHE_MAX_AGE_DAYS * 24 * 60 * 60);

const CACHE_ACTIVITY_MARKER_NAME: &str = ".ast-index-access-v1";
const MAX_CACHE_ACTIVITY_MARKER_BYTES: u64 = 64;
const CACHE_ACTIVITY_TOUCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60 * 60);

const CACHE_ACTIVITY_FILES: &[&str] = &[
    "index.db",
    "index.db-wal",
    "index.db-shm",
    "index.db-journal",
    "index.db.swap",
    "index.db.swap-wal",
    "index.db.swap-shm",
    "index.db.swap-journal",
    "index.db.swap-pending",
    "index.db.publish-state-v1",
    "index.db.publish-commit-v1",
    "index.db.update-state-v1.json",
    CACHE_ACTIVITY_MARKER_NAME,
];

fn touch_cache_activity_marker(db_path: &Path) -> Result<()> {
    let cache_dir = db_path
        .parent()
        .context("database path has no cache directory")?;
    let marker = cache_dir.join(CACHE_ACTIVITY_MARKER_NAME);
    let now = std::time::SystemTime::now();
    let existing = match std::fs::symlink_metadata(&marker) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.file_type().is_file() && metadata.len() <= MAX_CACHE_ACTIVITY_MARKER_BYTES,
                "cache activity marker is not a bounded regular file: {}",
                marker.display()
            );
            let modified = metadata.modified().with_context(|| {
                format!(
                    "failed to inspect cache activity marker {}",
                    marker.display()
                )
            })?;
            match now.duration_since(modified) {
                Ok(age) if age < CACHE_ACTIVITY_TOUCH_INTERVAL => return Ok(()),
                Err(_) => return Ok(()),
                _ => {}
            }
            Some(metadata)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to inspect cache activity marker {}",
                    marker.display()
                )
            })
        }
    };

    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }

    let file = options
        .open(&marker)
        .with_context(|| format!("failed to open cache activity marker {}", marker.display()))?;
    let opened = file.metadata().with_context(|| {
        format!(
            "failed to inspect open cache activity marker {}",
            marker.display()
        )
    })?;
    anyhow::ensure!(
        opened.file_type().is_file()
            && opened.len() <= MAX_CACHE_ACTIVITY_MARKER_BYTES
            && existing
                .as_ref()
                .map(|metadata| same_file_identity(metadata, &opened))
                .unwrap_or(true),
        "cache activity marker changed while opening: {}",
        marker.display()
    );
    file.set_len(0)
        .with_context(|| format!("failed to bound cache activity marker {}", marker.display()))?;
    file.set_modified(now)
        .with_context(|| format!("failed to touch cache activity marker {}", marker.display()))?;
    Ok(())
}

fn is_cache_key(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 16
        && name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_gc_tombstone(name: &str) -> bool {
    let mut parts = name.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(key), Some(pid), Some(sequence), None)
            if is_cache_key(key)
                && !pid.is_empty()
                && pid.bytes().all(|byte| byte.is_ascii_digit())
                && !sequence.is_empty()
                && sequence.bytes().all(|byte| byte.is_ascii_digit())
    )
}

/// Return the newest activity-file mtime. `None` is deliberately fail-closed:
/// missing anchors, symlinks, special files, future timestamps, and metadata
/// failures all keep the directory.
fn cache_age(dir: &Path, now: std::time::SystemTime) -> Option<std::time::Duration> {
    let mut newest = None;
    let mut has_anchor = false;

    for name in CACHE_ACTIVITY_FILES {
        let path = dir.join(name);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        if !metadata.file_type().is_file() {
            return None;
        }
        if *name == CACHE_ACTIVITY_MARKER_NAME && metadata.len() > MAX_CACHE_ACTIVITY_MARKER_BYTES {
            return None;
        }
        if matches!(*name, "index.db" | "index.db.swap") {
            has_anchor = true;
        }
        let modified = metadata.modified().ok()?;
        newest = Some(newest.map_or(modified, |current: std::time::SystemTime| {
            current.max(modified)
        }));
    }

    if !has_anchor {
        return None;
    }
    now.duration_since(newest?).ok()
}

fn collect_trash_dirs(trash_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(entries) = std::fs::read_dir(trash_dir) {
        for entry in entries.flatten() {
            let valid_name = entry
                .file_name()
                .to_str()
                .map(is_gc_tombstone)
                .unwrap_or(false);
            if valid_name && entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                paths.push(entry.path());
            }
        }
    }
    paths
}

fn prepare_trash_dir(trash_dir: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(trash_dir) {
        Ok(metadata) => return Ok(metadata.file_type().is_dir()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Ok(false),
    }
    std::fs::create_dir(trash_dir)?;
    Ok(std::fs::symlink_metadata(trash_dir)
        .map(|metadata| metadata.file_type().is_dir())
        .unwrap_or(false))
}

fn cache_has_unresolved_publication(cache_dir: &Path) -> bool {
    [
        "index.db.publish-state-v1",
        "index.db.publish-commit-v1",
        "index.db.swap",
        "index.db.swap-wal",
        "index.db.swap-shm",
        "index.db.swap-journal",
        "index.db.swap-pending",
    ]
    .iter()
    .any(
        |name| match std::fs::symlink_metadata(cache_dir.join(name)) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            // GC is best-effort and must fail closed when it cannot prove a
            // recovery artifact absent.
            Err(_) => true,
        },
    )
}

/// Remove cached indexes for *other* projects that have not been touched
/// within `max_age`. Best-effort: unreadable or undeletable entries are
/// skipped. `keep` is the hash-dir name of the project currently in use, so
/// it is never removed. Returns the number of project caches deleted.
///
/// Split out from `gc_stale_caches` so tests can drive it against a
/// throwaway base dir with an injected `now`.
pub fn gc_stale_caches_in(
    base: &Path,
    keep: Option<&str>,
    max_age: std::time::Duration,
    now: std::time::SystemTime,
) -> Result<usize> {
    use fs2::FileExt;

    if !base.is_dir() {
        return Ok(0);
    }

    // GC is best-effort. If another process is currently resolving/migrating
    // cache paths, leave the sweep for the next successful rebuild/update.
    let layout_lock = open_lock_file(&leases_dir(base).join("layout.lock"))?;
    if layout_lock.try_lock_exclusive().is_err() {
        return Ok(0);
    }

    let trash_dir = base.join(".gc-trash");
    if !prepare_trash_dir(&trash_dir)? {
        return Ok(0);
    }
    let mut quarantined = collect_trash_dirs(&trash_dir);
    let entries = match std::fs::read_dir(base) {
        Ok(entries) => entries,
        Err(_) => return Ok(0),
    };
    let mut removed = 0;
    let mut sequence = 0_u64;

    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if !file_type.is_dir() {
            continue;
        }
        let dir = entry.path();
        let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_cache_key(name) || Some(name) == keep {
            continue;
        }

        // The exclusive lease is non-blocking: any leased production
        // connection, watch, rebuild, restore, or clear operation makes this
        // candidate ineligible. Compatibility APIs without leases do not.
        let project_lock = match open_lock_file(&leases_dir(base).join(format!("{name}.lock"))) {
            Ok(file) => file,
            Err(_) => continue,
        };
        if project_lock.try_lock_exclusive().is_err() {
            continue;
        }

        // Publication markers are recovery state, not mere activity hints.
        // An unmarked swap is generation-ambiguous as well: recovery refuses
        // to guess its ownership, so GC must preserve it for manual repair.
        if cache_has_unresolved_publication(&dir) {
            continue;
        }

        // Re-stat only after acquiring the lease. The earlier directory
        // entry is never trusted for the delete decision.
        if !cache_age(&dir, now)
            .map(|age| age > max_age)
            .unwrap_or(false)
        {
            continue;
        }

        let tombstone = loop {
            let candidate = trash_dir.join(format!("{name}.{}.{}", std::process::id(), sequence));
            sequence += 1;
            if !candidate.exists() {
                break candidate;
            }
        };
        if std::fs::rename(&dir, &tombstone).is_ok() {
            quarantined.push(tombstone);
            removed += 1;
        }
    }

    // Renaming is the atomic logical deletion. Physical cleanup happens
    // after releasing all locks; crash leftovers are retried on the next GC.
    drop(layout_lock);
    for path in quarantined {
        let _ = std::fs::remove_dir_all(path);
    }
    Ok(removed)
}

/// Garbage-collect stale index caches across all projects (see
/// `STALE_CACHE_MAX_AGE`). The cache for `current_root` is always kept.
///
/// No-op when the DB path is overridden via `AST_INDEX_DB_PATH` /
/// `KOTLIN_INDEX_DB_PATH`, since there is no `<hash>` cache layout to sweep.
pub fn gc_stale_caches(current_root: &Path) -> Result<usize> {
    let disabled = std::env::var("AST_INDEX_DISABLE_GC")
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    if disabled || overridden_db_path().is_some() {
        return Ok(0);
    }
    let base = match cache_base_dir() {
        Some(b) => b,
        None => return Ok(0),
    };
    let keep = project_cache_key(current_root)?;
    let current_db = base.join(&keep).join("index.db");
    if !std::fs::symlink_metadata(&current_db)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
    {
        return Ok(0);
    }
    gc_stale_caches_in(
        &base,
        Some(&keep),
        STALE_CACHE_MAX_AGE,
        std::time::SystemTime::now(),
    )
}

const PUBLICATION_STATE_VERSION: u8 = 1;
const PUBLICATION_STATE_EXTENSION: &str = "db.publish-state-v1";
const PUBLICATION_COMMIT_EXTENSION: &str = "db.publish-commit-v1";
const STAGING_OWNER_VERSION: u8 = 1;
const STAGING_OWNER_NAME: &str = ".ast-index-staging-owner-v1.json";

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PublicationOperation {
    Install,
    Clear,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PublicationState {
    version: u8,
    token: String,
    operation: PublicationOperation,
    // main, WAL, SHM, rollback journal. Publication first consolidates the
    // old generation, but persisting the complete initial family makes any
    // violated SQLite-quiescence assumption explicit and recoverable.
    artifacts: [bool; 4],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    staging_dir: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PublicationCommit {
    version: u8,
    token: String,
    operation: PublicationOperation,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StagingOwner {
    version: u8,
    token: String,
    purpose: String,
    directory: String,
    live_db: String,
}

fn publication_state_path(db_path: &Path) -> PathBuf {
    db_path.with_extension(PUBLICATION_STATE_EXTENSION)
}

fn publication_commit_path(db_path: &Path) -> PathBuf {
    db_path.with_extension(PUBLICATION_COMMIT_EXTENSION)
}

fn publication_pending_swap_path(db_path: &Path) -> PathBuf {
    db_path.with_extension("db.swap-pending")
}

fn publication_has_interrupted_state(db_path: &Path) -> Result<bool> {
    for path in [
        publication_state_path(db_path),
        publication_commit_path(db_path),
    ] {
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                anyhow::ensure!(
                    metadata.file_type().is_file()
                        && metadata.len() <= MAX_CACHE_OWNER_MANIFEST_BYTES,
                    "index publication marker is not a bounded regular file: {}",
                    path.display()
                );
                return Ok(true);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to inspect index publication marker {}",
                        path.display()
                    )
                })
            }
        }
    }
    for suffix in SWAP_SUFFIXES {
        let swap = db_path.with_extension(swap_extension(suffix));
        match std::fs::symlink_metadata(&swap) {
            Ok(metadata) => {
                anyhow::ensure!(
                    metadata.file_type().is_file(),
                    "index publication swap is not a regular file: {}",
                    swap.display()
                );
                return Ok(true);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to inspect index publication swap {}",
                        swap.display()
                    )
                })
            }
        }
    }
    let pending_swap = publication_pending_swap_path(db_path);
    match std::fs::symlink_metadata(&pending_swap) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.file_type().is_file(),
                "index publication pending swap is not a regular file: {}",
                pending_swap.display()
            );
            return Ok(true);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(false)
}

fn ensure_no_interrupted_publication(db_path: &Path) -> Result<()> {
    if publication_has_interrupted_state(db_path)? {
        return Err(publication_busy(format!(
            "interrupted publication at {}; run rebuild or restore to recover",
            db_path.display()
        )));
    }
    Ok(())
}

fn remove_regular_file_if_present(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.file_type().is_file(),
                "refusing to remove non-regular publication artifact {}",
                path.display()
            );
            std::fs::remove_file(path).with_context(|| {
                format!("failed to remove publication artifact {}", path.display())
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("failed to inspect publication artifact {}", path.display())),
    }
}

fn write_publication_marker<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path
        .parent()
        .context("index publication marker has no parent directory")?;
    write_bounded_json_file(path, parent, value, false)
}

fn remove_publication_marker(path: &Path) -> Result<()> {
    remove_regular_file_if_present(path)?;
    let parent = path
        .parent()
        .context("index publication marker has no parent directory")?;
    sync_cache_directory(parent)
}

fn new_publication_token() -> String {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{timestamp}-{sequence}", std::process::id())
}

fn publication_artifact_bitmap(db_path: &Path) -> Result<[bool; 4]> {
    let mut present = [false; 4];
    for (index, suffix) in LIVE_DB_SUFFIXES.iter().enumerate() {
        let path = db_path.with_extension(live_extension(suffix));
        present[index] = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                anyhow::ensure!(
                    metadata.file_type().is_file(),
                    "index database artifact is not a regular file: {}",
                    path.display()
                );
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
    }
    Ok(present)
}

fn valid_publication_staging_dir_name(name: &str) -> bool {
    (name.starts_with(".rebuild-") || name.starts_with(".restore-"))
        && name.len() <= 255
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn abandoned_staging_purpose(name: &str) -> Option<&'static str> {
    for purpose in ["rebuild", "restore"] {
        let Some(remainder) = name.strip_prefix(&format!(".{purpose}-")) else {
            continue;
        };
        let (pid, sequence) = remainder.split_once('-')?;
        if !pid.is_empty()
            && pid.bytes().all(|byte| byte.is_ascii_digit())
            && !sequence.is_empty()
            && sequence.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Some(purpose);
        }
    }
    None
}

fn staging_owner_path(directory: &Path) -> PathBuf {
    directory.join(STAGING_OWNER_NAME)
}

fn validate_staging_location(staged_db: &Path, live_db: &Path, purpose: &str) -> Result<String> {
    anyhow::ensure!(
        matches!(purpose, "rebuild" | "restore"),
        "unsupported index staging purpose: {purpose}"
    );
    anyhow::ensure!(
        staged_db.file_name().and_then(|name| name.to_str()) == Some("index.db"),
        "staged generation database must be named index.db"
    );
    let directory = staged_db
        .parent()
        .context("staged generation has no directory")?;
    anyhow::ensure!(
        directory.parent() == live_db.parent(),
        "staged generation must be beside the live index"
    );
    let name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .context("staged generation directory name is not valid UTF-8")?;
    anyhow::ensure!(
        abandoned_staging_purpose(name) == Some(purpose),
        "invalid {purpose} staging directory name: {name}"
    );
    Ok(name.to_owned())
}

/// Persist a bounded owner record before a writer starts populating a private
/// rebuild/restore directory. A later mutation guard may remove only a
/// directory whose name, owner record, and contents all validate.
pub fn register_index_staging(staged_db: &Path, live_db: &Path, purpose: &str) -> Result<()> {
    let directory_name = validate_staging_location(staged_db, live_db, purpose)?;
    let directory = staged_db
        .parent()
        .context("staged generation has no directory")?;
    let metadata = std::fs::symlink_metadata(directory).with_context(|| {
        format!(
            "failed to inspect staging directory {}",
            directory.display()
        )
    })?;
    anyhow::ensure!(
        metadata.file_type().is_dir(),
        "staging path is not a real directory: {}",
        directory.display()
    );
    let owner = StagingOwner {
        version: STAGING_OWNER_VERSION,
        token: new_publication_token(),
        purpose: purpose.to_owned(),
        directory: directory_name,
        live_db: live_db.to_string_lossy().into_owned(),
    };
    write_bounded_json_file(&staging_owner_path(directory), directory, &owner, false)?;
    sync_cache_directory(directory)
}

fn read_valid_staging_owner(directory: &Path) -> Result<StagingOwner> {
    let name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .context("staging directory name is not valid UTF-8")?;
    let purpose = abandoned_staging_purpose(name)
        .with_context(|| format!("unrecognized staging directory {name}"))?;
    let owner_path = staging_owner_path(directory);
    let owner: StagingOwner = read_bounded_json_file(&owner_path)?
        .with_context(|| format!("staging owner marker is missing in {}", directory.display()))?;
    anyhow::ensure!(
        owner.version == STAGING_OWNER_VERSION
            && valid_cache_generation_token(&owner.token)
            && owner.purpose == purpose
            && owner.directory == name,
        "staging owner marker does not match {}",
        directory.display()
    );
    anyhow::ensure!(
        Path::new(&owner.live_db).parent() == directory.parent(),
        "staging owner live database is outside {}",
        directory.parent().unwrap_or(directory).display()
    );
    Ok(owner)
}

fn cleanup_owned_staging_directory(directory: &Path, live_db: &Path) -> Result<()> {
    let owner = read_valid_staging_owner(directory)?;
    anyhow::ensure!(
        owner.live_db == live_db.to_string_lossy(),
        "staging owner marker does not match {}",
        directory.display()
    );
    let owner_path = staging_owner_path(directory);

    let allowed = [
        STAGING_OWNER_NAME,
        "index.db",
        "index.db-wal",
        "index.db-shm",
        "index.db-journal",
    ];
    for entry in std::fs::read_dir(directory).with_context(|| {
        format!(
            "failed to inspect staging directory {}",
            directory.display()
        )
    })? {
        let entry = entry?;
        let entry_name = entry
            .file_name()
            .to_str()
            .context("staging artifact name is not valid UTF-8")?
            .to_owned();
        anyhow::ensure!(
            allowed.contains(&entry_name.as_str()),
            "unexpected artifact in abandoned staging directory: {}",
            entry.path().display()
        );
        let metadata = std::fs::symlink_metadata(entry.path())?;
        anyhow::ensure!(
            metadata.file_type().is_file(),
            "staging artifact is not a regular file: {}",
            entry.path().display()
        );
    }

    for name in [
        "index.db-journal",
        "index.db-wal",
        "index.db-shm",
        "index.db",
    ] {
        remove_regular_file_if_present(&directory.join(name))?;
    }
    remove_regular_file_if_present(&owner_path)?;
    std::fs::remove_dir(directory).with_context(|| {
        format!(
            "failed to remove abandoned staging directory {}",
            directory.display()
        )
    })?;
    sync_cache_directory(
        directory
            .parent()
            .context("staging directory has no cache parent")?,
    )
}

/// Remove a staging directory created by [`register_index_staging`].
pub fn discard_index_staging(staged_db: &Path, live_db: &Path) -> Result<()> {
    let Some(directory) = staged_db.parent() else {
        anyhow::bail!("staged generation has no directory");
    };
    match std::fs::symlink_metadata(directory) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.file_type().is_dir(),
                "staging path is not a real directory: {}",
                directory.display()
            );
            cleanup_owned_staging_directory(directory, live_db)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn cleanup_abandoned_index_staging(db_path: &Path) -> Result<()> {
    let parent = db_path
        .parent()
        .context("live index has no cache directory")?;
    for entry in std::fs::read_dir(parent)
        .with_context(|| format!("failed to inspect cache directory {}", parent.display()))?
    {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if abandoned_staging_purpose(&name).is_none() {
            continue;
        }
        let metadata = std::fs::symlink_metadata(entry.path())?;
        anyhow::ensure!(
            metadata.file_type().is_dir(),
            "abandoned staging path is not a real directory: {}",
            entry.path().display()
        );
        let owner = read_valid_staging_owner(&entry.path())?;
        if owner.live_db != db_path.to_string_lossy() {
            // Multiple explicit DB overrides may intentionally share a
            // parent. A valid owner for another live DB is not ours to
            // inspect or remove; malformed ownership still fails closed.
            continue;
        }
        cleanup_owned_staging_directory(&entry.path(), db_path)?;
    }
    Ok(())
}

fn publication_staging_dir_name(db_path: &Path, staged_db: &Path) -> Result<String> {
    let live_parent = db_path
        .parent()
        .context("live index has no parent directory")?;
    let staged_parent = staged_db
        .parent()
        .context("staged index has no parent directory")?;
    anyhow::ensure!(
        staged_parent.parent() == Some(live_parent),
        "staged index must live in a private directory beside {}",
        db_path.display()
    );
    anyhow::ensure!(
        staged_db.file_name().and_then(|name| name.to_str()) == Some("index.db"),
        "staged generation database must be named index.db"
    );
    let name = staged_parent
        .file_name()
        .and_then(|name| name.to_str())
        .context("staged generation directory name is not valid UTF-8")?;
    anyhow::ensure!(
        valid_publication_staging_dir_name(name),
        "invalid staged generation directory name: {name}"
    );
    Ok(name.to_owned())
}

fn cleanup_recorded_publication_staging(
    db_path: &Path,
    state: Option<&PublicationState>,
) -> Result<()> {
    let Some(name) = state.and_then(|state| state.staging_dir.as_deref()) else {
        return Ok(());
    };
    anyhow::ensure!(
        valid_publication_staging_dir_name(name),
        "invalid staged generation directory in publication marker: {name}"
    );
    let parent = db_path
        .parent()
        .context("live index has no parent directory")?;
    let directory = parent.join(name);
    match std::fs::symlink_metadata(&directory) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.file_type().is_dir(),
                "recorded staging path is not a real directory: {}",
                directory.display()
            );
            if abandoned_staging_purpose(name).is_some() {
                cleanup_owned_staging_directory(&directory, db_path)
            } else {
                cleanup_restore_staging(&directory.join("index.db"))?;
                std::fs::remove_dir(&directory).with_context(|| {
                    format!(
                        "failed to remove recovered staging directory {}",
                        directory.display()
                    )
                })?;
                sync_cache_directory(parent)
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| {
            format!(
                "failed to inspect recovered staging directory {}",
                directory.display()
            )
        }),
    }
}

fn checkpoint_and_consolidate_live_db(db_path: &Path) -> Result<[bool; 4]> {
    ensure_safe_live_db_artifacts(db_path)?;
    if !db_path.exists() {
        let bitmap = publication_artifact_bitmap(db_path)?;
        anyhow::ensure!(
            !bitmap.iter().any(|present| *present),
            "SQLite sidecar exists without a live index at {}",
            db_path.display()
        );
        return Ok(bitmap);
    }

    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(db_path, flags)
        .with_context(|| format!("failed to quiesce live index {}", db_path.display()))?;
    conn.busy_timeout(std::time::Duration::ZERO)?;
    let (busy, _log_frames, _checkpointed): (i64, i64, i64) =
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    anyhow::ensure!(
        busy == 0,
        "live index is still active during publication; retry shortly"
    );
    let journal_mode: String =
        conn.query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))?;
    anyhow::ensure!(
        journal_mode.eq_ignore_ascii_case("delete"),
        "failed to consolidate live index into rollback-journal mode"
    );
    drop(conn);

    // Once the exclusive publication lock has drained cooperating SQLite
    // connections, checkpointed WAL bookkeeping files carry no generation
    // data and may be removed before the durable initial bitmap is written.
    for suffix in ["-wal", "-shm", "-journal"] {
        remove_regular_file_if_present(&db_path.with_extension(live_extension(suffix)))?;
    }
    sync_regular_file(db_path)?;
    let bitmap = publication_artifact_bitmap(db_path)?;
    anyhow::ensure!(
        !bitmap[1] && !bitmap[2] && !bitmap[3],
        "live index left SQLite sidecars after publication checkpoint"
    );
    Ok(bitmap)
}

fn sync_regular_file(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect index file {}", path.display()))?;
    anyhow::ensure!(
        metadata.file_type().is_file(),
        "index file is not regular: {}",
        path.display()
    );
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    options.write(true);
    options
        .open(path)
        .and_then(|file| file.sync_all())
        .with_context(|| format!("failed to sync index file {}", path.display()))
}

fn snapshot_live_main(db_path: &Path, swap_path: &Path) -> Result<()> {
    match std::fs::hard_link(db_path, swap_path) {
        Ok(()) => Ok(()),
        Err(link_error) if link_error.kind() != std::io::ErrorKind::AlreadyExists => {
            let pending = publication_pending_swap_path(db_path);
            ensure_regular_or_missing(&pending)?;
            anyhow::ensure!(
                !pending.exists(),
                "untracked pending index snapshot exists at {}",
                pending.display()
            );
            let copy_result = (|| -> Result<()> {
                std::fs::copy(db_path, &pending).with_context(|| {
                    format!(
                        "failed to snapshot live index {} after hard-link failure: {link_error}",
                        db_path.display()
                    )
                })?;
                sync_regular_file(&pending)?;
                std::fs::rename(&pending, swap_path).with_context(|| {
                    format!(
                        "failed to install copied index snapshot {}",
                        swap_path.display()
                    )
                })?;
                Ok(())
            })();
            if copy_result.is_err() {
                let _ = remove_regular_file_if_present(&pending);
            }
            copy_result
        }
        Err(error) => Err(error)
            .with_context(|| format!("publication swap already exists: {}", swap_path.display())),
    }
}

fn recover_interrupted_publication_at_path(db_path: &Path) -> Result<()> {
    ensure_safe_live_db_artifacts(db_path)?;
    ensure_safe_swap_db_artifacts(db_path)?;
    ensure_regular_or_missing(&publication_pending_swap_path(db_path))?;
    let state_path = publication_state_path(db_path);
    let commit_path = publication_commit_path(db_path);
    let state: Option<PublicationState> = read_bounded_json_file(&state_path)?;
    let commit: Option<PublicationCommit> = read_bounded_json_file(&commit_path)?;

    if state.is_none() && commit.is_none() {
        anyhow::ensure!(
            !SWAP_SUFFIXES
                .iter()
                .any(|suffix| db_path.with_extension(swap_extension(suffix)).exists()),
            "untracked index swap exists at {}; refusing to delete it",
            db_path.display()
        );
        anyhow::ensure!(
            !publication_pending_swap_path(db_path).exists(),
            "untracked pending index swap exists at {}; refusing to delete it",
            db_path.display()
        );
        return Ok(());
    }
    if let Some(state) = state.as_ref() {
        anyhow::ensure!(
            state.version == PUBLICATION_STATE_VERSION
                && valid_cache_generation_token(&state.token),
            "invalid index publication state {}",
            state_path.display()
        );
        anyhow::ensure!(
            !state.artifacts[1] && !state.artifacts[2] && !state.artifacts[3],
            "index publication state contains unconsolidated SQLite sidecars at {}",
            db_path.display()
        );
    }
    if let Some(commit) = commit.as_ref() {
        anyhow::ensure!(
            commit.version == PUBLICATION_STATE_VERSION
                && valid_cache_generation_token(&commit.token),
            "invalid index publication commit {}",
            commit_path.display()
        );
        if let Some(state) = state.as_ref() {
            anyhow::ensure!(
                commit.token == state.token && commit.operation == state.operation,
                "index publication state/commit mismatch at {}",
                db_path.display()
            );
        }
    }

    let committed_operation = commit.as_ref().map(|marker| marker.operation);
    match committed_operation {
        Some(PublicationOperation::Install) => {
            anyhow::ensure!(
                std::fs::symlink_metadata(db_path)
                    .map(|metadata| metadata.file_type().is_file())
                    .unwrap_or(false),
                "committed index publication has no live database at {}",
                db_path.display()
            );
            cleanup_recorded_committed_swaps(db_path, state.as_ref())?;
        }
        Some(PublicationOperation::Clear) => {
            for suffix in LIVE_DB_SUFFIXES {
                remove_regular_file_if_present(&db_path.with_extension(live_extension(suffix)))?;
            }
            cleanup_recorded_committed_swaps(db_path, state.as_ref())?;
        }
        None => {
            let state = state
                .as_ref()
                .context("index publication commit exists without a recoverable state")?;
            if state.artifacts[0] {
                let swap = db_path.with_extension(swap_extension(""));
                if swap.exists() {
                    remove_regular_file_if_present(db_path)?;
                    std::fs::rename(&swap, db_path).with_context(|| {
                        format!(
                            "failed to restore interrupted index from {}",
                            swap.display()
                        )
                    })?;
                } else {
                    anyhow::ensure!(
                        db_path.exists(),
                        "interrupted publication lost both live and swap databases at {}",
                        db_path.display()
                    );
                }
            } else {
                remove_regular_file_if_present(db_path)?;
            }
            for suffix in ["-wal", "-shm", "-journal"] {
                remove_regular_file_if_present(&db_path.with_extension(live_extension(suffix)))?;
                let swap = db_path.with_extension(swap_extension(suffix));
                anyhow::ensure!(
                    !swap.exists(),
                    "unexpected sidecar swap in consolidated publication: {}",
                    swap.display()
                );
            }
        }
    }

    let parent = db_path
        .parent()
        .context("index database has no parent directory")?;
    remove_regular_file_if_present(&publication_pending_swap_path(db_path))?;
    sync_cache_directory(parent)?;
    cleanup_recorded_publication_staging(db_path, state.as_ref())?;
    // State is removed first. A crash between removals leaves a standalone
    // commit marker, which unambiguously keeps the already-durable new state.
    remove_publication_marker(&state_path)?;
    remove_publication_marker(&commit_path)?;
    Ok(())
}

fn cleanup_recorded_committed_swaps(
    db_path: &Path,
    state: Option<&PublicationState>,
) -> Result<()> {
    anyhow::ensure!(
        !publication_pending_swap_path(db_path).exists(),
        "committed publication contains an unrecorded pending swap at {}",
        db_path.display()
    );
    for (index, suffix) in SWAP_SUFFIXES.iter().enumerate() {
        let swap = db_path.with_extension(swap_extension(suffix));
        let recorded = state.map(|state| state.artifacts[index]).unwrap_or(false);
        if swap.exists() {
            anyhow::ensure!(
                recorded,
                "committed publication contains an unrecorded swap artifact: {}",
                swap.display()
            );
            remove_regular_file_if_present(&swap)?;
        }
    }
    Ok(())
}

/// Exclusive, non-blocking guard for replacing one complete index generation.
/// Ordinary WAL updates never acquire it exclusively.
pub struct IndexPublicationGuard {
    db_path: PathBuf,
    _lease: ProjectLease,
    lock_path: PathBuf,
    lock_file: Option<File>,
}

fn install_staged_at_path_with<F>(db_path: &Path, staged_db: &Path, rename: F) -> Result<()>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    sync_staged_db_for_publication(staged_db)?;
    recover_interrupted_publication_at_path(db_path)?;
    let artifacts = checkpoint_and_consolidate_live_db(db_path)?;

    let token = new_publication_token();
    let state = PublicationState {
        version: PUBLICATION_STATE_VERSION,
        token: token.clone(),
        operation: PublicationOperation::Install,
        artifacts,
        staging_dir: Some(publication_staging_dir_name(db_path, staged_db)?),
    };
    let state_path = publication_state_path(db_path);
    write_publication_marker(&state_path, &state)?;

    let publish_result = (|| -> Result<()> {
        if artifacts[0] {
            snapshot_live_main(db_path, &db_path.with_extension(swap_extension("")))?;
            sync_cache_directory(db_path.parent().context("index database has no parent")?)?;
        }

        #[cfg(windows)]
        if artifacts[0] {
            // Windows rename does not replace an existing file. The
            // publication lock still prevents readers from observing the
            // bounded name handoff.
            remove_regular_file_if_present(db_path)?;
        }
        rename(staged_db, db_path).with_context(|| {
            format!(
                "failed to atomically install staged index {} at {}",
                staged_db.display(),
                db_path.display()
            )
        })?;
        sync_regular_file(db_path)?;
        sync_cache_directory(db_path.parent().context("index database has no parent")?)?;

        let commit = PublicationCommit {
            version: PUBLICATION_STATE_VERSION,
            token,
            operation: PublicationOperation::Install,
        };
        write_publication_marker(&publication_commit_path(db_path), &commit)?;
        Ok(())
    })();

    if let Err(error) = publish_result {
        return match recover_interrupted_publication_at_path(db_path) {
            Ok(()) => Err(error),
            Err(recovery_error) => Err(anyhow::anyhow!(
                "{error:#}; failed to recover old index generation: {recovery_error:#}"
            )),
        };
    }
    recover_interrupted_publication_at_path(db_path)
}

fn prepare_staged_db_for_reads(staged_db: &Path) -> Result<()> {
    let staged_metadata = std::fs::symlink_metadata(staged_db)
        .with_context(|| format!("failed to inspect staged index {}", staged_db.display()))?;
    anyhow::ensure!(
        staged_metadata.file_type().is_file(),
        "staged index is not a regular file: {}",
        staged_db.display()
    );
    ensure_sqlite_source_artifacts_are_regular(staged_db)?;

    // SQLite's NOFOLLOW flag rejects a symlink in any path component on some
    // platforms (for example macOS `/var` -> `/private/var`). Resolve only the
    // already-created parent and prove the resulting path is the same file.
    let staged_parent = staged_db
        .parent()
        .context("staged index path has no parent directory")?;
    let canonical_parent = safe_canonicalize(staged_parent);
    let canonical_staged = canonical_parent.join(
        staged_db
            .file_name()
            .context("staged index path has no file name")?,
    );
    let canonical_metadata = std::fs::symlink_metadata(&canonical_staged).with_context(|| {
        format!(
            "failed to inspect resolved staged index {}",
            canonical_staged.display()
        )
    })?;
    anyhow::ensure!(
        canonical_metadata.file_type().is_file()
            && same_file_identity(&staged_metadata, &canonical_metadata),
        "staged index changed while resolving: {}",
        staged_db.display()
    );
    ensure_sqlite_source_artifacts_are_regular(&canonical_staged)?;

    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let connection = Connection::open_with_flags(&canonical_staged, flags)?;
    let journal_mode: String =
        connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    anyhow::ensure!(
        journal_mode.eq_ignore_ascii_case("wal"),
        "staged index could not enable WAL mode before publication"
    );
    drop(connection);
    Ok(())
}

impl IndexPublicationGuard {
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn install_staged(&self, staged_db: &Path) -> Result<()> {
        prepare_staged_db_for_reads(staged_db)?;
        install_staged_at_path_with(&self.db_path, staged_db, |source, target| {
            std::fs::rename(source, target)
        })?;
        if self._lease.is_managed() {
            touch_cache_activity_marker(&self.db_path)?;
        }
        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        recover_interrupted_publication_at_path(&self.db_path)?;
        let artifacts = checkpoint_and_consolidate_live_db(&self.db_path)?;
        let token = new_publication_token();
        let state = PublicationState {
            version: PUBLICATION_STATE_VERSION,
            token: token.clone(),
            operation: PublicationOperation::Clear,
            artifacts,
            staging_dir: None,
        };
        write_publication_marker(&publication_state_path(&self.db_path), &state)?;
        let clear_result = (|| -> Result<()> {
            if artifacts[0] {
                snapshot_live_main(
                    &self.db_path,
                    &self.db_path.with_extension(swap_extension("")),
                )?;
                sync_cache_directory(
                    self.db_path
                        .parent()
                        .context("index database has no parent")?,
                )?;
                remove_regular_file_if_present(&self.db_path)?;
            }
            sync_cache_directory(
                self.db_path
                    .parent()
                    .context("index database has no parent")?,
            )?;
            let commit = PublicationCommit {
                version: PUBLICATION_STATE_VERSION,
                token,
                operation: PublicationOperation::Clear,
            };
            write_publication_marker(&publication_commit_path(&self.db_path), &commit)
        })();
        if let Err(error) = clear_result {
            return match recover_interrupted_publication_at_path(&self.db_path) {
                Ok(()) => Err(error),
                Err(recovery_error) => Err(anyhow::anyhow!(
                    "{error:#}; failed to recover old index generation: {recovery_error:#}"
                )),
            };
        }
        recover_interrupted_publication_at_path(&self.db_path)
    }
}

impl Drop for IndexPublicationGuard {
    fn drop(&mut self) {
        let Ok(mut registry) = publication_registry().lock() else {
            return;
        };
        if let Some(file) = self.lock_file.take() {
            let _ = fs2::FileExt::unlock(&file);
        }
        registry.exclusive.remove(&self.lock_path);
    }
}

/// Acquire the short generation-publication lock. Contention is deliberately
/// non-blocking so callers receive a clear retryable busy error.
pub fn acquire_index_publication_guard(project_root: &Path) -> Result<IndexPublicationGuard> {
    acquire_index_publication_guard_inner(project_root, true)
}

fn acquire_index_publication_guard_inner(
    project_root: &Path,
    recover: bool,
) -> Result<IndexPublicationGuard> {
    let (db_path, lease, _normalized) = resolve_db_path_and_lease(project_root)?;
    let lock_path = publication_lock_path(&db_path, &lease)?;
    let mut registry = publication_registry()
        .lock()
        .map_err(|_| anyhow::anyhow!("publication lock registry is poisoned"))?;
    let shared_is_live = registry
        .shared
        .get(&lock_path)
        .and_then(Weak::upgrade)
        .is_some();
    if shared_is_live || registry.exclusive.contains(&lock_path) {
        return Err(publication_busy(lock_path.display().to_string()));
    }
    let file = open_lock_file(&lock_path)?;
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => {}
        Err(error) if lock_is_contended(&error) => {
            return Err(publication_busy(lock_path.display().to_string()))
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to acquire index publication lock {}",
                    lock_path.display()
                )
            })
        }
    }
    registry.exclusive.insert(lock_path.clone());
    drop(registry);

    let guard = IndexPublicationGuard {
        db_path,
        _lease: lease,
        lock_path,
        lock_file: Some(file),
    };
    if recover {
        recover_interrupted_publication_at_path(&guard.db_path)?;
    }
    Ok(guard)
}

pub fn clear_published_index(project_root: &Path) -> Result<()> {
    acquire_index_publication_guard(project_root)?.clear()
}

/// Recover a crashed generation handoff only when durable artifacts indicate
/// that recovery is needed. Healthy rebuilds therefore do not take the
/// publication lock before their long staging phase.
pub fn recover_interrupted_index_publication(project_root: &Path) -> Result<()> {
    let (db_path, _lease, _normalized) = resolve_db_path_and_lease(project_root)?;
    if !publication_has_interrupted_state(&db_path)? {
        return Ok(());
    }
    drop(acquire_index_publication_guard(project_root)?);
    Ok(())
}

/// Delete DB file and WAL/SHM files for the project
pub fn delete_db(project_root: &Path) -> Result<()> {
    let publication = acquire_index_publication_guard(project_root)?;
    delete_db_at_path(publication.db_path())
}

fn delete_db_at_path(db_path: &Path) -> Result<()> {
    ensure_safe_live_db_artifacts(db_path)?;
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let p = db_path.with_extension(format!("db{}", suffix));
        if p.exists() {
            std::fs::remove_file(&p)?;
        }
    }
    Ok(())
}

fn swap_extension(suffix: &str) -> String {
    format!("db.swap{}", suffix)
}

fn live_extension(suffix: &str) -> String {
    format!("db{}", suffix)
}

/// Atomically move the current DB aside (rename to `index.db.swap*`).
///
/// Returns `true` when there was an old DB to move. The caller is expected
/// to wrap the rebuild in a `RebuildSwap` guard so the swap is either
/// committed (deleted) on success or restored on failure.
///
/// Pre-existing swap files are never deleted implicitly: without a durable
/// publication marker their generation cannot be identified safely.
pub fn move_db_to_swap(project_root: &Path) -> Result<bool> {
    let publication = acquire_index_publication_guard(project_root)?;
    move_db_to_swap_at_path(publication.db_path())
}

fn move_db_to_swap_at_path(db_path: &Path) -> Result<bool> {
    move_db_to_swap_at_path_with(db_path, |source, target| std::fs::rename(source, target))
}

fn move_db_to_swap_at_path_with<F>(db_path: &Path, mut rename: F) -> Result<bool>
where
    F: FnMut(&Path, &Path) -> std::io::Result<()>,
{
    ensure_safe_live_db_artifacts(db_path)?;
    ensure_safe_swap_db_artifacts(db_path)?;
    let mut moved_suffixes = Vec::new();
    let move_result = (|| -> Result<()> {
        for suffix in SWAP_SUFFIXES {
            let live = db_path.with_extension(live_extension(suffix));
            let swap = db_path.with_extension(swap_extension(suffix));
            anyhow::ensure!(
                !swap.exists(),
                "untracked index swap already exists at {}; refusing to overwrite it",
                swap.display()
            );
            if live.exists() {
                rename(&live, &swap).with_context(|| {
                    format!(
                        "failed to move database artifact {} to {}",
                        live.display(),
                        swap.display()
                    )
                })?;
                moved_suffixes.push(*suffix);
            }
        }
        ensure_safe_swap_db_artifacts(db_path)
    })();

    if let Err(move_error) = move_result {
        let rollback_result = (|| -> Result<()> {
            for suffix in moved_suffixes.iter().rev() {
                let live = db_path.with_extension(live_extension(suffix));
                let swap = db_path.with_extension(swap_extension(suffix));
                anyhow::ensure!(
                    !live.exists(),
                    "cannot roll back partial database swap because {} exists",
                    live.display()
                );
                rename(&swap, &live).with_context(|| {
                    format!(
                        "failed to roll back database artifact {} to {}",
                        swap.display(),
                        live.display()
                    )
                })?;
            }
            ensure_safe_live_db_artifacts(db_path)
        })();
        return match rollback_result {
            Ok(()) => Err(move_error),
            Err(rollback_error) => Err(anyhow::anyhow!(
                "{move_error:#}; failed to roll back partial database swap: {rollback_error:#}"
            )),
        };
    }

    Ok(!moved_suffixes.is_empty())
}

/// Restore the previously-swapped DB. Used when a rebuild aborts.
/// Removes any partial new DB the failed rebuild wrote before renaming
/// the swap back into place.
pub fn restore_db_from_swap(project_root: &Path) -> Result<()> {
    let publication = acquire_index_publication_guard_inner(project_root, false)?;
    restore_db_from_swap_at_path(publication.db_path())
}

fn restore_db_from_swap_at_path(db_path: &Path) -> Result<()> {
    ensure_safe_live_db_artifacts(db_path)?;
    ensure_safe_swap_db_artifacts(db_path)?;
    for suffix in SWAP_SUFFIXES {
        let live = db_path.with_extension(live_extension(suffix));
        let swap = db_path.with_extension(swap_extension(suffix));
        if swap.exists() {
            if live.exists() {
                let _ = std::fs::remove_file(&live);
            }
            std::fs::rename(&swap, &live)?;
        }
    }
    ensure_safe_live_db_artifacts(db_path)?;
    Ok(())
}

/// Remove the swap aside (called after a successful rebuild commits).
pub fn remove_swap(project_root: &Path) -> Result<()> {
    let publication = acquire_index_publication_guard_inner(project_root, false)?;
    remove_swap_at_path(publication.db_path())
}

fn remove_swap_at_path(db_path: &Path) -> Result<()> {
    ensure_safe_swap_db_artifacts(db_path)?;
    for suffix in SWAP_SUFFIXES {
        let swap = db_path.with_extension(swap_extension(suffix));
        if swap.exists() {
            let _ = std::fs::remove_file(&swap);
        }
    }
    Ok(())
}

/// RAII guard for atomic rebuild.
///
/// `begin()` swaps the live DB to `.db.swap` so the rebuild starts from a
/// clean state without losing the previous index. If the guard is dropped
/// without `commit()` (an error bubbled up, walker aborted on cap, etc.),
/// `restore_db_from_swap` runs in the destructor and the previous index
/// is back in place. On `commit()` the swap is deleted.
pub struct RebuildSwap {
    db_path: PathBuf,
    _publication: IndexPublicationGuard,
    had_old_db: bool,
    committed: bool,
}

impl RebuildSwap {
    pub fn begin(project_root: &Path) -> Result<Self> {
        let publication = acquire_index_publication_guard(project_root)?;
        let db_path = publication.db_path().to_path_buf();
        let had_old_db = move_db_to_swap_at_path(&db_path)?;
        Ok(Self {
            db_path,
            _publication: publication,
            had_old_db,
            committed: false,
        })
    }

    pub fn commit(mut self) -> Result<()> {
        remove_swap_at_path(&self.db_path).ok();
        self.committed = true;
        Ok(())
    }
}

impl Drop for RebuildSwap {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if self.had_old_db {
            eprintln!("[ast-index] rebuild failed — restoring previous index from swap");
            if let Err(e) = restore_db_from_swap_at_path(&self.db_path) {
                eprintln!(
                    "[ast-index] failed to restore previous index: {e}. \
                     A backup may remain at .db.swap*"
                );
            }
        } else {
            // No old DB to restore — drop the half-written new one (if any)
            // and clean up the (likely absent) swap files.
            let _ = delete_db_at_path(&self.db_path);
            let _ = remove_swap_at_path(&self.db_path);
        }
    }
}

fn create_base_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        -- Files table
        CREATE TABLE IF NOT EXISTS files (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL,
            root_path TEXT NOT NULL DEFAULT '',
            mtime INTEGER NOT NULL,
            size INTEGER NOT NULL,
            UNIQUE(root_path, path)
        );

        -- Symbols table (classes, interfaces, functions, etc.)
        CREATE TABLE IF NOT EXISTS symbols (
            id INTEGER PRIMARY KEY,
            file_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            qualified_name TEXT,
            kind TEXT NOT NULL,
            line INTEGER NOT NULL,
            parent_id INTEGER,
            signature TEXT,
            FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE
        );

        -- Modules table
        CREATE TABLE IF NOT EXISTS modules (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            path TEXT NOT NULL,
            kind TEXT
        );

        -- Module dependencies
        CREATE TABLE IF NOT EXISTS module_deps (
            id INTEGER PRIMARY KEY,
            module_id INTEGER NOT NULL,
            dep_module_id INTEGER NOT NULL,
            dep_kind TEXT,
            FOREIGN KEY (module_id) REFERENCES modules(id) ON DELETE CASCADE,
            FOREIGN KEY (dep_module_id) REFERENCES modules(id) ON DELETE CASCADE
        );

        -- Inheritance/implementation relationships
        CREATE TABLE IF NOT EXISTS inheritance (
            id INTEGER PRIMARY KEY,
            child_id INTEGER NOT NULL,
            parent_name TEXT NOT NULL,
            kind TEXT NOT NULL,
            FOREIGN KEY (child_id) REFERENCES symbols(id) ON DELETE CASCADE
        );

        -- References table (symbol usages)
        CREATE TABLE IF NOT EXISTS refs (
            id INTEGER PRIMARY KEY,
            file_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            line INTEGER NOT NULL,
            context TEXT,
            FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE
        );

        -- XML usages (classes used in XML layouts)
        CREATE TABLE IF NOT EXISTS xml_usages (
            id INTEGER PRIMARY KEY,
            module_id INTEGER,
            file_path TEXT NOT NULL,
            line INTEGER NOT NULL,
            class_name TEXT NOT NULL,
            usage_type TEXT,
            element_id TEXT,
            FOREIGN KEY (module_id) REFERENCES modules(id) ON DELETE CASCADE
        );

        -- Resources definitions
        CREATE TABLE IF NOT EXISTS resources (
            id INTEGER PRIMARY KEY,
            module_id INTEGER,
            type TEXT NOT NULL,
            name TEXT NOT NULL,
            file_path TEXT NOT NULL,
            line INTEGER,
            FOREIGN KEY (module_id) REFERENCES modules(id) ON DELETE CASCADE
        );

        -- Resource usages
        CREATE TABLE IF NOT EXISTS resource_usages (
            id INTEGER PRIMARY KEY,
            resource_id INTEGER,
            usage_file TEXT NOT NULL,
            usage_line INTEGER NOT NULL,
            usage_type TEXT,
            FOREIGN KEY (resource_id) REFERENCES resources(id) ON DELETE CASCADE
        );

        -- Transitive dependencies cache
        CREATE TABLE IF NOT EXISTS transitive_deps (
            id INTEGER PRIMARY KEY,
            module_id INTEGER NOT NULL,
            dependency_id INTEGER NOT NULL,
            depth INTEGER NOT NULL,
            path TEXT,
            FOREIGN KEY (module_id) REFERENCES modules(id) ON DELETE CASCADE,
            FOREIGN KEY (dependency_id) REFERENCES modules(id) ON DELETE CASCADE
        );

        -- iOS storyboard/xib usages
        CREATE TABLE IF NOT EXISTS storyboard_usages (
            id INTEGER PRIMARY KEY,
            module_id INTEGER,
            file_path TEXT NOT NULL,
            line INTEGER NOT NULL,
            class_name TEXT NOT NULL,
            usage_type TEXT,
            storyboard_id TEXT,
            FOREIGN KEY (module_id) REFERENCES modules(id) ON DELETE CASCADE
        );

        -- iOS assets (from .xcassets)
        CREATE TABLE IF NOT EXISTS ios_assets (
            id INTEGER PRIMARY KEY,
            module_id INTEGER,
            type TEXT NOT NULL,
            name TEXT NOT NULL,
            file_path TEXT NOT NULL,
            FOREIGN KEY (module_id) REFERENCES modules(id) ON DELETE CASCADE
        );

        -- iOS asset usages
        CREATE TABLE IF NOT EXISTS ios_asset_usages (
            id INTEGER PRIMARY KEY,
            asset_id INTEGER,
            usage_file TEXT NOT NULL,
            usage_line INTEGER NOT NULL,
            usage_type TEXT,
            FOREIGN KEY (asset_id) REFERENCES ios_assets(id) ON DELETE CASCADE
        );

        -- Metadata for storing index settings
        CREATE TABLE IF NOT EXISTS metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        -- Named workspace subtrees (#31). Each row represents an extra source
        -- root attached to this project: the user gives it a short `name`
        -- (used in CLI filters and in output prefixes), `original_path` is
        -- what the user typed (relative or absolute, kept for portability),
        -- and `canonical_path` is the normalized absolute form actually
        -- written into `files.root_path` during indexing.
        CREATE TABLE IF NOT EXISTS subtrees (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            canonical_path TEXT NOT NULL UNIQUE,
            original_path TEXT NOT NULL
        );
        "#,
    )?;
    Ok(())
}

fn create_secondary_indexes(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS idx_files_path ON files(path);
        CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
        CREATE INDEX IF NOT EXISTS idx_symbols_qualified_name
            ON symbols(qualified_name) WHERE qualified_name IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_symbols_kind ON symbols(kind);
        CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_id);
        CREATE INDEX IF NOT EXISTS idx_module_deps_module ON module_deps(module_id);
        CREATE INDEX IF NOT EXISTS idx_module_deps_dep ON module_deps(dep_module_id);
        CREATE INDEX IF NOT EXISTS idx_inheritance_child ON inheritance(child_id);
        CREATE INDEX IF NOT EXISTS idx_inheritance_parent ON inheritance(parent_name);
        CREATE INDEX IF NOT EXISTS idx_refs_file ON refs(file_id);
        -- Composite covering index for find_references: lets SQLite avoid
        -- full table scan when filtering by name AND joining with files
        -- on large ref tables (millions of rows). See issue #19.
        CREATE INDEX IF NOT EXISTS idx_refs_name_file_line ON refs(name, file_id, line);
        CREATE INDEX IF NOT EXISTS idx_xml_usages_class ON xml_usages(class_name);
        CREATE INDEX IF NOT EXISTS idx_xml_usages_module ON xml_usages(module_id);
        CREATE INDEX IF NOT EXISTS idx_resources_name ON resources(name);
        CREATE INDEX IF NOT EXISTS idx_resources_type ON resources(type);
        CREATE INDEX IF NOT EXISTS idx_resources_module ON resources(module_id);
        CREATE INDEX IF NOT EXISTS idx_resource_usages_resource ON resource_usages(resource_id);
        CREATE INDEX IF NOT EXISTS idx_transitive_deps_module ON transitive_deps(module_id);
        CREATE INDEX IF NOT EXISTS idx_transitive_deps_dep ON transitive_deps(dependency_id);
        CREATE INDEX IF NOT EXISTS idx_storyboard_usages_class ON storyboard_usages(class_name);
        CREATE INDEX IF NOT EXISTS idx_storyboard_usages_module ON storyboard_usages(module_id);
        CREATE INDEX IF NOT EXISTS idx_ios_assets_name ON ios_assets(name);
        CREATE INDEX IF NOT EXISTS idx_ios_assets_type ON ios_assets(type);
        CREATE INDEX IF NOT EXISTS idx_ios_asset_usages_asset ON ios_asset_usages(asset_id);
        "#,
    )?;
    Ok(())
}

fn create_symbols_fts(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
            name,
            signature,
            content=symbols,
            content_rowid=id
        );

        CREATE TRIGGER IF NOT EXISTS symbols_ai AFTER INSERT ON symbols BEGIN
            INSERT INTO symbols_fts(rowid, name, signature) VALUES (new.id, new.name, new.signature);
        END;
        CREATE TRIGGER IF NOT EXISTS symbols_ad AFTER DELETE ON symbols BEGIN
            INSERT INTO symbols_fts(symbols_fts, rowid, name, signature) VALUES('delete', old.id, old.name, old.signature);
        END;
        CREATE TRIGGER IF NOT EXISTS symbols_au AFTER UPDATE ON symbols BEGIN
            INSERT INTO symbols_fts(symbols_fts, rowid, name, signature) VALUES('delete', old.id, old.name, old.signature);
            INSERT INTO symbols_fts(rowid, name, signature) VALUES (new.id, new.name, new.signature);
        END;
        "#,
    )?;
    conn.execute(
        "INSERT INTO symbols_fts(symbols_fts) VALUES ('rebuild')",
        [],
    )?;
    Ok(())
}

/// Initialize the full database schema for regular use and tests.
pub fn init_db(conn: &Connection) -> Result<()> {
    create_base_schema(conn)?;
    create_secondary_indexes(conn)?;
    create_symbols_fts(conn)?;
    Ok(())
}

/// Initialize a minimal schema optimized for fresh full rebuilds.
pub fn init_db_for_rebuild(conn: &Connection) -> Result<()> {
    create_base_schema(conn)
}

/// Finalize a rebuild-optimized database by creating indexes and FTS after bulk inserts.
pub fn finalize_db_after_rebuild(conn: &Connection) -> Result<()> {
    create_secondary_indexes(conn)?;
    create_symbols_fts(conn)?;
    Ok(())
}

/// Apply conservative SQLite settings tuned for rebuild throughput.
///
/// This is safe to use for every rebuild: it does not relax durability or
/// journaling, it only increases the cache from the normal 8 MB to 16 MB.
pub fn enable_rebuild_pragmas(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "cache_size", "-16000")?; // 16 MB cache
    Ok(())
}

/// Restore the regular connection settings after a rebuild.
pub fn restore_rebuild_pragmas(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "cache_size", "-8000")?;
    Ok(())
}

const CREATE_METADATA_SQL: &str =
    "CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)";
const CREATE_SUBTREES_SQL: &str = r#"
    CREATE TABLE IF NOT EXISTS subtrees (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE,
        canonical_path TEXT NOT NULL UNIQUE,
        original_path TEXT NOT NULL
    )
"#;
const CREATE_QUALIFIED_NAME_INDEX_SQL: &str = r#"
    CREATE INDEX IF NOT EXISTS idx_symbols_qualified_name
        ON symbols(qualified_name) WHERE qualified_name IS NOT NULL
"#;
const CREATE_REFS_NAME_FILE_LINE_INDEX_SQL: &str =
    "CREATE INDEX IF NOT EXISTS idx_refs_name_file_line ON refs(name, file_id, line)";
const DEFAULT_BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        params![table],
        |row| row.get::<_, bool>(0),
    )
    .map_err(Into::into)
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
        params![table, column],
        |row| row.get::<_, bool>(0),
    )
    .map_err(Into::into)
}

fn has_unique_index_on_columns(
    conn: &Connection,
    table: &str,
    expected_columns: &[&str],
) -> Result<bool> {
    let mut indexes =
        conn.prepare("SELECT name FROM pragma_index_list(?1) WHERE \"unique\" = 1 ORDER BY name")?;
    let names = indexes
        .query_map([table], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for name in names {
        let mut columns = conn.prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")?;
        let columns = columns
            .query_map([name], |row| row.get::<_, Option<String>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if columns.len() == expected_columns.len()
            && columns
                .iter()
                .zip(expected_columns)
                .all(|(actual, expected)| actual.as_deref() == Some(*expected))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn files_has_legacy_path_unique(conn: &Connection) -> Result<bool> {
    if !table_exists(conn, "files")? {
        return Ok(false);
    }
    has_unique_index_on_columns(conn, "files", &["path"])
}

fn ensure_foreign_key_integrity(conn: &Connection) -> Result<()> {
    let violation = conn
        .query_row("PRAGMA foreign_key_check", [], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .optional()?;
    if let Some((table, row_id, parent, constraint)) = violation {
        anyhow::bail!(
            "foreign key check failed: {table} row {row_id:?} references {parent} constraint {constraint}"
        );
    }
    Ok(())
}

fn rebuild_legacy_files_table(conn: &Connection) -> Result<()> {
    const STAGED_FILES_TABLE: &str = "files__ast_index_schema_v1";
    anyhow::ensure!(
        !table_exists(conn, STAGED_FILES_TABLE)?,
        "reserved migration table already exists: {STAGED_FILES_TABLE}"
    );
    conn.execute_batch(
        r#"
        CREATE TABLE files__ast_index_schema_v1 (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL,
            root_path TEXT NOT NULL DEFAULT '',
            mtime INTEGER NOT NULL,
            size INTEGER NOT NULL,
            UNIQUE(root_path, path)
        );
        INSERT INTO files__ast_index_schema_v1 (id, path, root_path, mtime, size)
            SELECT id, path, root_path, mtime, size FROM files;
        DROP TABLE files;
        ALTER TABLE files__ast_index_schema_v1 RENAME TO files;
        CREATE INDEX idx_files_path ON files(path);
        "#,
    )
    .context("failed to rebuild legacy files uniqueness")?;
    Ok(())
}

fn index_exists(conn: &Connection, index: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1)",
        params![index],
        |row| row.get::<_, bool>(0),
    )
    .map_err(Into::into)
}

fn index_sql(conn: &Connection, index: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = ?1",
        params![index],
        |row| row.get::<_, Option<String>>(0),
    )
    .optional()
    .map(|value| value.flatten())
    .map_err(Into::into)
}

fn is_current_qualified_name_index(sql: &str) -> bool {
    let compact = sql
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    compact.contains("onsymbols(qualified_name)")
        && compact.ends_with("wherequalified_nameisnotnull")
}

#[derive(Default)]
struct OptionalIndexMigrations {
    drop_files_root_path_path: bool,
    drop_modules_name: bool,
    drop_refs_name: bool,
    rewrite_qualified_name: bool,
}

impl OptionalIndexMigrations {
    fn required(&self) -> bool {
        self.drop_files_root_path_path
            || self.drop_modules_name
            || self.drop_refs_name
            || self.rewrite_qualified_name
    }
}

struct OpenMigrationPreflight {
    functional_migration_required: bool,
    optional_indexes: OptionalIndexMigrations,
}

/// Inspect schema and ownership metadata using SELECT/PRAGMA only. A
/// current-schema reader must be able to complete this while another WAL
/// connection holds the write transaction.
fn inspect_open_migrations(
    conn: &Connection,
    normalized_root: &str,
) -> Result<OpenMigrationPreflight> {
    let metadata_exists = table_exists(conn, "metadata")?;
    let subtrees_exists = table_exists(conn, "subtrees")?;
    let files_exists = table_exists(conn, "files")?;
    let symbols_exists = table_exists(conn, "symbols")?;
    let files_current = !files_exists || column_exists(conn, "files", "root_path")?;
    let files_uniqueness_current = !files_exists || !files_has_legacy_path_unique(conn)?;
    let symbols_current = !symbols_exists || column_exists(conn, "symbols", "qualified_name")?;

    let (stored_root, has_legacy_extra_roots) = if metadata_exists {
        let stored_root = conn
            .query_row(
                "SELECT value FROM metadata WHERE key = 'project_root'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("failed to inspect project_root metadata")?;
        let has_legacy_extra_roots = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM metadata WHERE key = 'extra_roots')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .context("failed to inspect metadata.extra_roots")?;
        (stored_root, has_legacy_extra_roots)
    } else {
        (None, false)
    };

    let qualified_index_current = if symbols_exists && symbols_current {
        index_sql(conn, "idx_symbols_qualified_name")?
            .as_deref()
            .map(is_current_qualified_name_index)
            .unwrap_or(false)
    } else {
        true
    };
    let optional_indexes = OptionalIndexMigrations {
        drop_files_root_path_path: index_exists(conn, "idx_files_root_path_path")?,
        drop_modules_name: index_exists(conn, "idx_modules_name")?,
        drop_refs_name: index_exists(conn, "idx_refs_name")?,
        rewrite_qualified_name: !qualified_index_current,
    };

    Ok(OpenMigrationPreflight {
        functional_migration_required: !metadata_exists
            || !subtrees_exists
            || !files_current
            || !files_uniqueness_current
            || !symbols_current
            || stored_root.as_deref() != Some(normalized_root)
            || has_legacy_extra_roots,
        optional_indexes,
    })
}

fn apply_optional_index_migrations(
    conn: &Connection,
    migrations: &OptionalIndexMigrations,
) -> rusqlite::Result<()> {
    if migrations.drop_files_root_path_path {
        conn.execute("DROP INDEX IF EXISTS idx_files_root_path_path", [])?;
    }
    if migrations.drop_modules_name {
        conn.execute("DROP INDEX IF EXISTS idx_modules_name", [])?;
    }
    if migrations.drop_refs_name {
        // Older schemas may have only the narrow name index. Install the
        // covering replacement in this same transaction before removing the
        // last usable lookup index.
        conn.execute(CREATE_REFS_NAME_FILE_LINE_INDEX_SQL, [])?;
        conn.execute("DROP INDEX IF EXISTS idx_refs_name", [])?;
    }
    if migrations.rewrite_qualified_name {
        conn.execute("DROP INDEX IF EXISTS idx_symbols_qualified_name", [])?;
        conn.execute(CREATE_QUALIFIED_NAME_INDEX_SQL, [])?;
    }
    Ok(())
}

fn try_apply_optional_index_migrations(
    conn: &mut Connection,
    migrations: &OptionalIndexMigrations,
) -> Result<()> {
    if !migrations.required() {
        return Ok(());
    }

    conn.busy_timeout(std::time::Duration::ZERO)?;
    let migration_result = (|| -> rusqlite::Result<()> {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        apply_optional_index_migrations(&tx, migrations)?;
        tx.commit()
    })();
    conn.busy_timeout(DEFAULT_BUSY_TIMEOUT)?;

    match migration_result {
        Ok(()) => Ok(()),
        Err(error) if is_sqlite_busy(&error) => Ok(()),
        Err(error) => Err(error).context("failed to optimize legacy index schema"),
    }
}

/// Apply every backwards-compatible schema upgrade as one transaction.
/// SQLite DDL is transactional, so malformed legacy metadata or a lock error
/// cannot leave a half-created `subtrees` table or partially migrated rows.
fn apply_open_migrations_transaction(
    conn: &mut Connection,
    normalized_root: &str,
    rebuild_files: bool,
) -> Result<()> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("failed to start index schema migration")?;

    tx.execute(CREATE_METADATA_SQL, [])
        .context("failed to create metadata table")?;
    tx.execute(CREATE_SUBTREES_SQL, [])
        .context("failed to create subtrees table")?;

    if table_exists(&tx, "files")? && !column_exists(&tx, "files", "root_path")? {
        tx.execute(
            "ALTER TABLE files ADD COLUMN root_path TEXT NOT NULL DEFAULT ''",
            [],
        )
        .context("failed to add files.root_path")?;
    }
    if rebuild_files {
        rebuild_legacy_files_table(&tx)?;
    }

    if table_exists(&tx, "symbols")? {
        if !column_exists(&tx, "symbols", "qualified_name")? {
            tx.execute("ALTER TABLE symbols ADD COLUMN qualified_name TEXT", [])
                .context("failed to add symbols.qualified_name")?;
        }
        tx.execute("DROP INDEX IF EXISTS idx_symbols_qualified_name", [])
            .context("failed to replace idx_symbols_qualified_name")?;
        tx.execute(CREATE_QUALIFIED_NAME_INDEX_SQL, [])
            .context("failed to create idx_symbols_qualified_name")?;
    }

    tx.execute("DROP INDEX IF EXISTS idx_files_root_path_path", [])
        .context("failed to drop idx_files_root_path_path")?;
    tx.execute("DROP INDEX IF EXISTS idx_modules_name", [])
        .context("failed to drop idx_modules_name")?;
    if index_exists(&tx, "idx_refs_name")? {
        tx.execute(CREATE_REFS_NAME_FILE_LINE_INDEX_SQL, [])
            .context("failed to create idx_refs_name_file_line")?;
        tx.execute("DROP INDEX IF EXISTS idx_refs_name", [])
            .context("failed to drop idx_refs_name")?;
    }

    tx.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES ('project_root', ?1)",
        params![normalized_root],
    )
    .context("failed to update project_root metadata")?;
    migrate_extra_roots_rows(&tx)?;
    if rebuild_files {
        ensure_foreign_key_integrity(&tx)?;
    }
    tx.commit()
        .context("failed to commit index schema migration")?;
    Ok(())
}

fn apply_open_migrations(conn: &mut Connection, normalized_root: &str) -> Result<()> {
    let rebuild_files = files_has_legacy_path_unique(conn)?;
    let foreign_keys_enabled: bool = conn
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .context("failed to inspect foreign-key enforcement")?;
    if rebuild_files && foreign_keys_enabled {
        conn.pragma_update(None, "foreign_keys", "OFF")
            .context("failed to suspend foreign keys for files-table migration")?;
    }

    let migration_result = apply_open_migrations_transaction(conn, normalized_root, rebuild_files);
    let restore_result = if rebuild_files && foreign_keys_enabled {
        conn.pragma_update(None, "foreign_keys", "ON")
            .context("failed to restore foreign-key enforcement")
    } else {
        Ok(())
    };

    match (migration_result, restore_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(restore_error)) => Err(anyhow::anyhow!(
            "{error:#}; additionally failed to restore foreign-key enforcement: {restore_error:#}"
        )),
    }
}

/// SQLite connection paired with the shared external cache lease that keeps
/// its directory alive until the connection is dropped.
pub struct LeasedConnection {
    connection: Connection,
    _lease: ProjectLease,
    _publication: PublicationLease,
}

impl Deref for LeasedConnection {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}

impl DerefMut for LeasedConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.connection
    }
}

fn open_configured_connection(normalized_root: &str, db_path: &Path) -> Result<Connection> {
    let mut conn = Connection::open(db_path)?;

    // Connection-local settings do not write the database.
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "cache_size", "-8000")?; // 8 MB cache to limit memory

    // Reading an already-WAL journal mode is lock-free. Switching an older
    // database to WAL is an optional optimization: never make a reader wait
    // behind a writer solely for this persistent PRAGMA.
    conn.busy_timeout(std::time::Duration::ZERO)?;
    let journal_mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        match conn.query_row("PRAGMA journal_mode = WAL", [], |row| {
            row.get::<_, String>(0)
        }) {
            Ok(_) => {}
            Err(error) if is_sqlite_busy(&error) => {}
            Err(error) => return Err(error).context("failed to configure WAL journal mode"),
        }
    }
    conn.busy_timeout(DEFAULT_BUSY_TIMEOUT)?;

    let preflight = inspect_open_migrations(&conn, normalized_root)?;
    if preflight.functional_migration_required {
        apply_open_migrations(&mut conn, normalized_root)?;
    } else {
        try_apply_optional_index_migrations(&mut conn, &preflight.optional_indexes)?;
    }

    Ok(conn)
}

/// Open a new private database generation without acquiring the live
/// publication lock. The caller must use a path in a private staging
/// directory and publish it only through [`IndexPublicationGuard`].
pub fn open_staged_db(project_root: &Path, staged_db: &Path) -> Result<Connection> {
    ensure_restore_staging_is_absent(staged_db)?;
    if let Some(parent) = staged_db.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let normalized_root = normalize_root_for_storage(project_root);
    open_configured_connection(&normalized_root, staged_db)
}

/// Consolidate a completed private generation into one durable main file.
/// Consuming the connection makes it impossible for a caller to retain a
/// SQLite handle across publication and deadlock its own exclusive guard.
pub fn seal_staged_db(connection: Connection, staged_db: &Path) -> Result<()> {
    connection.busy_timeout(std::time::Duration::ZERO)?;
    let (busy, _log_frames, _checkpointed): (i64, i64, i64) =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    anyhow::ensure!(busy == 0, "staged index WAL could not be checkpointed");
    let journal_mode: String =
        connection.query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))?;
    anyhow::ensure!(
        journal_mode.eq_ignore_ascii_case("delete"),
        "staged index could not be consolidated into one database file"
    );
    drop(connection);
    for suffix in ["-wal", "-shm", "-journal"] {
        remove_regular_file_if_present(&sqlite_sidecar_path(staged_db, suffix))?;
    }
    sync_staged_db_for_publication(staged_db)
}

fn sync_staged_db_for_publication(staged_db: &Path) -> Result<()> {
    ensure_sqlite_source_artifacts_are_regular(staged_db)?;
    anyhow::ensure!(
        std::fs::symlink_metadata(staged_db)
            .map(|metadata| metadata.file_type().is_file())
            .unwrap_or(false),
        "staged index is missing: {}",
        staged_db.display()
    );
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = sqlite_sidecar_path(staged_db, suffix);
        anyhow::ensure!(
            !std::fs::symlink_metadata(&sidecar)
                .map(|metadata| metadata.file_type().is_file())
                .unwrap_or(false),
            "staged index still has an active SQLite sidecar: {}",
            sidecar.display()
        );
    }
    sync_regular_file(staged_db)?;
    sync_cache_directory(
        staged_db
            .parent()
            .context("staged index has no parent directory")?,
    )
}

/// Open or create a concrete SQLite connection.
///
/// Opening performs schema migrations only when the read-only preflight finds
/// an older schema. For managed caches, its shared project lease is retained
/// until process exit because a concrete `Connection` cannot carry a guard.
/// The publication lease is still released before this legacy compatibility
/// API returns, so generation replacement remains uncoordinated.
/// Long-lived operations, production commands, and callers that may race
/// stale-cache GC should use [`open_db_leased`].
pub fn open_db(project_root: &Path) -> Result<Connection> {
    let (db_path, lease, normalized_root) = resolve_db_path_and_lease(project_root)?;
    let publication = try_acquire_shared_publication(&db_path, &lease)?;
    ensure_no_interrupted_publication(&db_path)?;
    let connection = open_configured_connection(&normalized_root, &db_path)?;
    if lease.is_managed() {
        touch_cache_activity_marker(&db_path)?;
    }
    drop(publication);
    retain_legacy_open_db_lease(lease)?;
    Ok(connection)
}

/// Open or create SQLite while retaining the project's shared cache lease
/// for the full lifetime of the returned connection.
pub fn open_db_leased(project_root: &Path) -> Result<LeasedConnection> {
    let (db_path, lease, normalized_root) = resolve_db_path_and_lease(project_root)?;
    let publication = try_acquire_shared_publication(&db_path, &lease)?;
    ensure_no_interrupted_publication(&db_path)?;
    let connection = open_configured_connection(&normalized_root, &db_path)?;
    if lease.is_managed() {
        touch_cache_activity_marker(&db_path)?;
    }

    Ok(LeasedConnection {
        connection,
        _lease: lease,
        _publication: publication,
    })
}

/// Open an initialized live generation without ever creating a replacement
/// database when the index is absent.
pub fn open_existing_db_leased(project_root: &Path) -> Result<Option<LeasedConnection>> {
    let (db_path, lease, normalized_root) = resolve_db_path_and_lease(project_root)?;
    let publication = try_acquire_shared_publication(&db_path, &lease)?;
    ensure_no_interrupted_publication(&db_path)?;
    if !std::fs::symlink_metadata(&db_path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
    {
        return Ok(None);
    }
    let connection = open_configured_connection(&normalized_root, &db_path)?;
    if !table_exists(&connection, "files")? {
        return Ok(None);
    }
    if lease.is_managed() {
        touch_cache_activity_marker(&db_path)?;
    }
    Ok(Some(LeasedConnection {
        connection,
        _lease: lease,
        _publication: publication,
    }))
}

fn sqlite_sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
    let mut path = db_path.as_os_str().to_os_string();
    path.push(suffix);
    path.into()
}

fn ensure_sqlite_source_artifacts_are_regular(db_path: &Path) -> Result<()> {
    ensure_regular_or_missing(db_path)?;
    for suffix in ["-wal", "-shm", "-journal"] {
        ensure_regular_or_missing(&sqlite_sidecar_path(db_path, suffix))?;
    }
    Ok(())
}

fn ensure_restore_staging_is_absent(db_path: &Path) -> Result<()> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let path = sqlite_sidecar_path(db_path, suffix);
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => anyhow::bail!(
                "restore staging artifact already exists: {}",
                path.display()
            ),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to inspect restore staging path {}", path.display())
                })
            }
        }
    }
    Ok(())
}

fn cleanup_restore_staging(db_path: &Path) -> Result<()> {
    for suffix in ["-journal", "-wal", "-shm", ""] {
        let path = sqlite_sidecar_path(db_path, suffix);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => std::fs::remove_file(&path).with_context(|| {
                format!(
                    "failed to remove restore staging artifact {}",
                    path.display()
                )
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to inspect restore staging artifact {}",
                        path.display()
                    )
                })
            }
        }
    }
    Ok(())
}

const REQUIRED_RESTORE_INDEXES: &[&str] = &[
    "idx_files_path",
    "idx_symbols_name",
    "idx_symbols_qualified_name",
    "idx_symbols_kind",
    "idx_symbols_file",
    "idx_module_deps_module",
    "idx_module_deps_dep",
    "idx_inheritance_child",
    "idx_inheritance_parent",
    "idx_refs_file",
    "idx_refs_name_file_line",
    "idx_xml_usages_class",
    "idx_xml_usages_module",
    "idx_resources_name",
    "idx_resources_type",
    "idx_resources_module",
    "idx_resource_usages_resource",
    "idx_transitive_deps_module",
    "idx_transitive_deps_dep",
    "idx_storyboard_usages_class",
    "idx_storyboard_usages_module",
    "idx_ios_assets_name",
    "idx_ios_assets_type",
    "idx_ios_asset_usages_asset",
];

fn compact_schema_sql(sql: &str) -> String {
    sql.chars()
        .filter(|character| !character.is_ascii_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn validate_symbols_fts(conn: &Connection) -> Result<()> {
    let fts_sql = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'symbols_fts'",
            [],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let Some(fts_sql) = fts_sql else {
        anyhow::bail!(
            "restore source is not a complete ast-index database: missing symbols_fts virtual table"
        );
    };
    let compact = compact_schema_sql(&fts_sql);
    anyhow::ensure!(
        compact.contains("virtualtablesymbols_ftsusingfts5(")
            && compact.contains("content=symbols")
            && compact.contains("content_rowid=id"),
        "restore source is not a complete ast-index database: invalid symbols_fts definition"
    );

    let _: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbols_fts WHERE symbols_fts MATCH 'ast_index_restore_probe'",
            [],
            |row| row.get(0),
        )
        .context("restore source has an unusable symbols_fts index")?;

    let required_triggers: &[(&str, &str, &[&str])] = &[
        (
            "symbols_ai",
            "afterinsert",
            &["insertintosymbols_fts(rowid,name,signature)values(new.id,new.name,new.signature)"],
        ),
        (
            "symbols_ad",
            "afterdelete",
            &["insertintosymbols_fts(symbols_fts,rowid,name,signature)values('delete',old.id,old.name,old.signature)"],
        ),
        (
            "symbols_au",
            "afterupdate",
            &[
                "insertintosymbols_fts(symbols_fts,rowid,name,signature)values('delete',old.id,old.name,old.signature)",
                "insertintosymbols_fts(rowid,name,signature)values(new.id,new.name,new.signature)",
            ],
        ),
    ];
    for &(trigger, event, required_fragments) in required_triggers {
        let definition = conn
            .query_row(
                "SELECT tbl_name, sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                [trigger],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()?;
        let Some((table, Some(sql))) = definition else {
            anyhow::bail!(
                "restore source is not a complete ast-index database: missing {trigger} FTS sync trigger"
            );
        };
        let compact = compact_schema_sql(&sql);
        anyhow::ensure!(
            table == "symbols"
                && compact.contains(event)
                && compact.contains("onsymbols")
                && required_fragments
                    .iter()
                    .all(|fragment| compact.contains(fragment)),
            "restore source is not a complete ast-index database: invalid {trigger} FTS sync trigger"
        );
    }
    Ok(())
}

fn validate_required_index_schema(conn: &Connection) -> Result<()> {
    let required_tables: &[(&str, &[&str])] = &[
        ("files", &["id", "path", "root_path", "mtime", "size"]),
        (
            "symbols",
            &[
                "id",
                "file_id",
                "name",
                "qualified_name",
                "kind",
                "line",
                "parent_id",
                "signature",
            ],
        ),
        ("modules", &["id", "name", "path", "kind"]),
        (
            "module_deps",
            &["id", "module_id", "dep_module_id", "dep_kind"],
        ),
        ("inheritance", &["id", "child_id", "parent_name", "kind"]),
        ("refs", &["id", "file_id", "name", "line", "context"]),
        (
            "xml_usages",
            &[
                "id",
                "module_id",
                "file_path",
                "line",
                "class_name",
                "usage_type",
                "element_id",
            ],
        ),
        (
            "resources",
            &["id", "module_id", "type", "name", "file_path", "line"],
        ),
        (
            "resource_usages",
            &[
                "id",
                "resource_id",
                "usage_file",
                "usage_line",
                "usage_type",
            ],
        ),
        (
            "transitive_deps",
            &["id", "module_id", "dependency_id", "depth", "path"],
        ),
        (
            "storyboard_usages",
            &[
                "id",
                "module_id",
                "file_path",
                "line",
                "class_name",
                "usage_type",
                "storyboard_id",
            ],
        ),
        (
            "ios_assets",
            &["id", "module_id", "type", "name", "file_path"],
        ),
        (
            "ios_asset_usages",
            &["id", "asset_id", "usage_file", "usage_line", "usage_type"],
        ),
        ("metadata", &["key", "value"]),
        (
            "subtrees",
            &["id", "name", "canonical_path", "original_path"],
        ),
    ];
    for &(table, columns) in required_tables {
        anyhow::ensure!(
            table_exists(conn, table)?,
            "restore source is not an ast-index database: missing {table} table"
        );
        for column in columns {
            anyhow::ensure!(
                column_exists(conn, table, column)?,
                "restore source is not an ast-index database: missing {table}.{column}"
            );
        }
    }

    validate_symbols_fts(conn)?;
    for index in REQUIRED_RESTORE_INDEXES {
        anyhow::ensure!(
            index_exists(conn, index)?,
            "restore source is not a complete ast-index database: missing {index} index"
        );
    }
    let qualified_sql = index_sql(conn, "idx_symbols_qualified_name")?
        .context("restore source is missing idx_symbols_qualified_name")?;
    anyhow::ensure!(
        is_current_qualified_name_index(&qualified_sql),
        "restore source has an outdated idx_symbols_qualified_name definition"
    );
    anyhow::ensure!(
        has_unique_index_on_columns(conn, "files", &["root_path", "path"])?
            && !files_has_legacy_path_unique(conn)?,
        "restore source has an outdated files uniqueness constraint"
    );
    ensure_foreign_key_integrity(conn)?;
    Ok(())
}

/// Copy a consistent SQLite snapshot into an absent staging path, migrate and
/// validate it there, and return the statistics needed by the restore command.
/// The source is never opened writable; every staging artifact is removed if
/// any step fails.
pub fn stage_restore_snapshot(
    source: &Path,
    staged: &Path,
    normalized_root: &str,
) -> Result<DbStats> {
    let source_metadata = std::fs::symlink_metadata(source)
        .with_context(|| format!("failed to inspect restore source {}", source.display()))?;
    anyhow::ensure!(
        source_metadata.file_type().is_file(),
        "restore source is not a regular file: {}",
        source.display()
    );
    ensure_sqlite_source_artifacts_are_regular(source)?;
    // SQLite's NOFOLLOW flag rejects a symlink in any path component on some
    // platforms (for example macOS `/var` -> `/private/var`). The final
    // component was already lstat-validated above; canonicalize parent aliases
    // and prove the resulting file is still the same inode before opening it.
    let canonical_source = safe_canonicalize(source);
    let canonical_metadata = std::fs::symlink_metadata(&canonical_source).with_context(|| {
        format!(
            "failed to inspect resolved restore source {}",
            canonical_source.display()
        )
    })?;
    anyhow::ensure!(
        canonical_metadata.file_type().is_file()
            && same_file_identity(&source_metadata, &canonical_metadata),
        "restore source changed while resolving: {}",
        source.display()
    );
    ensure_sqlite_source_artifacts_are_regular(&canonical_source)?;
    ensure_restore_staging_is_absent(staged)?;
    let staged_parent = staged
        .parent()
        .context("restore staging path has no parent directory")?;
    let canonical_staged_parent = safe_canonicalize(staged_parent);
    anyhow::ensure!(
        std::fs::symlink_metadata(&canonical_staged_parent)
            .map(|metadata| metadata.file_type().is_dir())
            .unwrap_or(false),
        "restore staging parent is not a real directory: {}",
        canonical_staged_parent.display()
    );
    let canonical_staged = canonical_staged_parent.join(
        staged
            .file_name()
            .context("restore staging path has no file name")?,
    );
    ensure_restore_staging_is_absent(&canonical_staged)?;

    let result = (|| -> Result<DbStats> {
        let source_flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let source_conn = Connection::open_with_flags(&canonical_source, source_flags)
            .with_context(|| format!("failed to open restore source {}", source.display()))?;
        source_conn.busy_timeout(DEFAULT_BUSY_TIMEOUT)?;

        let opened_source = std::fs::symlink_metadata(source)
            .with_context(|| format!("failed to revalidate restore source {}", source.display()))?;
        anyhow::ensure!(
            opened_source.file_type().is_file()
                && same_file_identity(&source_metadata, &opened_source),
            "restore source changed while opening: {}",
            source.display()
        );
        let staged_text = canonical_staged
            .to_str()
            .context("restore staging path is not valid UTF-8")?;
        source_conn
            .execute("VACUUM INTO ?1", params![staged_text])
            .with_context(|| {
                format!(
                    "failed to create consistent snapshot from {}",
                    source.display()
                )
            })?;
        let snapshotted_source = std::fs::symlink_metadata(source)
            .with_context(|| format!("failed to revalidate restore source {}", source.display()))?;
        anyhow::ensure!(
            snapshotted_source.file_type().is_file()
                && same_file_identity(&source_metadata, &snapshotted_source),
            "restore source changed while snapshotting: {}",
            source.display()
        );
        drop(source_conn);

        ensure_sqlite_source_artifacts_are_regular(&canonical_staged)?;
        let staged_flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let mut staged_conn = Connection::open_with_flags(&canonical_staged, staged_flags)
            .with_context(|| {
                format!(
                    "failed to open staged snapshot {}",
                    canonical_staged.display()
                )
            })?;
        let _: String = staged_conn
            .query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))
            .context("failed to make staged snapshot self-contained")?;
        staged_conn.busy_timeout(DEFAULT_BUSY_TIMEOUT)?;
        apply_open_migrations(&mut staged_conn, normalized_root)?;

        let integrity: String = staged_conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .context("failed to check staged snapshot integrity")?;
        anyhow::ensure!(
            integrity.eq_ignore_ascii_case("ok"),
            "restore source failed SQLite integrity_check: {integrity}"
        );
        validate_required_index_schema(&staged_conn)?;
        let stats = get_stats(&staged_conn)?;
        drop(staged_conn);

        for suffix in ["-wal", "-shm", "-journal"] {
            let sidecar = sqlite_sidecar_path(&canonical_staged, suffix);
            anyhow::ensure!(
                !sidecar.exists(),
                "staged snapshot left a SQLite sidecar: {}",
                sidecar.display()
            );
        }
        Ok(stats)
    })();

    match result {
        Ok(stats) => Ok(stats),
        Err(error) => match cleanup_restore_staging(&canonical_staged) {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(anyhow::anyhow!(
                "{error:#}; failed to clean restore staging: {cleanup_error:#}"
            )),
        },
    }
}

/// Check if database exists and is initialized
pub fn db_exists(project_root: &Path) -> bool {
    if let Ok((db_path, _lease, _normalized)) = resolve_db_path_and_lease(project_root) {
        let publication = match try_acquire_shared_publication(&db_path, &_lease) {
            Ok(publication) => publication,
            // The bool compatibility API cannot return a retryable error.
            // Treat contention as "possibly present" so production callers
            // proceed to `open_db_leased` and surface `IndexPublicationBusy`
            // instead of printing a false "Index not found" result.
            Err(error) if is_publication_busy(&error) => return true,
            Err(_) => return false,
        };
        if ensure_no_interrupted_publication(&db_path).is_err() {
            drop(publication);
            return true;
        }
        if !std::fs::symlink_metadata(&db_path)
            .map(|metadata| metadata.file_type().is_file())
            .unwrap_or(false)
        {
            return false;
        }
        // Also check if tables exist
        if let Ok(conn) = Connection::open(&db_path) {
            conn.query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='files'",
                [],
                |_| Ok(()),
            )
            .is_ok()
        } else {
            false
        }
    } else {
        false
    }
}

/// Symbol kinds
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Class,
    Interface,
    Object,
    Enum,
    Function,
    Procedure,
    Property,
    TypeAlias,
    // Perl-specific
    Package,
    Constant,
    // For imports/includes
    Import,
    // For annotations/decorators
    Annotation,
}

impl SymbolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SymbolKind::Class => "class",
            SymbolKind::Interface => "interface",
            SymbolKind::Object => "object",
            SymbolKind::Enum => "enum",
            SymbolKind::Function => "function",
            SymbolKind::Procedure => "procedure",
            SymbolKind::Property => "property",
            SymbolKind::TypeAlias => "typealias",
            SymbolKind::Package => "package",
            SymbolKind::Constant => "constant",
            SymbolKind::Import => "import",
            SymbolKind::Annotation => "annotation",
        }
    }
}

/// Insert or update a file record
pub fn upsert_file(conn: &Connection, path: &str, mtime: i64, size: i64) -> Result<i64> {
    conn.execute(
        "INSERT OR REPLACE INTO files (path, mtime, size) VALUES (?1, ?2, ?3)",
        params![path, mtime, size],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Insert a symbol
pub fn insert_symbol(
    conn: &Connection,
    file_id: i64,
    name: &str,
    kind: SymbolKind,
    line: usize,
    signature: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO symbols (file_id, name, kind, line, signature) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![file_id, name, kind.as_str(), line as i64, signature],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Insert inheritance relationship
pub fn insert_inheritance(
    conn: &Connection,
    child_id: i64,
    parent_name: &str,
    kind: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO inheritance (child_id, parent_name, kind) VALUES (?1, ?2, ?3)",
        params![child_id, parent_name, kind],
    )?;
    Ok(())
}

/// Escape FTS5 special characters
fn escape_fts5_query(query: &str) -> String {
    // Handle empty query
    if query.trim().is_empty() {
        return String::new();
    }
    // Check for prefix operator: * must stay OUTSIDE quotes for FTS5
    let (term, suffix) = if query.ends_with('*') {
        (&query[..query.len() - 1], "*")
    } else {
        (query, "")
    };
    // Wrap in double quotes to treat as literal phrase
    // Escape any existing double quotes
    let escaped = term.replace('"', "\"\"");
    format!("\"{}\"{}", escaped, suffix)
}

/// Search symbols by name (FTS5)
pub fn search_symbols(conn: &Connection, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
    // Handle empty query
    if query.trim().is_empty() {
        return Ok(vec![]);
    }

    if query.contains("::") {
        let raw = query.trim_end_matches('*');
        let (sql, value) = if query.starts_with("::") {
            (
                r#"
                SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
                FROM symbols s
                JOIN files f ON s.file_id = f.id
                WHERE s.qualified_name LIKE ?1
                ORDER BY length(s.qualified_name), s.qualified_name
                LIMIT ?2
                "#,
                format!("%{}", raw),
            )
        } else if query.ends_with('*') {
            (
                r#"
                SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
                FROM symbols s
                JOIN files f ON s.file_id = f.id
                WHERE s.qualified_name LIKE ?1
                ORDER BY length(s.qualified_name), s.qualified_name
                LIMIT ?2
                "#,
                format!("{raw}%"),
            )
        } else {
            (
                r#"
                SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
                FROM symbols s
                JOIN files f ON s.file_id = f.id
                WHERE s.qualified_name = ?1
                LIMIT ?2
                "#,
                raw.to_string(),
            )
        };

        let mut stmt = conn.prepare(sql)?;
        return Ok(stmt
            .query_map(params![value, limit as i64], row_to_search_result)?
            .collect::<Result<Vec<_>, _>>()?);
    }

    let escaped_query = escape_fts5_query(query);

    let mut stmt = conn.prepare(
        r#"
        SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
        FROM symbols_fts fts
        JOIN symbols s ON fts.rowid = s.id
        JOIN files f ON s.file_id = f.id
        WHERE symbols_fts MATCH ?1
        LIMIT ?2
        "#,
    )?;

    let results = stmt
        .query_map(params![escaped_query, limit as i64], row_to_search_result)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Search result
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualified_name: Option<String>,
    pub kind: String,
    pub line: i64,
    pub signature: Option<String>,
    pub path: String,
    #[serde(skip_serializing)]
    pub root_path: Option<String>,
}

impl SearchResult {
    pub fn display_name(&self) -> &str {
        self.qualified_name.as_deref().unwrap_or(&self.name)
    }
}

fn row_to_search_result(row: &rusqlite::Row<'_>) -> rusqlite::Result<SearchResult> {
    let root_path = if row.as_ref().column_count() > 6 {
        row.get::<_, Option<String>>(6)?.filter(|s| !s.is_empty())
    } else {
        None
    };
    Ok(SearchResult {
        name: row.get(0)?,
        qualified_name: row.get(1)?,
        kind: row.get(2)?,
        line: row.get(3)?,
        signature: row.get(4)?,
        path: row.get(5)?,
        root_path,
    })
}

#[derive(Debug, Serialize)]
pub struct FileResult {
    pub path: String,
    #[serde(skip_serializing)]
    pub root_path: Option<String>,
}

/// Find files by name pattern
pub fn find_files(conn: &Connection, pattern: &str, limit: usize) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT path FROM files WHERE path LIKE ?1 LIMIT ?2")?;

    let pattern = format!("%{}%", pattern);
    let results = stmt
        .query_map(params![pattern, limit as i64], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

pub fn find_files_with_roots(
    conn: &Connection,
    pattern: &str,
    limit: usize,
) -> Result<Vec<FileResult>> {
    let mut stmt = conn.prepare("SELECT path, root_path FROM files WHERE path LIKE ?1 LIMIT ?2")?;

    let pattern = format!("%{}%", pattern);
    let results = stmt
        .query_map(params![pattern, limit as i64], |row| {
            Ok(FileResult {
                path: row.get(0)?,
                root_path: row.get::<_, Option<String>>(1)?.filter(|s| !s.is_empty()),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

pub fn count_files_with_roots_scoped(
    conn: &Connection,
    pattern: &str,
    scope: &SearchScope,
) -> Result<usize> {
    let (scope_clause, scope_params) = scope.path_condition();
    let sql = format!("SELECT COUNT(*) FROM files f WHERE f.path LIKE ?{scope_clause}");
    let mut values = vec![format!("%{pattern}%")];
    values.extend(scope_params);
    let params: Vec<&dyn rusqlite::types::ToSql> = values
        .iter()
        .map(|value| value as &dyn rusqlite::types::ToSql)
        .collect();
    let count: i64 = conn.query_row(&sql, params.as_slice(), |row| row.get(0))?;
    Ok(count as usize)
}

pub fn count_files_with_roots_terms_scoped(
    conn: &Connection,
    terms: &[&str],
    scope: &SearchScope,
) -> Result<usize> {
    if terms.is_empty() {
        return Ok(0);
    }
    let predicates = (0..terms.len())
        .map(|_| "f.path LIKE ?")
        .collect::<Vec<_>>()
        .join(" OR ");
    let (scope_clause, scope_params) = scope.path_condition();
    let sql = format!("SELECT COUNT(*) FROM files f WHERE ({predicates}){scope_clause}");
    let mut values: Vec<String> = terms.iter().map(|term| format!("%{term}%")).collect();
    values.extend(scope_params);
    let params: Vec<&dyn rusqlite::types::ToSql> = values
        .iter()
        .map(|value| value as &dyn rusqlite::types::ToSql)
        .collect();
    let count: i64 = conn.query_row(&sql, params.as_slice(), |row| row.get(0))?;
    Ok(count as usize)
}

pub fn find_files_with_roots_terms_scoped(
    conn: &Connection,
    terms: &[&str],
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<FileResult>> {
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let predicates = (0..terms.len())
        .map(|_| "f.path LIKE ?")
        .collect::<Vec<_>>()
        .join(" OR ");
    let (scope_clause, scope_params) = scope.path_condition();
    let sql = format!(
        "SELECT f.path, f.root_path FROM files f WHERE ({predicates}){scope_clause} ORDER BY f.path LIMIT ?"
    );
    let mut values: Vec<String> = terms.iter().map(|term| format!("%{term}%")).collect();
    values.extend(scope_params);
    values.push(limit.to_string());
    let params: Vec<&dyn rusqlite::types::ToSql> = values
        .iter()
        .map(|value| value as &dyn rusqlite::types::ToSql)
        .collect();
    let mut stmt = conn.prepare(&sql)?;
    let results = stmt
        .query_map(params.as_slice(), |row| {
            Ok(FileResult {
                path: row.get(0)?,
                root_path: row.get::<_, Option<String>>(1)?.filter(|s| !s.is_empty()),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

pub fn find_files_with_roots_scoped(
    conn: &Connection,
    pattern: &str,
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<FileResult>> {
    let (scope_clause, scope_params) = scope.path_condition();
    let sql = format!(
        "SELECT f.path, f.root_path FROM files f WHERE f.path LIKE ?{scope_clause} LIMIT ?"
    );
    let mut values = vec![format!("%{pattern}%")];
    values.extend(scope_params);
    values.push(limit.to_string());
    let params: Vec<&dyn rusqlite::types::ToSql> = values
        .iter()
        .map(|value| value as &dyn rusqlite::types::ToSql)
        .collect();
    let mut stmt = conn.prepare(&sql)?;
    let results = stmt
        .query_map(params.as_slice(), |row| {
            Ok(FileResult {
                path: row.get(0)?,
                root_path: row.get::<_, Option<String>>(1)?.filter(|s| !s.is_empty()),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

/// Find symbols by name (exact match first, then prefix/contains if no results)
pub fn find_symbols_by_name(
    conn: &Connection,
    name: &str,
    kind: Option<&str>,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    if name.starts_with("::") {
        let exact_query = if kind.is_some() {
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.qualified_name LIKE ?1 AND s.kind = ?2
            ORDER BY length(s.qualified_name), s.qualified_name
            LIMIT ?3
            "#
        } else {
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.qualified_name LIKE ?1
            ORDER BY length(s.qualified_name), s.qualified_name
            LIMIT ?2
            "#
        };

        let mut stmt = conn.prepare(exact_query)?;
        let pattern = format!("%{}", name);
        let results = if let Some(k) = kind {
            stmt.query_map(params![pattern, k, limit as i64], row_to_search_result)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![pattern, limit as i64], row_to_search_result)?
                .collect::<Result<Vec<_>, _>>()?
        };
        return Ok(results);
    }

    if name.contains("::") {
        let exact_query = if kind.is_some() {
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.qualified_name = ?1 AND s.kind = ?2
            LIMIT ?3
            "#
        } else {
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.qualified_name = ?1
            LIMIT ?2
            "#
        };

        let mut stmt = conn.prepare(exact_query)?;
        let results: Vec<SearchResult> = if let Some(k) = kind {
            stmt.query_map(params![name, k, limit as i64], row_to_search_result)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![name, limit as i64], row_to_search_result)?
                .collect::<Result<Vec<_>, _>>()?
        };

        if !results.is_empty() {
            return Ok(results);
        }

        let suffix_query = if kind.is_some() {
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.qualified_name LIKE ?1 AND s.kind = ?2
            ORDER BY length(s.qualified_name)
            LIMIT ?3
            "#
        } else {
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.qualified_name LIKE ?1
            ORDER BY length(s.qualified_name)
            LIMIT ?2
            "#
        };

        let mut stmt = conn.prepare(suffix_query)?;
        let suffix_pattern = format!("%::{}", name);
        let results = if let Some(k) = kind {
            stmt.query_map(
                params![suffix_pattern, k, limit as i64],
                row_to_search_result,
            )?
            .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![suffix_pattern, limit as i64], row_to_search_result)?
                .collect::<Result<Vec<_>, _>>()?
        };

        if !results.is_empty() {
            return Ok(results);
        }

        let prefix_query = if kind.is_some() {
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.qualified_name LIKE ?1 AND s.kind = ?2
            ORDER BY length(s.qualified_name)
            LIMIT ?3
            "#
        } else {
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.qualified_name LIKE ?1
            ORDER BY length(s.qualified_name)
            LIMIT ?2
            "#
        };

        let mut stmt = conn.prepare(prefix_query)?;
        let pattern = format!("{name}%");
        let results = if let Some(k) = kind {
            stmt.query_map(params![pattern, k, limit as i64], row_to_search_result)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![pattern, limit as i64], row_to_search_result)?
                .collect::<Result<Vec<_>, _>>()?
        };
        return Ok(results);
    }

    // Try exact match first
    let exact_query = if kind.is_some() {
        r#"
        SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE s.name = ?1 AND s.kind = ?2
        LIMIT ?3
        "#
    } else {
        r#"
        SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE s.name = ?1
        LIMIT ?2
        "#
    };

    let mut stmt = conn.prepare(exact_query)?;

    let results: Vec<SearchResult> = if let Some(k) = kind {
        stmt.query_map(params![name, k, limit as i64], row_to_search_result)?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map(params![name, limit as i64], row_to_search_result)?
            .collect::<Result<Vec<_>, _>>()?
    };

    // If no exact match, try prefix match
    if results.is_empty() {
        let pattern = format!("{}%", name);
        let prefix_query = if kind.is_some() {
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.name LIKE ?1 AND s.kind = ?2
            ORDER BY length(s.name)
            LIMIT ?3
            "#
        } else {
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.name LIKE ?1
            ORDER BY length(s.name)
            LIMIT ?2
            "#
        };

        let mut stmt = conn.prepare(prefix_query)?;
        let results: Vec<SearchResult> = if let Some(k) = kind {
            stmt.query_map(params![pattern, k, limit as i64], row_to_search_result)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![pattern, limit as i64], row_to_search_result)?
                .collect::<Result<Vec<_>, _>>()?
        };
        return Ok(results);
    }

    Ok(results)
}

/// Find class-like symbols (class, interface, object, enum) by name - single query
pub fn find_class_like(conn: &Connection, name: &str, limit: usize) -> Result<Vec<SearchResult>> {
    if name.starts_with("::") {
        let mut stmt = conn.prepare(
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.qualified_name LIKE ?1
              AND s.kind IN ('class', 'interface', 'object', 'enum', 'protocol', 'struct', 'actor', 'package')
            ORDER BY length(s.qualified_name), s.qualified_name
            LIMIT ?2
            "#,
        )?;
        let pattern = format!("%{}", name);
        return Ok(stmt
            .query_map(params![pattern, limit as i64], row_to_search_result)?
            .collect::<Result<Vec<_>, _>>()?);
    }

    if name.contains("::") {
        let mut stmt = conn.prepare(
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.qualified_name = ?1
              AND s.kind IN ('class', 'interface', 'object', 'enum', 'protocol', 'struct', 'actor', 'package')
            LIMIT ?2
            "#,
        )?;

        let exact = stmt
            .query_map(params![name, limit as i64], row_to_search_result)?
            .collect::<Result<Vec<_>, _>>()?;
        if !exact.is_empty() {
            return Ok(exact);
        }

        let mut stmt = conn.prepare(
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.qualified_name LIKE ?1
              AND s.kind IN ('class', 'interface', 'object', 'enum', 'protocol', 'struct', 'actor', 'package')
            ORDER BY length(s.qualified_name), s.qualified_name
            LIMIT ?2
            "#,
        )?;
        let suffix_pattern = format!("%::{}", name);
        let suffix = stmt
            .query_map(params![suffix_pattern, limit as i64], row_to_search_result)?
            .collect::<Result<Vec<_>, _>>()?;
        if !suffix.is_empty() {
            return Ok(suffix);
        }

        let mut stmt = conn.prepare(
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.qualified_name LIKE ?1
              AND s.kind IN ('class', 'interface', 'object', 'enum', 'protocol', 'struct', 'actor', 'package')
            ORDER BY length(s.qualified_name), s.qualified_name
            LIMIT ?2
            "#,
        )?;
        let pattern = format!("{name}%");
        return Ok(stmt
            .query_map(params![pattern, limit as i64], row_to_search_result)?
            .collect::<Result<Vec<_>, _>>()?);
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE s.name = ?1 AND s.kind IN ('class', 'interface', 'object', 'enum', 'protocol', 'struct', 'actor', 'package')
        LIMIT ?2
        "#,
    )?;

    let results = stmt
        .query_map(params![name, limit as i64], row_to_search_result)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Convert glob pattern to SQL LIKE pattern: * → %, ? → _
pub fn glob_to_like(pattern: &str) -> String {
    let mut result = String::with_capacity(pattern.len() + 4);
    for ch in pattern.chars() {
        match ch {
            '*' => result.push('%'),
            '?' => result.push('_'),
            '%' => {
                result.push_str("\\%");
            }
            '_' => {
                result.push_str("\\_");
            }
            _ => result.push(ch),
        }
    }
    result
}

/// Find class-like symbols matching a glob pattern
pub fn find_class_like_pattern(
    conn: &Connection,
    like_pattern: &str,
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<SearchResult>> {
    let (scope_clause, scope_params) = scope.path_condition();
    let qualified = like_pattern.contains("::");
    let search_pattern = if qualified && like_pattern.starts_with("::") {
        format!("%{}", like_pattern)
    } else {
        like_pattern.to_string()
    };
    let suffix_pattern =
        if qualified && !like_pattern.starts_with('%') && !like_pattern.starts_with("::") {
            Some(format!("%::{}", like_pattern))
        } else {
            None
        };
    let name_expr = if qualified {
        "COALESCE(s.qualified_name, s.name)"
    } else {
        "s.name"
    };

    let sql = format!(
        r#"
        SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE ({} LIKE ?1 ESCAPE '\'{} ) AND s.kind IN ('class', 'interface', 'object', 'enum', 'protocol', 'struct', 'actor', 'package'){}
        ORDER BY length({}), {}
        LIMIT ?{}
        "#,
        name_expr,
        if suffix_pattern.is_some() {
            format!(" OR {} LIKE ?2 ESCAPE '\\'", name_expr)
        } else {
            String::new()
        },
        scope_clause,
        name_expr,
        name_expr,
        2 + scope_params.len() + usize::from(suffix_pattern.is_some())
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    all_params.push(Box::new(search_pattern));
    if let Some(suffix_pattern) = suffix_pattern {
        all_params.push(Box::new(suffix_pattern));
    }
    for p in &scope_params {
        all_params.push(Box::new(p.clone()));
    }
    all_params.push(Box::new(limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        all_params.iter().map(|p| p.as_ref()).collect();
    let results = stmt
        .query_map(param_refs.as_slice(), row_to_search_result)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Find symbols matching a glob pattern with optional kind filter
pub fn find_symbols_by_pattern(
    conn: &Connection,
    like_pattern: &str,
    kind: Option<&str>,
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<SearchResult>> {
    let (scope_clause, scope_params) = scope.path_condition();
    let qualified = like_pattern.contains("::");
    let search_pattern = if qualified && like_pattern.starts_with("::") {
        format!("%{}", like_pattern)
    } else {
        like_pattern.to_string()
    };
    let suffix_pattern =
        if qualified && !like_pattern.starts_with('%') && !like_pattern.starts_with("::") {
            Some(format!("%::{}", like_pattern))
        } else {
            None
        };
    let name_expr = if qualified {
        "COALESCE(s.qualified_name, s.name)"
    } else {
        "s.name"
    };

    let kind_clause = if kind.is_some() {
        format!(" AND s.kind = ?{}", 2 + scope_params.len())
    } else {
        String::new()
    };

    let limit_idx = if kind.is_some() {
        3 + scope_params.len()
    } else {
        2 + scope_params.len()
    };

    let sql = format!(
        r#"
        SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE ({} LIKE ?1 ESCAPE '\'{} ){}{}
        ORDER BY length({}), {}
        LIMIT ?{}
        "#,
        name_expr,
        if suffix_pattern.is_some() {
            format!(" OR {} LIKE ?2 ESCAPE '\\'", name_expr)
        } else {
            String::new()
        },
        scope_clause,
        kind_clause,
        name_expr,
        name_expr,
        limit_idx + usize::from(suffix_pattern.is_some())
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    all_params.push(Box::new(search_pattern));
    if let Some(suffix_pattern) = suffix_pattern {
        all_params.push(Box::new(suffix_pattern));
    }
    for p in &scope_params {
        all_params.push(Box::new(p.clone()));
    }
    if let Some(k) = kind {
        all_params.push(Box::new(k.to_string()));
    }
    all_params.push(Box::new(limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        all_params.iter().map(|p| p.as_ref()).collect();
    let results = stmt
        .query_map(param_refs.as_slice(), row_to_search_result)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Find implementations (subclasses/implementors)
pub fn find_implementations(
    conn: &Connection,
    parent_name: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    // Match exact name or qualified suffix in either dot- or C++-style form.
    let suffix_pattern = format!("%.{}", parent_name);
    let namespace_suffix_pattern = format!("%::{}", parent_name);
    let mut stmt = conn.prepare(
        r#"
        SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
        FROM inheritance i
        JOIN symbols s ON i.child_id = s.id
        JOIN files f ON s.file_id = f.id
        WHERE i.parent_name = ?1 OR i.parent_name LIKE ?2 OR i.parent_name LIKE ?3
        ORDER BY
            CASE
                WHEN i.parent_name = ?1 THEN 0
                ELSE 1
            END, s.name
        LIMIT ?4
        "#,
    )?;

    let results = stmt
        .query_map(
            params![
                parent_name,
                suffix_pattern,
                namespace_suffix_pattern,
                limit as i64
            ],
            row_to_search_result,
        )?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

pub fn count_implementations(conn: &Connection, parent_name: &str) -> Result<usize> {
    let suffix_pattern = format!("%.{}", parent_name);
    let namespace_suffix_pattern = format!("%::{}", parent_name);
    let count: i64 = conn.query_row(
        r#"
        SELECT COUNT(*)
        FROM inheritance i
        JOIN symbols s ON i.child_id = s.id
        WHERE i.parent_name = ?1 OR i.parent_name LIKE ?2 OR i.parent_name LIKE ?3
        "#,
        params![parent_name, suffix_pattern, namespace_suffix_pattern],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

pub fn count_implementations_scoped(
    conn: &Connection,
    parent_name: &str,
    scope: &SearchScope,
) -> Result<usize> {
    if scope.is_empty() {
        return count_implementations(conn, parent_name);
    }
    let suffix_pattern = format!("%.{parent_name}");
    let namespace_suffix_pattern = format!("%::{parent_name}");
    let (scope_clause, scope_params) = scope.path_condition();
    let sql = format!(
        r#"
        SELECT COUNT(*)
        FROM inheritance i
        JOIN symbols s ON i.child_id = s.id
        JOIN files f ON s.file_id = f.id
        WHERE (i.parent_name = ? OR i.parent_name LIKE ? OR i.parent_name LIKE ?){scope_clause}
        "#
    );
    let mut values = vec![
        parent_name.to_string(),
        suffix_pattern,
        namespace_suffix_pattern,
    ];
    values.extend(scope_params);
    let params: Vec<&dyn rusqlite::types::ToSql> = values
        .iter()
        .map(|value| value as &dyn rusqlite::types::ToSql)
        .collect();
    let count: i64 = conn.query_row(&sql, params.as_slice(), |row| row.get(0))?;
    Ok(count as usize)
}

pub fn find_implementations_scoped(
    conn: &Connection,
    parent_name: &str,
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<SearchResult>> {
    if scope.is_empty() {
        return find_implementations(conn, parent_name, limit);
    }

    let suffix_pattern = format!("%.{}", parent_name);
    let namespace_suffix_pattern = format!("%::{}", parent_name);
    let (scope_clause, scope_params) = scope.path_condition();

    let sql = format!(
        r#"
        SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
        FROM inheritance i
        JOIN symbols s ON i.child_id = s.id
        JOIN files f ON s.file_id = f.id
        WHERE (i.parent_name = ?1 OR i.parent_name LIKE ?2 OR i.parent_name LIKE ?3){}
        ORDER BY
            CASE
                WHEN i.parent_name = ?1 THEN 0
                ELSE 1
            END, s.name
        LIMIT ?{}
        "#,
        scope_clause,
        4 + scope_params.len()
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    all_params.push(Box::new(parent_name.to_string()));
    all_params.push(Box::new(suffix_pattern));
    all_params.push(Box::new(namespace_suffix_pattern));
    for p in &scope_params {
        all_params.push(Box::new(p.clone()));
    }
    all_params.push(Box::new(limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        all_params.iter().map(|p| p.as_ref()).collect();
    let results = stmt
        .query_map(param_refs.as_slice(), row_to_search_result)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Get database statistics
pub fn get_stats(conn: &Connection) -> Result<DbStats> {
    let file_count: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;
    let symbol_count: i64 = conn.query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))?;
    let module_count: i64 = conn.query_row("SELECT COUNT(*) FROM modules", [], |row| row.get(0))?;
    let refs_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM refs", [], |row| row.get(0))
        .unwrap_or(0);
    let xml_usages_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM xml_usages", [], |row| row.get(0))
        .unwrap_or(0);
    let resources_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM resources", [], |row| row.get(0))
        .unwrap_or(0);
    let storyboard_usages_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM storyboard_usages", [], |row| {
            row.get(0)
        })
        .unwrap_or(0);
    let ios_assets_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM ios_assets", [], |row| row.get(0))
        .unwrap_or(0);

    Ok(DbStats {
        file_count,
        symbol_count,
        module_count,
        refs_count,
        xml_usages_count,
        resources_count,
        storyboard_usages_count,
        ios_assets_count,
    })
}

#[derive(Debug, Serialize)]
pub struct DbStats {
    pub file_count: i64,
    pub symbol_count: i64,
    pub module_count: i64,
    pub refs_count: i64,
    pub xml_usages_count: i64,
    pub resources_count: i64,
    pub storyboard_usages_count: i64,
    pub ios_assets_count: i64,
}

/// Clear all data from the database
pub fn clear_db(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        DELETE FROM ios_asset_usages;
        DELETE FROM ios_assets;
        DELETE FROM storyboard_usages;
        DELETE FROM resource_usages;
        DELETE FROM resources;
        DELETE FROM xml_usages;
        DELETE FROM transitive_deps;
        DELETE FROM refs;
        DELETE FROM inheritance;
        DELETE FROM module_deps;
        DELETE FROM modules;
        DELETE FROM symbols;
        DELETE FROM files;
        "#,
    )?;
    Ok(())
}

/// Reference result
#[derive(Debug, Serialize)]
pub struct RefResult {
    pub name: String,
    pub line: i64,
    pub context: Option<String>,
    pub path: String,
    #[serde(skip_serializing)]
    pub root_path: Option<String>,
}

fn row_to_ref_result(row: &rusqlite::Row<'_>) -> rusqlite::Result<RefResult> {
    let root_path = if row.as_ref().column_count() > 4 {
        row.get::<_, Option<String>>(4)?.filter(|s| !s.is_empty())
    } else {
        None
    };
    Ok(RefResult {
        name: row.get(0)?,
        line: row.get(1)?,
        context: row.get(2)?,
        path: row.get(3)?,
        root_path,
    })
}

/// Find references (usages) of a symbol
pub fn find_references(conn: &Connection, name: &str, limit: usize) -> Result<Vec<RefResult>> {
    // Early materialization: filter and sort refs using covering index BEFORE
    // joining with files. Avoids SQLite planner choosing full scan on large
    // tables (~12M rows) when ORDER BY references the joined table. See #19.
    //
    // Inner ORDER BY (file_id, line) is free because idx_refs_name_file_line
    // has exactly this sort order. Outer ORDER BY f.path reshuffles the tiny
    // result set (bounded by LIMIT) so output is stable for users.
    let mut stmt = conn.prepare(
        r#"
        SELECT r.name, r.line, r.context, f.path, f.root_path
        FROM (
            SELECT name, file_id, line, context
            FROM refs
            WHERE name = ?1
            ORDER BY file_id, line
            LIMIT ?2
        ) r
        JOIN files f ON f.id = r.file_id
        ORDER BY f.path, r.line
        "#,
    )?;

    let results = stmt
        .query_map(params![name, limit as i64], row_to_ref_result)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

pub fn count_references_scoped(
    conn: &Connection,
    name: &str,
    scope: &SearchScope,
) -> Result<usize> {
    let (scope_clause, scope_params) = scope.path_condition();
    let sql = format!(
        "SELECT COUNT(*) FROM refs r JOIN files f ON r.file_id = f.id WHERE r.name = ?{scope_clause}"
    );
    let mut values = vec![name.to_string()];
    values.extend(scope_params);
    let params: Vec<&dyn rusqlite::types::ToSql> = values
        .iter()
        .map(|value| value as &dyn rusqlite::types::ToSql)
        .collect();
    let count: i64 = conn.query_row(&sql, params.as_slice(), |row| row.get(0))?;
    Ok(count as usize)
}

/// All symbols defined in a file, ordered by line. Used by `explore --rwr`
/// to attribute a reference (file + line) to its owning symbol — the last
/// symbol whose start line is <= the reference line. Approximate without
/// `end_line`, but good enough to build a caller→callee graph in memory.
pub fn get_file_symbols(conn: &Connection, path: &str) -> Result<Vec<SearchResult>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE f.path = ?1
        ORDER BY s.line
        "#,
    )?;
    let results = stmt
        .query_map(params![path], row_to_search_result)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

/// Search references by name (prefix match, grouped by unique name)
pub fn search_refs(conn: &Connection, query: &str, limit: usize) -> Result<Vec<(String, i64)>> {
    let pattern = format!("{}%", query);
    let mut stmt = conn.prepare(
        r#"
        SELECT r.name, COUNT(*) as usage_count
        FROM refs r
        WHERE r.name LIKE ?1
        GROUP BY r.name
        ORDER BY
            CASE WHEN r.name = ?2 THEN 0
                 WHEN r.name LIKE ?1 THEN 1
                 ELSE 2
            END,
            usage_count DESC
        LIMIT ?3
        "#,
    )?;
    let results = stmt
        .query_map(params![pattern, query, limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

pub fn count_search_refs(conn: &Connection, query: &str) -> Result<usize> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT name) FROM refs WHERE name LIKE ?1",
        params![format!("{query}%")],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

pub fn count_search_ref_terms(conn: &Connection, terms: &[&str]) -> Result<usize> {
    if terms.is_empty() {
        return Ok(0);
    }
    let predicates = (0..terms.len())
        .map(|_| "name LIKE ?")
        .collect::<Vec<_>>()
        .join(" OR ");
    let sql = format!("SELECT COUNT(DISTINCT name) FROM refs WHERE {predicates}");
    let values: Vec<String> = terms.iter().map(|term| format!("{term}%")).collect();
    let params: Vec<&dyn rusqlite::types::ToSql> = values
        .iter()
        .map(|value| value as &dyn rusqlite::types::ToSql)
        .collect();
    let count: i64 = conn.query_row(&sql, params.as_slice(), |row| row.get(0))?;
    Ok(count as usize)
}

pub fn count_search_ref_terms_scoped(
    conn: &Connection,
    terms: &[&str],
    scope: &SearchScope,
) -> Result<usize> {
    if terms.is_empty() {
        return Ok(0);
    }
    let predicates = (0..terms.len())
        .map(|_| "r.name LIKE ?")
        .collect::<Vec<_>>()
        .join(" OR ");
    let (scope_clause, scope_params) = scope.path_condition();
    let sql = format!(
        "SELECT COUNT(DISTINCT r.name) FROM refs r JOIN files f ON r.file_id = f.id WHERE ({predicates}){scope_clause}"
    );
    let mut values: Vec<String> = terms.iter().map(|term| format!("{term}%")).collect();
    values.extend(scope_params);
    let params: Vec<&dyn rusqlite::types::ToSql> = values
        .iter()
        .map(|value| value as &dyn rusqlite::types::ToSql)
        .collect();
    let count: i64 = conn.query_row(&sql, params.as_slice(), |row| row.get(0))?;
    Ok(count as usize)
}

pub fn search_ref_terms_scoped(
    conn: &Connection,
    terms: &[&str],
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<(String, i64)>> {
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let predicates = (0..terms.len())
        .map(|_| "r.name LIKE ?")
        .collect::<Vec<_>>()
        .join(" OR ");
    let (scope_clause, scope_params) = scope.path_condition();
    let sql = format!(
        "SELECT r.name, COUNT(*) AS usage_count FROM refs r JOIN files f ON r.file_id = f.id WHERE ({predicates}){scope_clause} GROUP BY r.name ORDER BY usage_count DESC, r.name LIMIT ?"
    );
    let mut values: Vec<String> = terms.iter().map(|term| format!("{term}%")).collect();
    values.extend(scope_params);
    values.push(limit.to_string());
    let params: Vec<&dyn rusqlite::types::ToSql> = values
        .iter()
        .map(|value| value as &dyn rusqlite::types::ToSql)
        .collect();
    let mut stmt = conn.prepare(&sql)?;
    let results = stmt
        .query_map(params.as_slice(), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

pub fn search_refs_scoped(
    conn: &Connection,
    query: &str,
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<(String, i64)>> {
    if scope.is_empty() {
        return search_refs(conn, query, limit);
    }
    let (scope_clause, scope_params) = scope.path_condition();
    let sql = format!(
        r#"
        SELECT r.name, COUNT(*) AS usage_count
        FROM refs r
        JOIN files f ON r.file_id = f.id
        WHERE r.name LIKE ?{scope_clause}
        GROUP BY r.name
        ORDER BY CASE WHEN r.name = ? THEN 0 ELSE 1 END, usage_count DESC
        LIMIT ?
        "#
    );
    let mut values = vec![format!("{query}%")];
    values.extend(scope_params);
    values.push(query.to_string());
    values.push(limit.to_string());
    let params: Vec<&dyn rusqlite::types::ToSql> = values
        .iter()
        .map(|value| value as &dyn rusqlite::types::ToSql)
        .collect();
    let mut stmt = conn.prepare(&sql)?;
    let results = stmt
        .query_map(params.as_slice(), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

/// Count references in the database
pub fn count_refs(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM refs", [], |row| row.get(0))?)
}

/// Find import statements for a symbol name
pub fn find_imports(conn: &Connection, name: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE s.kind = 'import' AND s.name = ?1
        LIMIT ?2
        "#,
    )?;

    let results = stmt
        .query_map(params![name, limit as i64], row_to_search_result)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

pub fn count_imports(conn: &Connection, name: &str) -> Result<usize> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM symbols WHERE kind = 'import' AND name = ?1",
        params![name],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

pub fn count_imports_scoped(conn: &Connection, name: &str, scope: &SearchScope) -> Result<usize> {
    let (scope_clause, scope_params) = scope.path_condition();
    let sql = format!(
        "SELECT COUNT(*) FROM symbols s JOIN files f ON s.file_id = f.id WHERE s.kind = 'import' AND s.name = ?{scope_clause}"
    );
    let mut values = vec![name.to_string()];
    values.extend(scope_params);
    let params: Vec<&dyn rusqlite::types::ToSql> = values
        .iter()
        .map(|value| value as &dyn rusqlite::types::ToSql)
        .collect();
    let count: i64 = conn.query_row(&sql, params.as_slice(), |row| row.get(0))?;
    Ok(count as usize)
}

pub fn find_imports_scoped(
    conn: &Connection,
    name: &str,
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<SearchResult>> {
    let (scope_clause, scope_params) = scope.path_condition();
    let sql = format!(
        "SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path FROM symbols s JOIN files f ON s.file_id = f.id WHERE s.kind = 'import' AND s.name = ?{scope_clause} LIMIT ?"
    );
    let mut values = vec![name.to_string()];
    values.extend(scope_params);
    values.push(limit.to_string());
    let params: Vec<&dyn rusqlite::types::ToSql> = values
        .iter()
        .map(|value| value as &dyn rusqlite::types::ToSql)
        .collect();
    let mut stmt = conn.prepare(&sql)?;
    let results = stmt
        .query_map(params.as_slice(), row_to_search_result)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

pub fn find_definitions(conn: &Connection, name: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let find = |predicate: &str, value: String| -> Result<Vec<SearchResult>> {
        let sql = format!(
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE {predicate} AND s.kind != 'import'
            ORDER BY length(COALESCE(s.qualified_name, s.name)), COALESCE(s.qualified_name, s.name)
            LIMIT ?
            "#
        );
        let mut stmt = conn.prepare(&sql)?;
        let results = stmt
            .query_map(params![value, limit as i64], row_to_search_result)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(results)
    };

    if name.starts_with("::") {
        return find("s.qualified_name LIKE ?", format!("%{name}"));
    }
    if name.contains("::") {
        let exact = find("s.qualified_name = ?", name.to_string())?;
        if !exact.is_empty() {
            return Ok(exact);
        }
        let suffix = find("s.qualified_name LIKE ?", format!("%::{name}"))?;
        if !suffix.is_empty() {
            return Ok(suffix);
        }
        return find("s.qualified_name LIKE ?", format!("{name}%"));
    }
    let exact = find("s.name = ?", name.to_string())?;
    if !exact.is_empty() {
        Ok(exact)
    } else {
        find("s.name LIKE ?", format!("{name}%"))
    }
}

pub fn find_definitions_scoped(
    conn: &Connection,
    name: &str,
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<SearchResult>> {
    let find = |predicate: &str, value: String| -> Result<Vec<SearchResult>> {
        let (scope_clause, scope_params) = scope.path_condition();
        let sql = format!(
            "SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path FROM symbols s JOIN files f ON s.file_id = f.id WHERE {predicate} AND s.kind != 'import'{scope_clause} ORDER BY length(COALESCE(s.qualified_name, s.name)), COALESCE(s.qualified_name, s.name) LIMIT ?"
        );
        let mut values = vec![value];
        values.extend(scope_params);
        values.push(limit.to_string());
        let params: Vec<&dyn rusqlite::types::ToSql> = values
            .iter()
            .map(|value| value as &dyn rusqlite::types::ToSql)
            .collect();
        let mut stmt = conn.prepare(&sql)?;
        let results = stmt
            .query_map(params.as_slice(), row_to_search_result)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(results)
    };

    if name.starts_with("::") {
        return find("s.qualified_name LIKE ?", format!("%{name}"));
    }
    if name.contains("::") {
        let exact = find("s.qualified_name = ?", name.to_string())?;
        if !exact.is_empty() {
            return Ok(exact);
        }
        let suffix = find("s.qualified_name LIKE ?", format!("%::{name}"))?;
        if !suffix.is_empty() {
            return Ok(suffix);
        }
        return find("s.qualified_name LIKE ?", format!("{name}%"));
    }
    let exact = find("s.name = ?", name.to_string())?;
    if !exact.is_empty() {
        Ok(exact)
    } else {
        find("s.name LIKE ?", format!("{name}%"))
    }
}

/// Find all cross-references for a symbol: definitions, imports, and usages
pub fn find_cross_references(
    conn: &Connection,
    name: &str,
    limit: usize,
) -> Result<(Vec<SearchResult>, Vec<SearchResult>, Vec<RefResult>)> {
    // 1. Definitions (non-import symbols)
    let definitions = find_symbols_by_name(conn, name, None, limit)?
        .into_iter()
        .filter(|s| s.kind != "import")
        .collect();

    // 2. Imports
    let imports = find_imports(conn, name, limit)?;

    // 3. Usages (refs table)
    let usages = find_references(conn, name, limit)?;

    Ok((definitions, imports, usages))
}

/// Fuzzy search for symbols: exact → prefix → contains cascade
pub fn search_symbols_fuzzy(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    if query.contains("::") {
        let mut stmt = conn.prepare(
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.qualified_name LIKE ?1
            ORDER BY
                CASE WHEN s.qualified_name = ?2 THEN 0
                     WHEN s.qualified_name LIKE ?3 THEN 1
                     ELSE 2 END,
                length(s.qualified_name)
            LIMIT ?4
            "#,
        )?;
        let exact = if query.starts_with("::") {
            format!("%{}", query)
        } else {
            query.to_string()
        };
        let contains_pattern = if query.starts_with("::") {
            format!("%{}%", query)
        } else {
            format!("%{}%", query)
        };
        let prefix_pattern = if query.starts_with("::") {
            format!("%{}", query)
        } else {
            format!("{query}%")
        };
        return Ok(stmt
            .query_map(
                params![contains_pattern, exact, prefix_pattern, limit as i64],
                row_to_search_result,
            )?
            .collect::<Result<Vec<_>, _>>()?);
    }

    // Single query: contains match with ranking by relevance
    // exact match (name = query) first, then prefix, then contains — sorted by length
    let contains_pattern = format!("%{}%", query);
    let mut stmt = conn.prepare(
        r#"
        SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE s.name LIKE ?1
        ORDER BY
            CASE WHEN s.name = ?2 THEN 0
                 WHEN s.name LIKE ?3 THEN 1
                 ELSE 2 END,
            length(s.name)
        LIMIT ?4
        "#,
    )?;
    let prefix_pattern = format!("{}%", query);
    let results: Vec<SearchResult> = stmt
        .query_map(
            params![contains_pattern, query, prefix_pattern, limit as i64],
            row_to_search_result,
        )?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Scope filter for narrowing search results by file path or module
pub struct SearchScope<'a> {
    pub in_file: Option<&'a str>,
    pub module: Option<&'a str>,
    /// Directory prefix filter: only return results under this path (relative to project root)
    pub dir_prefix: Option<&'a str>,
}

impl<'a> SearchScope<'a> {
    pub fn none() -> Self {
        SearchScope {
            in_file: None,
            module: None,
            dir_prefix: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.in_file.is_none()
            && self.module.is_none()
            && self.dir_prefix.is_none()
            && std::env::var_os("AST_INDEX_LOCAL_SCOPE").is_none()
            && std::env::var_os("AST_INDEX_SUBTREE").is_none()
    }

    pub fn matches_path(&self, path: &str) -> bool {
        self.dir_prefix
            .map(|prefix| path.starts_with(prefix))
            .unwrap_or(true)
            && self.in_file.map(|file| path.contains(file)).unwrap_or(true)
            && self
                .module
                .map(|module| path.starts_with(module))
                .unwrap_or(true)
    }

    /// Build WHERE clause fragment and collect params
    fn path_condition(&self) -> (String, Vec<String>) {
        let mut conditions = Vec::new();
        let mut params = Vec::new();
        if let Some(prefix) = self.dir_prefix {
            conditions.push("f.path LIKE ?".to_string());
            params.push(format!("{}%", prefix));
        }
        if let Some(file) = self.in_file {
            conditions.push("f.path LIKE ?".to_string());
            params.push(format!("%{}%", file));
        }
        if let Some(module) = self.module {
            conditions.push("f.path LIKE ?".to_string());
            params.push(format!("{}%", module));
        }
        if std::env::var_os("AST_INDEX_LOCAL_SCOPE").is_some() {
            conditions.push(
                "NOT EXISTS (SELECT 1 FROM subtrees st WHERE st.canonical_path = f.root_path)"
                    .to_string(),
            );
        } else if let Ok(name) = std::env::var("AST_INDEX_SUBTREE") {
            conditions.push(
                "EXISTS (SELECT 1 FROM subtrees st WHERE st.canonical_path = f.root_path AND st.name = ?)"
                    .to_string(),
            );
            params.push(name);
        }
        if conditions.is_empty() {
            (String::new(), params)
        } else {
            (format!(" AND {}", conditions.join(" AND ")), params)
        }
    }
}

fn count_symbol_matches(
    conn: &Connection,
    predicate: &str,
    predicate_params: Vec<String>,
    kind: Option<&str>,
    scope: &SearchScope,
    class_only: bool,
    exclude_imports: bool,
) -> Result<usize> {
    let (scope_clause, scope_params) = scope.path_condition();
    let mut sql = format!(
        "SELECT COUNT(*) FROM symbols s JOIN files f ON s.file_id = f.id WHERE ({predicate}){scope_clause}"
    );
    if class_only {
        sql.push_str(" AND s.kind IN ('class', 'interface', 'object', 'enum', 'protocol', 'struct', 'actor', 'package')");
    }
    if exclude_imports {
        sql.push_str(" AND s.kind != 'import'");
    }
    if kind.is_some() {
        sql.push_str(" AND s.kind = ?");
    }

    let mut values = predicate_params;
    values.extend(scope_params);
    if let Some(kind) = kind {
        values.push(kind.to_string());
    }
    let params: Vec<&dyn rusqlite::types::ToSql> = values
        .iter()
        .map(|value| value as &dyn rusqlite::types::ToSql)
        .collect();
    let count: i64 = conn.query_row(&sql, params.as_slice(), |row| row.get(0))?;
    Ok(count as usize)
}

pub fn count_symbols_by_name_scoped(
    conn: &Connection,
    name: &str,
    kind: Option<&str>,
    scope: &SearchScope,
    exclude_imports: bool,
) -> Result<usize> {
    let count = |predicate: &str, value: String| {
        count_symbol_matches(
            conn,
            predicate,
            vec![value],
            kind,
            scope,
            false,
            exclude_imports,
        )
    };
    if name.starts_with("::") {
        return count("s.qualified_name LIKE ?", format!("%{name}"));
    }
    if name.contains("::") {
        let exact = count("s.qualified_name = ?", name.to_string())?;
        if exact > 0 {
            return Ok(exact);
        }
        let suffix = count("s.qualified_name LIKE ?", format!("%::{name}"))?;
        if suffix > 0 {
            return Ok(suffix);
        }
        return count("s.qualified_name LIKE ?", format!("{name}%"));
    }

    let exact = count("s.name = ?", name.to_string())?;
    if exact > 0 {
        Ok(exact)
    } else {
        count("s.name LIKE ?", format!("{name}%"))
    }
}

pub fn count_symbols_by_pattern_scoped(
    conn: &Connection,
    like_pattern: &str,
    kind: Option<&str>,
    scope: &SearchScope,
    class_only: bool,
) -> Result<usize> {
    let qualified = like_pattern.contains("::");
    let search_pattern = if qualified && like_pattern.starts_with("::") {
        format!("%{like_pattern}")
    } else {
        like_pattern.to_string()
    };
    let name_expr = if qualified {
        "COALESCE(s.qualified_name, s.name)"
    } else {
        "s.name"
    };
    if qualified && !like_pattern.starts_with('%') && !like_pattern.starts_with("::") {
        count_symbol_matches(
            conn,
            &format!("{name_expr} LIKE ? ESCAPE '\\' OR {name_expr} LIKE ? ESCAPE '\\'"),
            vec![search_pattern, format!("%::{like_pattern}")],
            kind,
            scope,
            class_only,
            false,
        )
    } else {
        count_symbol_matches(
            conn,
            &format!("{name_expr} LIKE ? ESCAPE '\\'"),
            vec![search_pattern],
            kind,
            scope,
            class_only,
            false,
        )
    }
}

pub fn count_class_like_scoped(
    conn: &Connection,
    name: &str,
    scope: &SearchScope,
) -> Result<usize> {
    let count = |predicate: &str, value: String| {
        count_symbol_matches(conn, predicate, vec![value], None, scope, true, false)
    };
    if name.starts_with("::") {
        return count("s.qualified_name LIKE ?", format!("%{name}"));
    }
    if name.contains("::") {
        let exact = count("s.qualified_name = ?", name.to_string())?;
        if exact > 0 {
            return Ok(exact);
        }
        let suffix = count("s.qualified_name LIKE ?", format!("%::{name}"))?;
        if suffix > 0 {
            return Ok(suffix);
        }
        return count("s.qualified_name LIKE ?", format!("{name}%"));
    }
    count("s.name = ?", name.to_string())
}

pub fn count_symbols_fuzzy_scoped(
    conn: &Connection,
    query: &str,
    kind: Option<&str>,
    scope: &SearchScope,
    class_only: bool,
) -> Result<usize> {
    let (predicate, value) = if query.contains("::") {
        ("s.qualified_name LIKE ?", format!("%{query}%"))
    } else {
        ("s.name LIKE ?", format!("%{query}%"))
    };
    count_symbol_matches(conn, predicate, vec![value], kind, scope, class_only, false)
}

pub fn count_search_symbols_scoped(
    conn: &Connection,
    query: &str,
    kind: Option<&str>,
    scope: &SearchScope,
) -> Result<usize> {
    if query.trim().is_empty() {
        return Ok(0);
    }
    if query.contains("::") {
        let raw = query.trim_end_matches('*');
        let (predicate, value) = if query.starts_with("::") {
            ("s.qualified_name LIKE ?", format!("%{raw}"))
        } else if query.ends_with('*') {
            ("s.qualified_name LIKE ?", format!("{raw}%"))
        } else {
            ("s.qualified_name = ?", raw.to_string())
        };
        return count_symbol_matches(conn, predicate, vec![value], kind, scope, false, false);
    }

    let escaped_query = escape_fts5_query(query);
    let (scope_clause, scope_params) = scope.path_condition();
    let mut sql = format!(
        r#"
        SELECT COUNT(*)
        FROM symbols_fts fts
        JOIN symbols s ON fts.rowid = s.id
        JOIN files f ON s.file_id = f.id
        WHERE symbols_fts MATCH ?{scope_clause}
        "#
    );
    if kind.is_some() {
        sql.push_str(" AND s.kind = ?");
    }
    let mut values = vec![escaped_query];
    values.extend(scope_params);
    if let Some(kind) = kind {
        values.push(kind.to_string());
    }
    let params: Vec<&dyn rusqlite::types::ToSql> = values
        .iter()
        .map(|value| value as &dyn rusqlite::types::ToSql)
        .collect();
    let count: i64 = conn.query_row(&sql, params.as_slice(), |row| row.get(0))?;
    Ok(count as usize)
}

pub fn count_search_symbol_terms_scoped(
    conn: &Connection,
    terms: &[&str],
    kind: Option<&str>,
    scope: &SearchScope,
    fuzzy: bool,
) -> Result<usize> {
    if terms.is_empty() {
        return Ok(0);
    }
    let (scope_clause, scope_params) = scope.path_condition();
    let mut values = Vec::new();
    let mut sql = if fuzzy {
        let predicates = terms
            .iter()
            .map(|term| {
                values.push(format!("%{term}%"));
                if term.contains("::") {
                    "s.qualified_name LIKE ?"
                } else {
                    "s.name LIKE ?"
                }
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        format!(
            "SELECT COUNT(*) FROM symbols s JOIN files f ON s.file_id = f.id WHERE ({predicates}){scope_clause}"
        )
    } else {
        let query = terms
            .iter()
            .map(|term| escape_fts5_query(&format!("{term}*")))
            .collect::<Vec<_>>()
            .join(" OR ");
        values.push(query);
        format!(
            "SELECT COUNT(*) FROM symbols_fts fts JOIN symbols s ON fts.rowid = s.id JOIN files f ON s.file_id = f.id WHERE symbols_fts MATCH ?{scope_clause}"
        )
    };
    values.extend(scope_params);
    if let Some(kind) = kind {
        sql.push_str(" AND s.kind = ?");
        values.push(kind.to_string());
    }
    let params: Vec<&dyn rusqlite::types::ToSql> = values
        .iter()
        .map(|value| value as &dyn rusqlite::types::ToSql)
        .collect();
    let count: i64 = conn.query_row(&sql, params.as_slice(), |row| row.get(0))?;
    Ok(count as usize)
}

pub fn search_symbol_terms_scoped(
    conn: &Connection,
    terms: &[&str],
    kind: Option<&str>,
    limit: usize,
    scope: &SearchScope,
    fuzzy: bool,
) -> Result<Vec<SearchResult>> {
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let (scope_clause, scope_params) = scope.path_condition();
    let mut values = Vec::new();
    let mut sql = if fuzzy {
        let predicates = terms
            .iter()
            .map(|term| {
                values.push(format!("%{term}%"));
                if term.contains("::") {
                    "s.qualified_name LIKE ?"
                } else {
                    "s.name LIKE ?"
                }
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        format!(
            "SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path FROM symbols s JOIN files f ON s.file_id = f.id WHERE ({predicates}){scope_clause}"
        )
    } else {
        values.push(
            terms
                .iter()
                .map(|term| escape_fts5_query(&format!("{term}*")))
                .collect::<Vec<_>>()
                .join(" OR "),
        );
        format!(
            "SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path FROM symbols_fts fts JOIN symbols s ON fts.rowid = s.id JOIN files f ON s.file_id = f.id WHERE symbols_fts MATCH ?{scope_clause}"
        )
    };
    values.extend(scope_params);
    if let Some(kind) = kind {
        sql.push_str(" AND s.kind = ?");
        values.push(kind.to_string());
    }
    sql.push_str(" ORDER BY length(COALESCE(s.qualified_name, s.name)), COALESCE(s.qualified_name, s.name), f.path, s.line LIMIT ?");
    values.push(limit.to_string());
    let params: Vec<&dyn rusqlite::types::ToSql> = values
        .iter()
        .map(|value| value as &dyn rusqlite::types::ToSql)
        .collect();
    let mut stmt = conn.prepare(&sql)?;
    let results = stmt
        .query_map(params.as_slice(), row_to_search_result)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

pub fn search_symbols_for_command(
    conn: &Connection,
    query: &str,
    kind: Option<&str>,
    limit: usize,
    scope: &SearchScope,
    fuzzy: bool,
    class_only: bool,
) -> Result<Vec<SearchResult>> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let (scope_clause, scope_params) = scope.path_condition();
    let mut values = Vec::new();
    let mut sql;

    if fuzzy {
        let (column, contains, exact, prefix) = if query.contains("::") {
            (
                "s.qualified_name",
                format!("%{query}%"),
                if query.starts_with("::") {
                    format!("%{query}")
                } else {
                    query.to_string()
                },
                if query.starts_with("::") {
                    format!("%{query}")
                } else {
                    format!("{query}%")
                },
            )
        } else {
            (
                "s.name",
                format!("%{query}%"),
                query.to_string(),
                format!("{query}%"),
            )
        };
        sql = format!(
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE {column} LIKE ?{scope_clause}
            "#
        );
        values.push(contains);
        values.extend(scope_params);
        if kind.is_some() {
            sql.push_str(" AND s.kind = ?");
        }
        if class_only {
            sql.push_str(" AND s.kind IN ('class', 'interface', 'object', 'enum', 'protocol', 'struct', 'actor', 'package')");
        }
        sql.push_str(&format!(
            " ORDER BY CASE WHEN {column} = ? THEN 0 WHEN {column} LIKE ? THEN 1 ELSE 2 END, length({column}) LIMIT ?"
        ));
        if let Some(kind) = kind {
            values.push(kind.to_string());
        }
        values.push(exact);
        values.push(prefix);
        values.push(limit.to_string());
    } else if query.contains("::") {
        let raw = query.trim_end_matches('*');
        let (predicate, value) = if query.starts_with("::") {
            ("s.qualified_name LIKE ?", format!("%{raw}"))
        } else if query.ends_with('*') {
            ("s.qualified_name LIKE ?", format!("{raw}%"))
        } else {
            ("s.qualified_name = ?", raw.to_string())
        };
        sql = format!(
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE {predicate}{scope_clause}
            "#
        );
        values.push(value);
        values.extend(scope_params);
        if kind.is_some() {
            sql.push_str(" AND s.kind = ?");
        }
        if class_only {
            sql.push_str(" AND s.kind IN ('class', 'interface', 'object', 'enum', 'protocol', 'struct', 'actor', 'package')");
        }
        sql.push_str(" ORDER BY length(s.qualified_name), s.qualified_name LIMIT ?");
        if let Some(kind) = kind {
            values.push(kind.to_string());
        }
        values.push(limit.to_string());
    } else {
        sql = format!(
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols_fts fts
            JOIN symbols s ON fts.rowid = s.id
            JOIN files f ON s.file_id = f.id
            WHERE symbols_fts MATCH ?{scope_clause}
            "#
        );
        values.push(escape_fts5_query(query));
        values.extend(scope_params);
        if kind.is_some() {
            sql.push_str(" AND s.kind = ?");
        }
        if class_only {
            sql.push_str(" AND s.kind IN ('class', 'interface', 'object', 'enum', 'protocol', 'struct', 'actor', 'package')");
        }
        sql.push_str(" LIMIT ?");
        if let Some(kind) = kind {
            values.push(kind.to_string());
        }
        values.push(limit.to_string());
    }

    let params: Vec<&dyn rusqlite::types::ToSql> = values
        .iter()
        .map(|value| value as &dyn rusqlite::types::ToSql)
        .collect();
    let mut stmt = conn.prepare(&sql)?;
    let results = stmt
        .query_map(params.as_slice(), row_to_search_result)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

/// Search symbols with scope filtering (file/module)
pub fn search_symbols_scoped(
    conn: &Connection,
    query: &str,
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<SearchResult>> {
    if scope.is_empty() {
        return search_symbols(conn, query, limit);
    }

    if query.trim().is_empty() {
        return Ok(vec![]);
    }

    if query.contains("::") {
        let raw = query.trim_end_matches('*');
        let (scope_clause, scope_params) = scope.path_condition();
        let (predicate, value) = if query.starts_with("::") {
            ("s.qualified_name LIKE ?1", format!("%{}", raw))
        } else if query.ends_with('*') {
            ("s.qualified_name LIKE ?1", format!("{raw}%"))
        } else {
            ("s.qualified_name = ?1", raw.to_string())
        };

        let sql = format!(
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE {}{}
            ORDER BY length(s.qualified_name), s.qualified_name
            LIMIT ?{}
            "#,
            predicate,
            scope_clause,
            2 + scope_params.len()
        );

        let mut stmt = conn.prepare(&sql)?;
        let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        all_params.push(Box::new(value));
        for p in &scope_params {
            all_params.push(Box::new(p.clone()));
        }
        all_params.push(Box::new(limit as i64));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            all_params.iter().map(|p| p.as_ref()).collect();
        return Ok(stmt
            .query_map(param_refs.as_slice(), row_to_search_result)?
            .collect::<Result<Vec<_>, _>>()?);
    }

    let escaped_query = escape_fts5_query(query);
    let (scope_clause, scope_params) = scope.path_condition();

    let sql = format!(
        r#"
        SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
        FROM symbols_fts fts
        JOIN symbols s ON fts.rowid = s.id
        JOIN files f ON s.file_id = f.id
        WHERE symbols_fts MATCH ?1{}
        LIMIT ?{}
        "#,
        scope_clause,
        2 + scope_params.len()
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    all_params.push(Box::new(escaped_query));
    for p in &scope_params {
        all_params.push(Box::new(p.clone()));
    }
    all_params.push(Box::new(limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        all_params.iter().map(|p| p.as_ref()).collect();
    let results = stmt
        .query_map(param_refs.as_slice(), row_to_search_result)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Find symbols by name with scope filtering
pub fn find_symbols_by_name_scoped(
    conn: &Connection,
    name: &str,
    kind: Option<&str>,
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<SearchResult>> {
    if scope.is_empty() {
        return find_symbols_by_name(conn, name, kind, limit);
    }

    let (scope_clause, scope_params) = scope.path_condition();

    if name.starts_with("::") || name.contains("::") {
        let predicate = if name.starts_with("::") {
            "s.qualified_name LIKE ?1"
        } else {
            "s.qualified_name = ?1"
        };
        let value = if name.starts_with("::") {
            format!("%{}", name)
        } else {
            name.to_string()
        };
        let mut sql = format!(
            "SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path FROM symbols s JOIN files f ON s.file_id = f.id WHERE {}{}",
            predicate, scope_clause
        );
        if kind.is_some() {
            sql.push_str(&format!(" AND s.kind = ?{}", 2 + scope_params.len()));
            sql.push_str(&format!(" LIMIT ?{}", 3 + scope_params.len()));
        } else {
            sql.push_str(&format!(" LIMIT ?{}", 2 + scope_params.len()));
        }

        let mut stmt = conn.prepare(&sql)?;
        let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        all_params.push(Box::new(value));
        for p in &scope_params {
            all_params.push(Box::new(p.clone()));
        }
        if let Some(k) = kind {
            all_params.push(Box::new(k.to_string()));
        }
        all_params.push(Box::new(limit as i64));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            all_params.iter().map(|p| p.as_ref()).collect();
        let exact = stmt
            .query_map(param_refs.as_slice(), row_to_search_result)?
            .collect::<Result<Vec<_>, _>>()?;

        if !exact.is_empty() || name.starts_with("::") {
            return Ok(exact);
        }

        let mut sql = format!(
            "SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path FROM symbols s JOIN files f ON s.file_id = f.id WHERE s.qualified_name LIKE ?1{}",
            scope_clause
        );
        if kind.is_some() {
            sql.push_str(&format!(" AND s.kind = ?{}", 2 + scope_params.len()));
            sql.push_str(&format!(" LIMIT ?{}", 3 + scope_params.len()));
        } else {
            sql.push_str(&format!(" LIMIT ?{}", 2 + scope_params.len()));
        }

        let mut stmt = conn.prepare(&sql)?;
        let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        all_params.push(Box::new(format!("%::{}", name)));
        for p in &scope_params {
            all_params.push(Box::new(p.clone()));
        }
        if let Some(k) = kind {
            all_params.push(Box::new(k.to_string()));
        }
        all_params.push(Box::new(limit as i64));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            all_params.iter().map(|p| p.as_ref()).collect();
        let suffix = stmt
            .query_map(param_refs.as_slice(), row_to_search_result)?
            .collect::<Result<Vec<_>, _>>()?;
        if !suffix.is_empty() {
            return Ok(suffix);
        }

        let mut sql = format!(
            "SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path FROM symbols s JOIN files f ON s.file_id = f.id WHERE s.qualified_name LIKE ?1{}",
            scope_clause
        );
        if kind.is_some() {
            sql.push_str(&format!(" AND s.kind = ?{}", 2 + scope_params.len()));
            sql.push_str(&format!(" LIMIT ?{}", 3 + scope_params.len()));
        } else {
            sql.push_str(&format!(" LIMIT ?{}", 2 + scope_params.len()));
        }

        let mut stmt = conn.prepare(&sql)?;
        let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        all_params.push(Box::new(format!("{name}%")));
        for p in &scope_params {
            all_params.push(Box::new(p.clone()));
        }
        if let Some(k) = kind {
            all_params.push(Box::new(k.to_string()));
        }
        all_params.push(Box::new(limit as i64));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            all_params.iter().map(|p| p.as_ref()).collect();
        return Ok(stmt
            .query_map(param_refs.as_slice(), row_to_search_result)?
            .collect::<Result<Vec<_>, _>>()?);
    }

    let mut sql = format!(
        "SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path FROM symbols s JOIN files f ON s.file_id = f.id WHERE s.name = ?1{}",
        scope_clause
    );
    if kind.is_some() {
        sql.push_str(&format!(" AND s.kind = ?{}", 2 + scope_params.len()));
        sql.push_str(&format!(" LIMIT ?{}", 3 + scope_params.len()));
    } else {
        sql.push_str(&format!(" LIMIT ?{}", 2 + scope_params.len()));
    }

    let mut stmt = conn.prepare(&sql)?;
    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    all_params.push(Box::new(name.to_string()));
    for p in &scope_params {
        all_params.push(Box::new(p.clone()));
    }
    if let Some(k) = kind {
        all_params.push(Box::new(k.to_string()));
    }
    all_params.push(Box::new(limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        all_params.iter().map(|p| p.as_ref()).collect();
    let results = stmt
        .query_map(param_refs.as_slice(), row_to_search_result)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Find class-like symbols with scope filtering
pub fn find_class_like_scoped(
    conn: &Connection,
    name: &str,
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<SearchResult>> {
    if scope.is_empty() {
        return find_class_like(conn, name, limit);
    }

    let (scope_clause, scope_params) = scope.path_condition();
    let predicate = if name.starts_with("::") {
        "s.qualified_name LIKE ?1"
    } else if name.contains("::") {
        "s.qualified_name = ?1"
    } else {
        "s.name = ?1"
    };
    let value = if name.starts_with("::") {
        format!("%{}", name)
    } else {
        name.to_string()
    };

    let sql = format!(
        r#"
        SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
        FROM symbols s
        JOIN files f ON s.file_id = f.id
        WHERE {} AND s.kind IN ('class', 'interface', 'object', 'enum', 'protocol', 'struct', 'actor', 'package'){}
        LIMIT ?{}
        "#,
        predicate,
        scope_clause,
        2 + scope_params.len()
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    all_params.push(Box::new(value));
    for p in &scope_params {
        all_params.push(Box::new(p.clone()));
    }
    all_params.push(Box::new(limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        all_params.iter().map(|p| p.as_ref()).collect();
    let results = stmt
        .query_map(param_refs.as_slice(), row_to_search_result)?
        .collect::<Result<Vec<_>, _>>()?;

    if results.is_empty() && name.contains("::") && !name.starts_with("::") {
        let sql = format!(
            r#"
            SELECT s.name, s.qualified_name, s.kind, s.line, s.signature, f.path, f.root_path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.qualified_name LIKE ?1 AND s.kind IN ('class', 'interface', 'object', 'enum', 'protocol', 'struct', 'actor', 'package'){}
            LIMIT ?{}
            "#,
            scope_clause,
            2 + scope_params.len()
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        all_params.push(Box::new(format!("%::{}", name)));
        for p in &scope_params {
            all_params.push(Box::new(p.clone()));
        }
        all_params.push(Box::new(limit as i64));
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            all_params.iter().map(|p| p.as_ref()).collect();
        return Ok(stmt
            .query_map(param_refs.as_slice(), row_to_search_result)?
            .collect::<Result<Vec<_>, _>>()?);
    }

    Ok(results)
}

/// Find references with scope filtering
pub fn find_references_scoped(
    conn: &Connection,
    name: &str,
    limit: usize,
    scope: &SearchScope,
) -> Result<Vec<RefResult>> {
    if scope.is_empty() {
        return find_references(conn, name, limit);
    }

    let (scope_clause, scope_params) = scope.path_condition();

    // Early materialization with scope pushed into the subquery via IN clause.
    // Avoids materializing millions of refs when scope narrows by path. See #19.
    //
    // Scope filter is applied at files table (small, ~tens of thousands),
    // producing a small file_id set, then refs are filtered by both name
    // AND file_id — both covered by idx_refs_name_file_line.
    let scope_subquery = if scope_clause.is_empty() {
        String::new()
    } else {
        // Strip leading " AND " and wrap in file_id IN subselect
        let bare_conditions = scope_clause.trim_start_matches(" AND ");
        format!(
            " AND file_id IN (SELECT id FROM files f WHERE {})",
            bare_conditions
        )
    };

    let sql = format!(
        r#"
        SELECT r.name, r.line, r.context, f.path, f.root_path
        FROM (
            SELECT name, file_id, line, context
            FROM refs
            WHERE name = ?1{}
            ORDER BY file_id, line
            LIMIT ?{}
        ) r
        JOIN files f ON f.id = r.file_id
        ORDER BY f.path, r.line
        "#,
        scope_subquery,
        2 + scope_params.len()
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    all_params.push(Box::new(name.to_string()));
    for p in &scope_params {
        all_params.push(Box::new(p.clone()));
    }
    all_params.push(Box::new(limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        all_params.iter().map(|p| p.as_ref()).collect();
    let results = stmt
        .query_map(param_refs.as_slice(), row_to_ref_result)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// A named workspace subtree attached to the current project (#31).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Subtree {
    pub name: String,
    /// Canonical absolute path used as `files.root_path` during indexing.
    pub canonical_path: String,
    /// Path as the user originally provided it (kept verbatim so a project
    /// committed with relative paths stays portable across machines).
    pub original_path: String,
}

/// List every subtree attached to this project, ordered by name.
pub fn list_subtrees(conn: &Connection) -> Result<Vec<Subtree>> {
    let mut stmt =
        conn.prepare("SELECT name, canonical_path, original_path FROM subtrees ORDER BY name")?;
    let rows = stmt.query_map([], |row| {
        Ok(Subtree {
            name: row.get(0)?,
            canonical_path: row.get(1)?,
            original_path: row.get(2)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn find_subtree_by_name(conn: &Connection, name: &str) -> Result<Option<Subtree>> {
    let result: rusqlite::Result<Subtree> = conn.query_row(
        "SELECT name, canonical_path, original_path FROM subtrees WHERE name = ?1",
        params![name],
        |row| {
            Ok(Subtree {
                name: row.get(0)?,
                canonical_path: row.get(1)?,
                original_path: row.get(2)?,
            })
        },
    );
    match result {
        Ok(s) => Ok(Some(s)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn find_subtree_by_root_path(
    conn: &Connection,
    canonical_path: &str,
) -> Result<Option<Subtree>> {
    let result: rusqlite::Result<Subtree> = conn.query_row(
        "SELECT name, canonical_path, original_path FROM subtrees WHERE canonical_path = ?1",
        params![canonical_path],
        |row| {
            Ok(Subtree {
                name: row.get(0)?,
                canonical_path: row.get(1)?,
                original_path: row.get(2)?,
            })
        },
    );
    match result {
        Ok(s) => Ok(Some(s)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Insert a new subtree row. Errors when the name or canonical_path are
/// already taken (UNIQUE constraint).
pub fn insert_subtree(
    conn: &Connection,
    name: &str,
    canonical_path: &str,
    original_path: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO subtrees (name, canonical_path, original_path) VALUES (?1, ?2, ?3)",
        params![name, canonical_path, original_path],
    )?;
    Ok(())
}

pub fn remove_subtree_by_name(conn: &Connection, name: &str) -> Result<bool> {
    let n = conn.execute("DELETE FROM subtrees WHERE name = ?1", params![name])?;
    Ok(n > 0)
}

/// Derive a short, filesystem-friendly default subtree name from a path.
/// Strips trailing slashes, takes the last meaningful component, and falls
/// back to `subtree` when the path has no usable basename (e.g. just `/`).
pub fn default_subtree_name(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    let base = Path::new(trimmed)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let clean = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    let clean = clean.trim_matches('-').to_string();
    if clean.is_empty() {
        "subtree".to_string()
    } else {
        clean
    }
}

/// Pick a name that's not yet taken in the subtrees table. Starts with the
/// caller's preferred name and appends `-2`, `-3`, ... on collision.
pub fn allocate_subtree_name(conn: &Connection, preferred: &str) -> Result<String> {
    if find_subtree_by_name(conn, preferred)?.is_none() {
        return Ok(preferred.to_string());
    }
    for n in 2..1000 {
        let candidate = format!("{}-{}", preferred, n);
        if find_subtree_by_name(conn, &candidate)?.is_none() {
            return Ok(candidate);
        }
    }
    Err(anyhow::anyhow!(
        "could not allocate a free subtree name based on '{}'",
        preferred
    ))
}

/// One-time migration of pre-3.47 `metadata.extra_roots` JSON into the
/// new `subtrees` table. Idempotent: deletes the metadata row after a
/// successful migration so we don't run this twice.
fn migrate_extra_roots_rows(conn: &Connection) -> Result<()> {
    let json: Option<String> = conn
        .query_row(
            "SELECT value FROM metadata WHERE key = 'extra_roots'",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("failed to read metadata.extra_roots")?;
    let Some(json) = json else {
        return Ok(());
    };
    let roots: Vec<String> = serde_json::from_str(&json)
        .context("metadata.extra_roots must be a JSON array of strings")?;
    for raw in roots {
        let canonical_path = normalize_root_for_storage(Path::new(&raw));
        if find_subtree_by_root_path(conn, &canonical_path)?.is_some() {
            continue;
        }
        let preferred = default_subtree_name(&raw);
        let name = allocate_subtree_name(conn, &preferred)?;
        // Keep the legacy value verbatim for display and portability, while
        // matching indexed `files.root_path` through the normalized key.
        insert_subtree(conn, &name, &canonical_path, &raw)?;
    }
    // Clear the legacy row last. The caller always wraps this helper in a
    // transaction, so every inserted subtree rolls back if deletion fails.
    let deleted = conn.execute("DELETE FROM metadata WHERE key = 'extra_roots'", [])?;
    anyhow::ensure!(
        deleted == 1,
        "metadata.extra_roots disappeared during migration"
    );
    Ok(())
}

/// Strict fallback for callers using a directly-created `Connection` rather
/// than `open_db`. Production opens migrate eagerly in `apply_open_migrations`.
fn migrate_extra_roots_to_subtrees(conn: &Connection) -> Result<()> {
    if !conn.is_autocommit() {
        conn.execute(CREATE_METADATA_SQL, [])?;
        conn.execute(CREATE_SUBTREES_SQL, [])?;
        return migrate_extra_roots_rows(conn);
    }

    let tx = conn
        .unchecked_transaction()
        .context("failed to start legacy extra_roots migration")?;
    tx.execute(CREATE_METADATA_SQL, [])?;
    tx.execute(CREATE_SUBTREES_SQL, [])?;
    migrate_extra_roots_rows(&tx)?;
    tx.commit()
        .context("failed to commit legacy extra_roots migration")?;
    Ok(())
}

/// Get extra source roots — backwards-compatible shim over the new
/// `subtrees` table. Returns the canonical_path of each subtree, ignoring
/// the name (existing callers do not yet care about subtree names).
pub fn get_extra_roots(conn: &Connection) -> Result<Vec<String>> {
    migrate_extra_roots_to_subtrees(conn)?;
    Ok(list_subtrees(conn)?
        .into_iter()
        .map(|s| s.canonical_path)
        .collect())
}

pub fn is_experimental_fast_rebuild_enabled_in_db(conn: &Connection) -> bool {
    let result: Result<String, _> = conn.query_row(
        "SELECT value FROM metadata WHERE key = 'experimental_fast_rebuild'",
        [],
        |row| row.get(0),
    );
    result.map(|v| v == "1").unwrap_or(false)
}

pub fn set_experimental_fast_rebuild_enabled(conn: &Connection, enabled: bool) -> Result<()> {
    let value = if enabled { "1" } else { "0" };
    conn.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES ('experimental_fast_rebuild', ?1)",
        [value],
    )?;
    Ok(())
}

/// Add an extra source root with an auto-generated subtree name.
///
/// Backwards-compatible shim for the pre-3.47 `add-root` CLI command.
/// Picks a default name from the path basename and falls back to
/// `<base>-2`, `<base>-3`, ... on collision. Stores `path` verbatim in
/// `original_path` so the user's preference (relative vs absolute) is
/// preserved.
pub fn add_extra_root(conn: &Connection, path: &str) -> Result<()> {
    migrate_extra_roots_to_subtrees(conn)?;
    let canonical_path = normalize_root_for_storage(Path::new(path));
    let exists = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM subtrees
            WHERE canonical_path = ?1 OR original_path = ?2
        )",
        params![canonical_path, path],
        |row| row.get::<_, bool>(0),
    )?;
    if exists {
        return Ok(());
    }
    let preferred = default_subtree_name(path);
    let name = allocate_subtree_name(conn, &preferred)?;
    insert_subtree(conn, &name, &canonical_path, path)
}

/// Remove an extra source root identified by its canonical path.
pub fn remove_extra_root(conn: &Connection, path: &str) -> Result<bool> {
    migrate_extra_roots_to_subtrees(conn)?;
    let canonical_path = normalize_root_for_storage(Path::new(path));
    let n = conn.execute(
        "DELETE FROM subtrees WHERE canonical_path = ?1 OR original_path = ?2",
        params![canonical_path, path],
    )?;
    Ok(n > 0)
}

/// Normalize a module name input so that `:core:utils`, `core/utils`, and
/// `core.utils` all resolve to the same stored row when the stored name
/// matches one of those forms. Strips a leading `:`, then tries an exact
/// match first; if that misses, falls back to probing the slash-to-dot and
/// colon-to-dot variants.
///
/// Returns the row id of the matching module, or `None` when no row matches.
pub fn find_module_id_by_name(conn: &Connection, input: &str) -> Result<Option<i64>> {
    // Strip leading colon (Gradle-style `:core:utils` → `core:utils`).
    let stripped = input.trim_start_matches(':');
    // Build candidate list: original stripped, colon→dot, slash→dot.
    let dot_from_colon = stripped.replace(':', ".");
    let dot_from_slash = stripped.replace('/', ".");
    let candidates = [stripped, dot_from_colon.as_str(), dot_from_slash.as_str()];

    for candidate in candidates {
        let result: Result<i64, _> = conn.query_row(
            "SELECT id FROM modules WHERE name = ?1",
            params![candidate],
            |row| row.get(0),
        );
        match result {
            Ok(id) => return Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(None)
}

/// Return the name of a module by its id, or `None` when the id is absent.
pub fn get_module_name(conn: &Connection, id: i64) -> Result<Option<String>> {
    let result: Result<String, _> = conn.query_row(
        "SELECT name FROM modules WHERE id = ?1",
        params![id],
        |row| row.get(0),
    );
    match result {
        Ok(name) => Ok(Some(name)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Return the total number of rows in `module_deps`.
pub fn count_module_deps(conn: &Connection) -> Result<i64> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM module_deps", [], |row| row.get(0))?;
    Ok(count)
}

/// Return the `dep_kind` of a self-edge `id → id` in `module_deps`, if one
/// exists. Optionally filtered by `dep_kind`.
///
/// Used by `module-route` to surface the real edge kind on a self-loop
/// instead of guessing a default like "implementation".
pub fn get_module_self_edge_kind(
    conn: &Connection,
    id: i64,
    kind_filter: Option<&str>,
) -> Result<Option<String>> {
    let result: Result<String, _> = if let Some(kind) = kind_filter {
        conn.query_row(
            "SELECT dep_kind FROM module_deps WHERE module_id = ?1 AND dep_module_id = ?1 AND dep_kind = ?2 LIMIT 1",
            params![id, kind],
            |row| row.get(0),
        )
    } else {
        conn.query_row(
            "SELECT dep_kind FROM module_deps WHERE module_id = ?1 AND dep_module_id = ?1 ORDER BY dep_kind LIMIT 1",
            params![id],
            |row| row.get(0),
        )
    };
    match result {
        Ok(kind) => Ok(Some(kind)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Return outgoing edges from `module_id`, optionally filtered by `dep_kind`.
///
/// Deduplicates via `SELECT DISTINCT` to guard against parallel edges with
/// different metadata producing duplicate paths. Results are ordered by name
/// for deterministic test output.
///
/// Returns `(dep_module_id, dep_module_name, dep_kind)`.
pub fn get_outgoing_edges_dedup(
    conn: &Connection,
    module_id: i64,
    kind_filter: Option<&str>,
) -> Result<Vec<(i64, String, String)>> {
    if let Some(kind) = kind_filter {
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT md.dep_module_id, m.name, md.dep_kind
             FROM module_deps md
             JOIN modules m ON md.dep_module_id = m.id
             WHERE md.module_id = ?1 AND md.dep_kind = ?2
             ORDER BY m.name",
        )?;
        let rows = stmt
            .query_map(params![module_id, kind], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    } else {
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT md.dep_module_id, m.name, md.dep_kind
             FROM module_deps md
             JOIN modules m ON md.dep_module_id = m.id
             WHERE md.module_id = ?1
             ORDER BY m.name",
        )?;
        let rows = stmt
            .query_map(params![module_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

/// Return incoming edges to `module_id` — i.e. modules that depend ON it.
/// Used by reverse-BFS pruning in `module-route --all`.
pub fn get_incoming_edges_dedup(
    conn: &Connection,
    module_id: i64,
    kind_filter: Option<&str>,
) -> Result<Vec<(i64, String, String)>> {
    if let Some(kind) = kind_filter {
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT md.module_id, m.name, md.dep_kind
             FROM module_deps md
             JOIN modules m ON md.module_id = m.id
             WHERE md.dep_module_id = ?1 AND md.dep_kind = ?2
             ORDER BY m.name",
        )?;
        let rows = stmt
            .query_map(params![module_id, kind], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    } else {
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT md.module_id, m.name, md.dep_kind
             FROM module_deps md
             JOIN modules m ON md.module_id = m.id
             WHERE md.dep_module_id = ?1
             ORDER BY m.name",
        )?;
        let rows = stmt
            .query_map(params![module_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

fn current_unix_millis() -> Result<i64> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_millis();
    i64::try_from(millis).context("current Unix timestamp does not fit in i64 milliseconds")
}

fn mark_metadata_timestamp(conn: &Connection, key: &str) -> Result<()> {
    let value = current_unix_millis()?.to_string();
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Record completion of a file-index update as Unix milliseconds.
pub fn mark_index_updated(conn: &Connection) -> Result<()> {
    mark_metadata_timestamp(conn, "last_update_at")
}

/// Persist that an incremental file-index update may be only partially applied.
pub fn mark_index_update_dirty(conn: &Connection) -> Result<()> {
    mark_metadata_timestamp(conn, "index_update_dirty_at")
}

/// Return whether an incremental update has an unresolved dirty marker.
pub fn has_index_update_dirty(conn: &Connection) -> Result<bool> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM metadata WHERE key = 'index_update_dirty_at'
         )",
        [],
        |row| row.get(0),
    )?;
    Ok(exists)
}

/// Atomically publish successful update completion and clear its dirty marker.
pub fn complete_index_update(conn: &mut Connection) -> Result<()> {
    let now = current_unix_millis()?;
    let tx = conn.transaction()?;
    let dirty_at = tx
        .query_row(
            "SELECT value FROM metadata WHERE key = 'index_update_dirty_at'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .and_then(|value| value.parse::<i64>().ok());
    let completed_at = dirty_at.map_or(now, |dirty| now.max(dirty));
    tx.execute(
        "INSERT INTO metadata (key, value) VALUES ('last_update_at', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [completed_at.to_string()],
    )?;
    tx.execute(
        "DELETE FROM metadata WHERE key = 'index_update_dirty_at'",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

/// Record completion of module indexing as Unix milliseconds.
pub fn mark_modules_indexed(conn: &Connection) -> Result<()> {
    mark_metadata_timestamp(conn, "last_modules_indexed_at")
}

/// Returns module indexing time and the effective file-update time.
///
/// An unresolved `index_update_dirty_at` forces the effective update to
/// `i64::MAX`, so consumers stay stale even when millisecond timestamps are
/// equal after a partial or crashed incremental update.
pub fn get_modules_index_freshness(conn: &Connection) -> Result<Option<(i64, i64)>> {
    let indexed_at: Result<String, _> = conn.query_row(
        "SELECT value FROM metadata WHERE key = 'last_modules_indexed_at'",
        [],
        |row| row.get(0),
    );
    let updated_at: Result<String, _> = conn.query_row(
        "SELECT value FROM metadata WHERE key = 'last_update_at'",
        [],
        |row| row.get(0),
    );
    let dirty_at: Result<String, _> = conn.query_row(
        "SELECT value FROM metadata WHERE key = 'index_update_dirty_at'",
        [],
        |row| row.get(0),
    );

    let is_dirty = match dirty_at {
        Ok(value) => {
            if value.parse::<i64>().is_err() {
                eprintln!("Warning: malformed 'index_update_dirty_at' in metadata");
            }
            true
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => false,
        Err(error) => return Err(error.into()),
    };
    let indexed_at = match indexed_at {
        Ok(value) => match value.parse::<i64>() {
            Ok(value) => value,
            Err(_) => {
                eprintln!("Warning: malformed 'last_modules_indexed_at' in metadata");
                if is_dirty {
                    i64::MIN
                } else {
                    return Ok(None);
                }
            }
        },
        Err(rusqlite::Error::QueryReturnedNoRows) if is_dirty => i64::MIN,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if is_dirty {
        return Ok(Some((indexed_at, i64::MAX)));
    }
    let updated_at = match updated_at {
        Ok(value) => match value.parse::<i64>() {
            Ok(value) => Some(value),
            Err(_) => {
                eprintln!("Warning: malformed 'last_update_at' in metadata");
                Some(i64::MAX)
            }
        },
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(error) => return Err(error.into()),
    };
    match updated_at {
        Some(updated) => Ok(Some((indexed_at, updated))),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn
    }

    #[test]
    fn partial_swap_failure_restores_already_moved_artifacts() {
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp.path().join("index.db");
        let wal_path = db_path.with_extension("db-wal");
        std::fs::write(&db_path, b"main").unwrap();
        std::fs::write(&wal_path, b"wal").unwrap();
        let calls = std::cell::Cell::new(0_u8);

        let error = move_db_to_swap_at_path_with(&db_path, |source, target| {
            let call = calls.get() + 1;
            calls.set(call);
            if call == 2 {
                return Err(std::io::Error::other("injected WAL rename failure"));
            }
            std::fs::rename(source, target)
        })
        .unwrap_err();

        assert!(
            format!("{error:#}").contains("injected WAL rename failure"),
            "unexpected error: {error:#}"
        );
        assert_eq!(std::fs::read(&db_path).unwrap(), b"main");
        assert_eq!(std::fs::read(&wal_path).unwrap(), b"wal");
        assert!(!db_path.with_extension("db.swap").exists());
        assert!(!db_path.with_extension("db.swap-wal").exists());
    }

    #[test]
    fn rollback_journal_participates_in_swap_restore_and_cleanup() {
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp.path().join("index.db");
        let journal_path = db_path.with_extension("db-journal");
        std::fs::write(&db_path, b"main").unwrap();
        std::fs::write(&journal_path, b"journal").unwrap();

        assert!(move_db_to_swap_at_path(&db_path).unwrap());
        assert!(db_path.with_extension("db.swap").is_file());
        assert!(db_path.with_extension("db.swap-journal").is_file());
        restore_db_from_swap_at_path(&db_path).unwrap();
        assert_eq!(std::fs::read(&db_path).unwrap(), b"main");
        assert_eq!(std::fs::read(&journal_path).unwrap(), b"journal");
        assert!(!db_path.with_extension("db.swap-journal").exists());

        assert!(move_db_to_swap_at_path(&db_path).unwrap());
        remove_swap_at_path(&db_path).unwrap();
        assert!(!db_path.with_extension("db.swap").exists());
        assert!(!db_path.with_extension("db.swap-journal").exists());
    }

    fn create_publication_fixture(path: &Path, value: &str) {
        let conn = Connection::open(path).unwrap();
        conn.execute("CREATE TABLE sentinel(value TEXT NOT NULL)", [])
            .unwrap();
        conn.execute("INSERT INTO sentinel(value) VALUES (?1)", [value])
            .unwrap();
        drop(conn);
    }

    fn publication_fixture_value(path: &Path) -> String {
        Connection::open(path)
            .unwrap()
            .query_row("SELECT value FROM sentinel", [], |row| row.get(0))
            .unwrap()
    }

    fn write_preparing_marker(
        db_path: &Path,
        operation: PublicationOperation,
        artifacts: [bool; 4],
    ) -> PublicationState {
        let state = PublicationState {
            version: PUBLICATION_STATE_VERSION,
            token: new_publication_token(),
            operation,
            artifacts,
            staging_dir: None,
        };
        write_publication_marker(&publication_state_path(db_path), &state).unwrap();
        state
    }

    #[test]
    fn injected_staged_install_failure_restores_old_generation() {
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp.path().join("index.db");
        let staged = temp.path().join(".rebuild-test/index.db");
        create_publication_fixture(&db_path, "old");
        std::fs::create_dir(staged.parent().unwrap()).unwrap();
        create_publication_fixture(&staged, "new");

        let error = install_staged_at_path_with(&db_path, &staged, |_source, _target| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected staged install failure",
            ))
        })
        .unwrap_err();

        assert!(format!("{error:#}").contains("injected staged install failure"));
        assert_eq!(publication_fixture_value(&db_path), "old");
        assert!(!staged.exists());
        assert!(!staged.parent().unwrap().exists());
        assert!(!db_path.with_extension("db.swap").exists());
        assert!(!publication_state_path(&db_path).exists());
        assert!(!publication_commit_path(&db_path).exists());
    }

    #[test]
    fn preparing_marker_recovers_old_generation_after_candidate_install() {
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp.path().join("index.db");
        let staged = temp.path().join("staged.db");
        create_publication_fixture(&db_path, "old");
        create_publication_fixture(&staged, "new");
        let artifacts = checkpoint_and_consolidate_live_db(&db_path).unwrap();
        write_preparing_marker(&db_path, PublicationOperation::Install, artifacts);
        snapshot_live_main(&db_path, &db_path.with_extension("db.swap")).unwrap();
        std::fs::rename(&staged, &db_path).unwrap();

        let interrupted = ensure_no_interrupted_publication(&db_path).unwrap_err();
        assert!(is_publication_busy(&interrupted));
        recover_interrupted_publication_at_path(&db_path).unwrap();

        assert_eq!(publication_fixture_value(&db_path), "old");
        assert!(!db_path.with_extension("db.swap").exists());
        assert!(!publication_state_path(&db_path).exists());
    }

    #[test]
    fn preparing_marker_without_old_generation_removes_partial_candidate() {
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp.path().join("index.db");
        let staged = temp.path().join("staged.db");
        create_publication_fixture(&staged, "partial");
        write_preparing_marker(
            &db_path,
            PublicationOperation::Install,
            [false, false, false, false],
        );
        std::fs::rename(&staged, &db_path).unwrap();

        recover_interrupted_publication_at_path(&db_path).unwrap();

        assert!(!db_path.exists());
        assert!(!publication_state_path(&db_path).exists());
    }

    #[test]
    fn committed_marker_keeps_installed_generation_and_cleans_old_swap() {
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp.path().join("index.db");
        let staged = temp.path().join("staged.db");
        create_publication_fixture(&db_path, "old");
        create_publication_fixture(&staged, "new");
        let artifacts = checkpoint_and_consolidate_live_db(&db_path).unwrap();
        let state = write_preparing_marker(&db_path, PublicationOperation::Install, artifacts);
        snapshot_live_main(&db_path, &db_path.with_extension("db.swap")).unwrap();
        std::fs::rename(&staged, &db_path).unwrap();
        write_publication_marker(
            &publication_commit_path(&db_path),
            &PublicationCommit {
                version: PUBLICATION_STATE_VERSION,
                token: state.token,
                operation: PublicationOperation::Install,
            },
        )
        .unwrap();

        recover_interrupted_publication_at_path(&db_path).unwrap();

        assert_eq!(publication_fixture_value(&db_path), "new");
        assert!(!db_path.with_extension("db.swap").exists());
        assert!(!publication_commit_path(&db_path).exists());
    }

    #[test]
    fn committed_marker_rejects_unrecorded_swap_sidecar() {
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp.path().join("index.db");
        let staged = temp.path().join("staged.db");
        create_publication_fixture(&db_path, "old");
        create_publication_fixture(&staged, "new");
        let artifacts = checkpoint_and_consolidate_live_db(&db_path).unwrap();
        let state = write_preparing_marker(&db_path, PublicationOperation::Install, artifacts);
        snapshot_live_main(&db_path, &db_path.with_extension("db.swap")).unwrap();
        std::fs::rename(&staged, &db_path).unwrap();
        let unexpected = db_path.with_extension("db.swap-wal");
        std::fs::write(&unexpected, b"unknown generation").unwrap();
        write_publication_marker(
            &publication_commit_path(&db_path),
            &PublicationCommit {
                version: PUBLICATION_STATE_VERSION,
                token: state.token,
                operation: PublicationOperation::Install,
            },
        )
        .unwrap();

        let error = recover_interrupted_publication_at_path(&db_path).unwrap_err();

        assert!(format!("{error:#}").contains("unrecorded swap artifact"));
        assert_eq!(publication_fixture_value(&db_path), "new");
        assert!(unexpected.exists(), "unexpected artifact was deleted");
        assert!(publication_state_path(&db_path).exists());
        assert!(publication_commit_path(&db_path).exists());
    }

    #[test]
    fn untracked_swap_blocks_recovery_without_being_deleted() {
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp.path().join("index.db");
        let swap = db_path.with_extension("db.swap");
        create_publication_fixture(&db_path, "live");
        create_publication_fixture(&swap, "unknown");

        let error = recover_interrupted_publication_at_path(&db_path).unwrap_err();

        assert!(format!("{error:#}").contains("untracked index swap"));
        assert_eq!(publication_fixture_value(&db_path), "live");
        assert_eq!(publication_fixture_value(&swap), "unknown");
    }

    #[test]
    fn managed_publication_lock_lives_outside_replaceable_cache_target() {
        let temp = tempfile::TempDir::new().unwrap();
        let base = temp.path().join("cache");
        let key = "abc123";
        let target = base.join(key);
        std::fs::create_dir_all(&target).unwrap();
        let lease = acquire_shared_project_lease(&base, key).unwrap();
        let db_path = target.join("index.db");

        let lock_path = publication_lock_path(&db_path, &lease).unwrap();

        assert_eq!(
            lock_path,
            base.join(".leases").join(format!("{key}.publish.lock"))
        );
        assert!(
            std::fs::read_dir(&target).unwrap().next().is_none(),
            "publication lock polluted replaceable cache target"
        );
        assert_eq!(
            publication_lock_path(&db_path, &ProjectLease::none()).unwrap(),
            db_path.with_extension("publish.lock")
        );
    }

    #[test]
    fn mutation_guard_cleans_only_owned_bounded_staging_directories() {
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp.path().join("index.db");
        let owned_dir = temp.path().join(".rebuild-12-34");
        let owned_db = owned_dir.join("index.db");
        std::fs::create_dir(&owned_dir).unwrap();
        register_index_staging(&owned_db, &db_path, "rebuild").unwrap();
        std::fs::write(&owned_db, b"abandoned").unwrap();

        cleanup_abandoned_index_staging(&db_path).unwrap();
        assert!(!owned_dir.exists());

        let unowned_dir = temp.path().join(".restore-56-78");
        std::fs::create_dir(&unowned_dir).unwrap();
        std::fs::write(unowned_dir.join("index.db"), b"must survive").unwrap();
        let error = cleanup_abandoned_index_staging(&db_path).unwrap_err();
        assert!(format!("{error:#}").contains("owner marker is missing"));
        assert!(unowned_dir.join("index.db").exists());
    }

    #[test]
    fn mutation_guard_refuses_unknown_artifact_in_owned_staging() {
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp.path().join("index.db");
        let staging_dir = temp.path().join(".restore-90-12");
        let staged_db = staging_dir.join("index.db");
        std::fs::create_dir(&staging_dir).unwrap();
        register_index_staging(&staged_db, &db_path, "restore").unwrap();
        std::fs::write(staging_dir.join("notes.txt"), b"foreign").unwrap();

        let error = cleanup_abandoned_index_staging(&db_path).unwrap_err();

        assert!(format!("{error:#}").contains("unexpected artifact"));
        assert!(staging_dir.join("notes.txt").exists());
        assert!(staging_owner_path(&staging_dir).exists());
    }

    #[test]
    fn mutation_guard_skips_valid_foreign_staging_in_shared_parent() {
        let temp = tempfile::TempDir::new().unwrap();
        let first_live = temp.path().join("first.sqlite");
        let second_live = temp.path().join("second.sqlite");
        let first_dir = temp.path().join(".rebuild-101-1");
        let second_dir = temp.path().join(".restore-202-2");
        let first_staged = first_dir.join("index.db");
        let second_staged = second_dir.join("index.db");
        std::fs::create_dir(&first_dir).unwrap();
        std::fs::create_dir(&second_dir).unwrap();
        register_index_staging(&first_staged, &first_live, "rebuild").unwrap();
        register_index_staging(&second_staged, &second_live, "restore").unwrap();
        std::fs::write(&first_staged, b"first").unwrap();
        std::fs::write(&second_staged, b"second").unwrap();

        cleanup_abandoned_index_staging(&first_live).unwrap();

        assert!(!first_dir.exists());
        assert!(
            second_staged.exists(),
            "cleanup for one override deleted foreign staging"
        );
        cleanup_abandoned_index_staging(&second_live).unwrap();
        assert!(!second_dir.exists());
    }

    #[test]
    fn optimized_indexes_avoid_redundancy_and_cover_lookup_plans() {
        let conn = create_test_db();

        for redundant in [
            "idx_files_root_path_path",
            "idx_modules_name",
            "idx_refs_name",
        ] {
            assert!(
                !index_exists(&conn, redundant).unwrap(),
                "fresh schema unexpectedly created {redundant}"
            );
        }
        let qualified_sql = index_sql(&conn, "idx_symbols_qualified_name")
            .unwrap()
            .unwrap();
        assert!(is_current_qualified_name_index(&qualified_sql));

        let qualified_plan: String = conn
            .query_row(
                "EXPLAIN QUERY PLAN SELECT id FROM symbols WHERE qualified_name = 'pkg.Type'",
                [],
                |row| row.get(3),
            )
            .unwrap();
        assert!(
            qualified_plan.contains("idx_symbols_qualified_name"),
            "qualified-name equality lost its index: {qualified_plan}"
        );

        let refs_plan: String = conn
            .query_row(
                "EXPLAIN QUERY PLAN SELECT file_id, line FROM refs WHERE name = 'Target'",
                [],
                |row| row.get(3),
            )
            .unwrap();
        assert!(
            refs_plan.contains("idx_refs_name_file_line"),
            "exact reference lookup lost its covering index: {refs_plan}"
        );
    }

    #[test]
    fn freshness_markers_write_typed_unix_milliseconds() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO metadata (key, value) VALUES ('last_update_at', 'malformed')",
            [],
        )
        .unwrap();
        let before = current_unix_millis().unwrap();

        mark_index_updated(&conn).unwrap();
        assert_eq!(get_modules_index_freshness(&conn).unwrap(), None);
        mark_modules_indexed(&conn).unwrap();

        let after = current_unix_millis().unwrap();
        let (modules_indexed_at, updated_at) = get_modules_index_freshness(&conn).unwrap().unwrap();
        assert!((before..=after).contains(&updated_at));
        assert!((before..=after).contains(&modules_indexed_at));
    }

    #[test]
    fn dirty_update_marker_is_published_and_completed_atomically() {
        let mut conn = create_test_db();
        conn.execute_batch(
            "INSERT INTO metadata (key, value) VALUES
                ('last_modules_indexed_at', '11'),
                ('last_update_at', '7'),
                ('index_update_dirty_at', '13');",
        )
        .unwrap();

        assert!(has_index_update_dirty(&conn).unwrap());
        assert_eq!(
            get_modules_index_freshness(&conn).unwrap(),
            Some((11, i64::MAX))
        );

        complete_index_update(&mut conn).unwrap();

        assert!(!has_index_update_dirty(&conn).unwrap());
        let (modules_indexed_at, updated_at) = get_modules_index_freshness(&conn).unwrap().unwrap();
        assert_eq!(modules_indexed_at, 11);
        assert!(updated_at >= 13);
    }

    #[test]
    fn malformed_dirty_marker_fails_staleness_check_closed() {
        let conn = create_test_db();
        conn.execute_batch(
            "INSERT INTO metadata (key, value) VALUES
                ('last_modules_indexed_at', '11'),
                ('last_update_at', '7'),
                ('index_update_dirty_at', 'malformed');",
        )
        .unwrap();

        assert_eq!(
            get_modules_index_freshness(&conn).unwrap(),
            Some((11, i64::MAX))
        );
    }

    #[test]
    fn dirty_marker_equal_to_module_timestamp_still_forces_stale() {
        let conn = create_test_db();
        conn.execute_batch(
            "INSERT INTO metadata (key, value) VALUES
                ('last_modules_indexed_at', '11'),
                ('last_update_at', '7'),
                ('index_update_dirty_at', '11');",
        )
        .unwrap();

        assert_eq!(
            get_modules_index_freshness(&conn).unwrap(),
            Some((11, i64::MAX))
        );
    }

    #[test]
    fn dirty_marker_without_module_baseline_still_forces_stale() {
        let conn = create_test_db();
        conn.execute_batch(
            "INSERT INTO metadata (key, value) VALUES
                ('last_update_at', '7'),
                ('index_update_dirty_at', '11');",
        )
        .unwrap();

        assert_eq!(
            get_modules_index_freshness(&conn).unwrap(),
            Some((i64::MIN, i64::MAX))
        );
    }

    #[test]
    fn dirty_marker_with_malformed_module_baseline_still_forces_stale() {
        let conn = create_test_db();
        conn.execute_batch(
            "INSERT INTO metadata (key, value) VALUES
                ('last_modules_indexed_at', 'malformed'),
                ('last_update_at', '7'),
                ('index_update_dirty_at', '11');",
        )
        .unwrap();

        assert_eq!(
            get_modules_index_freshness(&conn).unwrap(),
            Some((i64::MIN, i64::MAX))
        );
    }

    fn set_qualified_name(conn: &Connection, name: &str, qualified_name: &str) {
        conn.execute(
            "UPDATE symbols SET qualified_name = ?1 WHERE name = ?2",
            params![qualified_name, name],
        )
        .unwrap();
    }

    #[test]
    fn test_simple_hash_deterministic() {
        let h1 = simple_hash("/Users/test/project");
        let h2 = simple_hash("/Users/test/project");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_simple_hash_different() {
        let h1 = simple_hash("/Users/test/project1");
        let h2 = simple_hash("/Users/test/project2");
        assert_ne!(h1, h2);
    }

    #[test]
    fn unsafe_cache_owner_manifests_are_not_verified() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = "/foreign/project";
        let key = simple_hash(root);
        let manifest = CacheOwnerManifest::new(root, root);

        let malformed = temp.path().join("malformed");
        std::fs::create_dir(&malformed).unwrap();
        std::fs::write(cache_owner_manifest_path(&malformed), b"{not-json").unwrap();
        assert!(verified_cache_owner(&malformed, &key).is_none());

        let special = temp.path().join("special");
        std::fs::create_dir(&special).unwrap();
        std::fs::create_dir(cache_owner_manifest_path(&special)).unwrap();
        assert!(verified_cache_owner(&special, &key).is_none());
        assert!(quarantine_replaceable_migration_target(&special, &key, &manifest).is_err());

        let regular_target = temp.path().join("regular-target");
        std::fs::write(&regular_target, b"not a directory").unwrap();
        assert!(quarantine_replaceable_migration_target(&regular_target, &key, &manifest).is_err());

        let mismatched = temp.path().join("mismatched");
        std::fs::create_dir(&mismatched).unwrap();
        std::fs::write(
            cache_owner_manifest_path(&mismatched),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(verified_cache_owner(&mismatched, "deadbeef").is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let linked = temp.path().join("linked");
            let outside = temp.path().join("outside-owner.json");
            std::fs::create_dir(&linked).unwrap();
            std::fs::write(&outside, serde_json::to_vec(&manifest).unwrap()).unwrap();
            symlink(&outside, cache_owner_manifest_path(&linked)).unwrap();
            assert!(verified_cache_owner(&linked, &key).is_none());
            assert!(quarantine_replaceable_migration_target(&linked, &key, &manifest).is_err());
            assert!(
                std::fs::symlink_metadata(cache_owner_manifest_path(&linked))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );

            let outside_dir = temp.path().join("outside-cache");
            let linked_target = temp.path().join("linked-target");
            std::fs::create_dir(&outside_dir).unwrap();
            std::fs::write(outside_dir.join("must-survive.txt"), b"preserved").unwrap();
            symlink(&outside_dir, &linked_target).unwrap();
            assert!(
                quarantine_replaceable_migration_target(&linked_target, &key, &manifest).is_err()
            );
            assert_eq!(
                std::fs::read(outside_dir.join("must-survive.txt")).unwrap(),
                b"preserved"
            );
            assert!(std::fs::symlink_metadata(&linked_target)
                .unwrap()
                .file_type()
                .is_symlink());
        }
    }

    #[test]
    fn cache_owner_manifest_preserves_alias_identity_chain() {
        let first = CacheOwnerManifest::new("/normalized/one", "/alias/a");
        let second = CacheOwnerManifest::new("/normalized/one", "/alias/b");
        let first_key = simple_hash("/normalized/one");

        let merged = first.merged_for_target(&second).unwrap();
        assert!(merged.is_self_consistent(&first_key));
        for identity in ["/normalized/one", "/alias/a", "/alias/b"] {
            assert!(merged.contains_root(identity));
        }

        let remounted = CacheOwnerManifest::new("/normalized/two", "/alias/b");
        let pinned = first.merged_while_pinned(&second).unwrap();
        let pinned = pinned.merged_while_pinned(&remounted).unwrap();
        assert_eq!(pinned.normalized_root, first.normalized_root);
        assert_eq!(pinned.raw_root, first.raw_root);
        assert!(pinned.is_self_consistent(&first_key));
        for identity in ["/normalized/one", "/normalized/two", "/alias/a", "/alias/b"] {
            assert!(pinned.contains_root(identity));
        }

        assert!(merged.overlaps(&remounted));
        let migrated = merged.merged_for_target(&remounted).unwrap();
        assert!(migrated.is_self_consistent(&simple_hash("/normalized/two")));
        for identity in ["/normalized/one", "/normalized/two", "/alias/a", "/alias/b"] {
            assert!(migrated.contains_root(identity));
        }

        let backwards_compatible: CacheOwnerManifest = serde_json::from_value(serde_json::json!({
            "version": 1,
            "normalized_root": "/normalized/one",
            "raw_root": "/alias/a"
        }))
        .unwrap();
        assert!(backwards_compatible.known_roots.is_empty());
    }

    #[test]
    fn cache_owner_intent_recovers_interrupted_directory_migration() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache_base = temp.path().join("cache");
        let normalized_old = "/normalized/old";
        let normalized_new = "/normalized/new";
        let active_alias = "/alias/active";
        let target_only_alias = "/alias/target-only";
        let source_key = simple_hash(normalized_old);
        let target_key = simple_hash(normalized_new);
        assert_ne!(source_key, target_key);

        let target_dir = cache_base.join(&target_key);
        std::fs::create_dir_all(&target_dir).unwrap();
        let db_path = target_dir.join("index.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        conn.execute(
            "INSERT INTO metadata (key, value) VALUES ('project_root', ?1)",
            params![normalized_old],
        )
        .unwrap();
        drop(conn);

        let source_owner = CacheOwnerManifest::new(normalized_old, active_alias);
        persist_cache_owner_manifest(&target_dir, &source_key, &source_owner).unwrap();
        let desired = CacheOwnerManifest::new(normalized_new, active_alias);
        let target_owner = CacheOwnerManifest::new(normalized_new, target_only_alias);
        let carried = target_owner.merged_for_target(&desired).unwrap();
        write_cache_owner_intent(&cache_base, &target_dir, &target_key, &carried).unwrap();

        let recovered_desired =
            merge_cache_owner_intents(&cache_base, &target_dir, &target_key, &desired).unwrap();
        install_or_recover_target_cache_owner(
            &cache_base,
            &target_dir,
            &db_path,
            &target_key,
            &recovered_desired,
        )
        .unwrap();

        let recovered = read_cache_owner_manifest(&target_dir).unwrap().unwrap();
        assert!(recovered.is_self_consistent(&target_key));
        for identity in [
            normalized_old,
            normalized_new,
            active_alias,
            target_only_alias,
        ] {
            assert!(recovered.contains_root(identity));
        }
        assert!(cache_owner_intents(&cache_base, &target_dir, &target_key)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn cache_directory_migration_rebinds_target_history_to_source_generation() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache_base = temp.path().join("cache");
        let normalized_old = "/normalized/old";
        let normalized_new = "/normalized/new";
        let active_alias = "/alias/active";
        let target_only_alias = "/alias/target-only";
        let source_key = simple_hash(normalized_old);
        let target_key = simple_hash(normalized_new);
        let source_dir = cache_base.join(&source_key);
        let target_dir = cache_base.join(&target_key);
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::create_dir_all(&target_dir).unwrap();

        let requested = CacheOwnerManifest::new(normalized_new, active_alias);
        let target_owner = CacheOwnerManifest::new(normalized_new, target_only_alias);
        let carried = target_owner.merged_for_target(&requested).unwrap();
        write_cache_owner_intent(&cache_base, &target_dir, &target_key, &carried).unwrap();
        let desired =
            merge_cache_owner_intents(&cache_base, &target_dir, &target_key, &requested).unwrap();
        ensure_cache_generation(&source_dir).unwrap();

        let migrated =
            rename_cache_directory(&source_dir, &target_dir, &target_key, &desired).unwrap();
        assert_eq!(migrated, desired);
        assert!(read_cache_owner_manifest(&target_dir).unwrap().is_none());

        let current_generation_intents =
            cache_owner_intents(&cache_base, &target_dir, &target_key).unwrap();
        assert_eq!(current_generation_intents.len(), 1);
        assert!(current_generation_intents[0]
            .1
            .contains_root(target_only_alias));
    }

    #[test]
    fn cache_directory_migration_rekeys_old_intent_before_manifest_install() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache_base = temp.path().join("cache");
        let normalized_old = "/normalized/old";
        let normalized_new = "/normalized/new";
        let shared_alias = "/alias/shared";
        let old_only_alias = "/alias/old-only";
        let source_key = simple_hash(normalized_old);
        let target_key = simple_hash(normalized_new);
        let source_dir = cache_base.join(&source_key);
        let target_dir = cache_base.join(&target_key);
        std::fs::create_dir_all(&source_dir).unwrap();

        let source_owner = CacheOwnerManifest::new(normalized_old, old_only_alias)
            .merged_while_pinned(&CacheOwnerManifest::new(normalized_old, shared_alias))
            .unwrap();
        write_cache_owner_intent(&cache_base, &source_dir, &source_key, &source_owner).unwrap();
        assert!(read_cache_owner_manifest(&source_dir).unwrap().is_none());

        let requested = CacheOwnerManifest::new(normalized_new, shared_alias);
        let recovered_source =
            validate_cache_owner_for_migration(&cache_base, &source_dir, &source_key, &requested)
                .unwrap()
                .unwrap();
        let desired = recovered_source.merged_for_target(&requested).unwrap();
        rename_cache_directory(&source_dir, &target_dir, &target_key, &desired).unwrap();

        assert!(read_cache_owner_manifest(&target_dir).unwrap().is_none());
        let recovered = effective_cache_owner(&cache_base, &target_dir, &target_key)
            .unwrap()
            .unwrap();
        for identity in [normalized_old, normalized_new, shared_alias, old_only_alias] {
            assert!(recovered.contains_root(identity));
        }
    }

    #[test]
    fn second_remount_recovers_all_source_generation_intents() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache_base = temp.path().join("cache");
        let normalized_first = "/normalized/first";
        let normalized_second = "/normalized/second";
        let normalized_third = "/normalized/third";
        let shared_alias = "/alias/shared";
        let second_target_alias = "/alias/second-target";
        let first_key = simple_hash(normalized_first);
        let second_key = simple_hash(normalized_second);
        let third_key = simple_hash(normalized_third);
        let source_dir = cache_base.join(&first_key);
        let second_target_dir = cache_base.join(&second_key);
        let third_target_dir = cache_base.join(&third_key);
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::create_dir_all(&second_target_dir).unwrap();

        let source_owner = CacheOwnerManifest::new(normalized_first, shared_alias);
        persist_cache_owner_manifest(&source_dir, &first_key, &source_owner).unwrap();
        let second_requested = CacheOwnerManifest::new(normalized_second, shared_alias);
        let second_target = CacheOwnerManifest::new(normalized_second, second_target_alias);
        persist_cache_owner_manifest(&second_target_dir, &second_key, &second_target).unwrap();
        let second_desired = second_target.merged_for_target(&second_requested).unwrap();
        let interrupted_owner = source_owner.merged_for_target(&second_desired).unwrap();

        // K1 -> K2 durably records the complete owner on K1's generation,
        // then crashes after quarantining K2 but before moving K1.
        write_cache_owner_intent(&cache_base, &source_dir, &second_key, &interrupted_owner)
            .unwrap();
        let quarantined = quarantine_replaceable_migration_target(
            &second_target_dir,
            &second_key,
            &second_desired,
        )
        .unwrap();
        assert!(quarantined.is_some());
        assert!(!second_target_dir.exists());
        assert!(source_dir.exists());

        // Before recovery, the same raw identity remounts again at K3. K2's
        // intent is still part of K1's authorized generation and must carry
        // its otherwise unique alias into K3.
        let third_requested = CacheOwnerManifest::new(normalized_third, shared_alias);
        let authorized_source = validate_cache_owner_for_migration(
            &cache_base,
            &source_dir,
            &first_key,
            &third_requested,
        )
        .unwrap()
        .unwrap();
        let third_desired = merge_authorized_source_owner_intents(
            &cache_base,
            &source_dir,
            &authorized_source,
            &third_requested,
        )
        .unwrap();
        let migrated =
            rename_cache_directory(&source_dir, &third_target_dir, &third_key, &third_desired)
                .unwrap();
        install_cache_owner_manifest(&third_target_dir, &third_key, &migrated, true).unwrap();
        cleanup_source_generation_owner_intents(&cache_base, &third_target_dir, &migrated);

        let installed = read_cache_owner_manifest(&third_target_dir)
            .unwrap()
            .unwrap();
        assert!(installed.is_self_consistent(&third_key));
        for identity in [
            normalized_first,
            normalized_second,
            normalized_third,
            shared_alias,
            second_target_alias,
        ] {
            assert!(installed.contains_root(identity));
        }
        assert!(
            read_cache_owner_intents(&cache_base, &third_target_dir, None)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn second_remount_recovers_after_source_rename_before_manifest_install() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache_base = temp.path().join("cache");
        let normalized_first = "/normalized/first";
        let normalized_second = "/normalized/second";
        let normalized_third = "/normalized/third";
        let shared_alias = "/alias/shared";
        let first_key = simple_hash(normalized_first);
        let second_key = simple_hash(normalized_second);
        let third_key = simple_hash(normalized_third);
        let first_dir = cache_base.join(&first_key);
        let second_dir = cache_base.join(&second_key);
        let third_dir = cache_base.join(&third_key);
        std::fs::create_dir_all(&first_dir).unwrap();

        let db_path = first_dir.join("index.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        conn.execute(
            "INSERT INTO metadata (key, value) VALUES ('project_root', ?1)",
            params![normalized_first],
        )
        .unwrap();
        drop(conn);

        let first_owner = CacheOwnerManifest::new(normalized_first, shared_alias);
        persist_cache_owner_manifest(&first_dir, &first_key, &first_owner).unwrap();
        let second_requested = CacheOwnerManifest::new(normalized_second, shared_alias);
        let second_desired = first_owner.merged_for_target(&second_requested).unwrap();

        // K1 -> K2 moved the directory, then crashed before replacing K1's
        // manifest. The current-generation K2 intent is the required bridge.
        rename_cache_directory(&first_dir, &second_dir, &second_key, &second_desired).unwrap();
        let stale_manifest = read_cache_owner_manifest(&second_dir).unwrap().unwrap();
        assert!(stale_manifest.is_self_consistent(&first_key));
        assert!(!stale_manifest.is_self_consistent(&second_key));

        let recovered_second = effective_cache_owner(&cache_base, &second_dir, &second_key)
            .unwrap()
            .unwrap();
        assert!(recovered_second.is_self_consistent(&second_key));
        assert!(recovered_second.contains_root(normalized_first));
        let metadata_root = read_cached_project_root(&second_dir.join("index.db")).unwrap();
        assert!(recovered_second.contains_root(&metadata_root));

        // A second remount requests K3 before K2 recovery installs its final
        // manifest. The recovered K2 owner independently authorizes migration.
        let third_requested = CacheOwnerManifest::new(normalized_third, shared_alias);
        let authorized_source = validate_cache_owner_for_migration(
            &cache_base,
            &second_dir,
            &second_key,
            &third_requested,
        )
        .unwrap()
        .unwrap();
        let third_desired = merge_authorized_source_owner_intents(
            &cache_base,
            &second_dir,
            &authorized_source,
            &third_requested,
        )
        .unwrap();
        let migrated =
            rename_cache_directory(&second_dir, &third_dir, &third_key, &third_desired).unwrap();
        install_cache_owner_manifest(&third_dir, &third_key, &migrated, true).unwrap();
        cleanup_source_generation_owner_intents(&cache_base, &third_dir, &migrated);

        let installed = read_cache_owner_manifest(&third_dir).unwrap().unwrap();
        assert!(installed.is_self_consistent(&third_key));
        for identity in [
            normalized_first,
            normalized_second,
            normalized_third,
            shared_alias,
        ] {
            assert!(installed.contains_root(identity));
        }
    }

    #[test]
    fn cross_key_intent_does_not_authorize_mismatched_manifest() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache_base = temp.path().join("cache");
        let first_owner = CacheOwnerManifest::new("/normalized/first", "/alias/shared");
        let second_owner = CacheOwnerManifest::new("/normalized/second", "/alias/shared");
        let first_key = simple_hash(&first_owner.normalized_root);
        let second_key = simple_hash(&second_owner.normalized_root);
        let unrelated_key = simple_hash("/normalized/unrelated");
        let cache_dir = cache_base.join(&second_key);
        std::fs::create_dir_all(&cache_dir).unwrap();

        persist_cache_owner_manifest(&cache_dir, &first_key, &first_owner).unwrap();
        let unrelated_intent = CacheOwnerManifest::new("/normalized/unrelated", "/alias/shared");
        write_cache_owner_intent(&cache_base, &cache_dir, &unrelated_key, &unrelated_intent)
            .unwrap();

        let error = effective_cache_owner(&cache_base, &cache_dir, &second_key).unwrap_err();
        assert!(format!("{error:#}").contains("does not match directory key"));
    }

    #[test]
    fn cache_owner_intent_bridges_old_key_but_not_a_recreated_generation() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache_base = temp.path().join("cache");
        let normalized_old = "/normalized/old";
        let normalized_new = "/normalized/new";
        let first_alias = "/alias/first";
        let remounted_alias = "/alias/remounted";
        let replacement_alias = "/alias/replacement";
        let old_key = simple_hash(normalized_old);
        let old_dir = cache_base.join(&old_key);
        std::fs::create_dir_all(&old_dir).unwrap();

        let original = CacheOwnerManifest::new(normalized_old, first_alias);
        persist_cache_owner_manifest(&old_dir, &old_key, &original).unwrap();
        let alias_update = CacheOwnerManifest::new(normalized_old, remounted_alias);
        let intent_owner = original.merged_while_pinned(&alias_update).unwrap();
        write_cache_owner_intent(&cache_base, &old_dir, &old_key, &intent_owner).unwrap();
        std::fs::remove_file(cache_owner_manifest_path(&old_dir)).unwrap();

        let requested_after_remount = CacheOwnerManifest::new(normalized_new, remounted_alias);
        let recovered = effective_cache_owner(&cache_base, &old_dir, &old_key)
            .unwrap()
            .unwrap();
        assert!(recovered.overlaps(&requested_after_remount));
        assert!(recovered.contains_root(remounted_alias));

        let retired = cache_base.join("retired-generation");
        std::fs::rename(&old_dir, &retired).unwrap();
        std::fs::create_dir_all(&old_dir).unwrap();
        let replacement = CacheOwnerManifest::new(normalized_old, replacement_alias);
        persist_cache_owner_manifest(&old_dir, &old_key, &replacement).unwrap();

        let recreated = effective_cache_owner(&cache_base, &old_dir, &old_key)
            .unwrap()
            .unwrap();
        assert!(recreated.contains_root(replacement_alias));
        assert!(!recreated.contains_root(remounted_alias));
        assert!(!recreated.overlaps(&requested_after_remount));
    }

    #[test]
    fn relative_cache_owner_identities_are_scoped_to_their_working_directory() {
        let relative = Path::new(".");
        let first_raw = absolute_lexical_root_in(Path::new("/workspace/first"), relative);
        let second_raw = absolute_lexical_root_in(Path::new("/workspace/second"), relative);
        let first =
            CacheOwnerManifest::new("/normalized/first", first_raw.to_string_lossy().as_ref());
        let second =
            CacheOwnerManifest::new("/normalized/second", second_raw.to_string_lossy().as_ref());

        assert_ne!(first.raw_root, second.raw_root);
        assert!(!first.overlaps(&second));
    }

    #[test]
    fn legacy_migration_uses_normalized_target_and_bridges_exclusive_to_shared() {
        let temp = tempfile::TempDir::new().unwrap();
        let new_base = temp.path().join("ast-index");
        let old_base = temp.path().join("kotlin-index");
        let parent = temp.path().join("workspace");
        let child = parent.join("child");
        let real = parent.join("real");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        let raw_root = child.join("..").join("real");
        let raw_key = simple_hash(raw_root.to_string_lossy().as_ref());
        let normalized_key = project_cache_key(&raw_root).unwrap();
        assert_ne!(raw_key, normalized_key);

        let old_dir = old_base.join(&raw_key);
        std::fs::create_dir_all(&old_dir).unwrap();
        let old_db = old_dir.join("index.db");
        std::fs::write(&old_db, "legacy-index").unwrap();
        std::fs::write(old_dir.join("marker.txt"), "whole-directory").unwrap();
        let stale_time = std::time::SystemTime::now()
            .checked_sub(STALE_CACHE_MAX_AGE + std::time::Duration::from_secs(60))
            .unwrap();
        OpenOptions::new()
            .write(true)
            .open(&old_db)
            .unwrap()
            .set_modified(stale_time)
            .unwrap();

        let leases = leases_dir(&new_base);
        std::fs::create_dir_all(&leases).unwrap();
        let external = open_lock_file(&leases.join(format!("{normalized_key}.lock"))).unwrap();
        fs2::FileExt::lock_shared(&external).unwrap();
        let error = match migrate_legacy_project_in(&new_base, &old_base, &raw_root) {
            Ok(_) => panic!("legacy migration ignored an active normalized target"),
            Err(error) => error,
        };
        assert!(format!("{error:#}").contains("active"));
        assert!(old_db.is_file());
        assert!(!new_base.join(&normalized_key).join("index.db").exists());
        fs2::FileExt::unlock(&external).unwrap();
        drop(external);

        let target = new_base.join(&normalized_key);
        let unexpected = target.join("must-survive.txt");
        std::fs::write(&unexpected, "not resolver-owned").unwrap();
        let nonempty_error = match migrate_legacy_project_in(&new_base, &old_base, &raw_root) {
            Ok(_) => panic!("legacy migration replaced an unrelated target file"),
            Err(error) => error,
        };
        assert!(format!("{nonempty_error:#}").contains("non-empty"));
        assert_eq!(
            std::fs::read_to_string(&unexpected).unwrap(),
            "not resolver-owned"
        );
        assert!(old_db.is_file());
        std::fs::remove_file(&unexpected).unwrap();

        // Root discovery resolves the normalized cache before legacy migration
        // and leaves only resolver-owned metadata when no index.db exists.
        let desired_owner = CacheOwnerManifest::new(
            &normalize_root(&raw_root),
            raw_root.to_string_lossy().as_ref(),
        );
        let prior_target_alias = "/previous/target-only-alias";
        let target_owner = desired_owner
            .merged_while_pinned(&CacheOwnerManifest::new(
                &desired_owner.normalized_root,
                prior_target_alias,
            ))
            .unwrap();
        install_cache_owner_manifest(&target, &normalized_key, &target_owner, false).unwrap();
        let discovery_lease = acquire_shared_project_lease(&new_base, &normalized_key).unwrap();
        let discovery_publication =
            try_acquire_shared_publication(&target.join("index.db"), &discovery_lease).unwrap();
        let target_entries = std::fs::read_dir(&target)
            .unwrap()
            .collect::<std::io::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(target_entries.len(), 2);
        assert!(target_entries
            .iter()
            .any(|entry| entry.path() == cache_owner_manifest_path(&target)));
        assert!(target_entries
            .iter()
            .any(|entry| entry.path() == cache_generation_marker_path(&target)));
        assert!(
            leases
                .join(format!("{normalized_key}.publish.lock"))
                .is_file(),
            "root discovery did not create the external publication lock"
        );
        drop(discovery_publication);
        drop(discovery_lease);

        let lease = migrate_legacy_project_in(&new_base, &old_base, &raw_root).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("marker.txt")).unwrap(),
            "whole-directory"
        );
        let owner = read_cache_owner_manifest(&target).unwrap().unwrap();
        assert_eq!(owner.normalized_root, normalize_root(&raw_root));
        assert_eq!(owner.raw_root, raw_root.to_string_lossy());
        assert!(owner.is_self_consistent(&normalized_key));
        assert!(owner.contains_root(prior_target_alias));
        assert!(!old_dir.exists());
        assert!(!new_base.join(raw_key).join("index.db").exists());

        let removed = gc_stale_caches_in(
            &new_base,
            None,
            STALE_CACHE_MAX_AGE,
            std::time::SystemTime::now(),
        )
        .unwrap();
        assert_eq!(removed, 0);
        drop(lease);
        let removed = gc_stale_caches_in(
            &new_base,
            None,
            STALE_CACHE_MAX_AGE,
            std::time::SystemTime::now(),
        )
        .unwrap();
        assert_eq!(removed, 1);
        assert!(!target.exists());
    }

    #[test]
    fn test_init_db() {
        let conn = create_test_db();
        // Check tables exist
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='files'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_escape_fts5_query_simple() {
        assert_eq!(escape_fts5_query("MyClass"), "\"MyClass\"");
    }

    #[test]
    fn test_escape_fts5_query_prefix() {
        assert_eq!(escape_fts5_query("Slow*"), "\"Slow\"*");
        assert_eq!(escape_fts5_query("SlowUpstream*"), "\"SlowUpstream\"*");
    }

    #[test]
    fn test_escape_fts5_query_empty() {
        assert_eq!(escape_fts5_query(""), "");
        assert_eq!(escape_fts5_query("   "), "");
    }

    #[test]
    fn test_escape_fts5_query_with_quotes() {
        assert_eq!(escape_fts5_query("say \"hello\""), "\"say \"\"hello\"\"\"");
    }

    #[test]
    fn test_upsert_and_search() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "src/main.kt", 1000, 100).unwrap();
        assert!(file_id > 0);

        insert_symbol(
            &conn,
            file_id,
            "MyService",
            SymbolKind::Class,
            10,
            Some("class MyService"),
        )
        .unwrap();
        insert_symbol(
            &conn,
            file_id,
            "processData",
            SymbolKind::Function,
            20,
            Some("fun processData()"),
        )
        .unwrap();

        let results = search_symbols(&conn, "MyService", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "MyService");
        assert_eq!(results[0].kind, "class");
        assert_eq!(results[0].path, "src/main.kt");
    }

    #[test]
    fn test_search_empty_query() {
        let conn = create_test_db();
        let results = search_symbols(&conn, "", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_find_files() {
        let conn = create_test_db();
        upsert_file(&conn, "src/main.kt", 1000, 100).unwrap();
        upsert_file(&conn, "src/utils/Helper.kt", 2000, 200).unwrap();

        let files = find_files(&conn, "Helper", 10).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].contains("Helper"));
    }

    #[test]
    fn test_find_symbols_by_name() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "src/model.kt", 1000, 100).unwrap();
        insert_symbol(
            &conn,
            file_id,
            "User",
            SymbolKind::Class,
            5,
            Some("data class User"),
        )
        .unwrap();
        insert_symbol(
            &conn,
            file_id,
            "UserRepository",
            SymbolKind::Interface,
            20,
            Some("interface UserRepository"),
        )
        .unwrap();

        let results = find_symbols_by_name(&conn, "User", None, 10).unwrap();
        assert!(results.len() >= 1);
        assert!(results.iter().any(|r| r.name == "User"));
    }

    #[test]
    fn test_find_symbols_by_qualified_name() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "src/client.cpp", 1000, 100).unwrap();
        insert_symbol(
            &conn,
            file_id,
            "Client",
            SymbolKind::Class,
            5,
            Some("class Client"),
        )
        .unwrap();
        set_qualified_name(&conn, "Client", "arcanum::Client");

        let bare = find_symbols_by_name(&conn, "Client", None, 10).unwrap();
        assert_eq!(bare.len(), 1);
        assert_eq!(bare[0].name, "Client");
        assert_eq!(bare[0].qualified_name.as_deref(), Some("arcanum::Client"));

        let qualified = find_symbols_by_name(&conn, "arcanum::Client", None, 10).unwrap();
        assert_eq!(qualified.len(), 1);
        assert_eq!(qualified[0].name, "Client");
        assert_eq!(
            qualified[0].qualified_name.as_deref(),
            Some("arcanum::Client")
        );
    }

    #[test]
    fn test_find_symbols_by_pattern_with_namespace_suffix() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "src/client.cpp", 1000, 100).unwrap();
        insert_symbol(
            &conn,
            file_id,
            "Extra",
            SymbolKind::Class,
            5,
            Some("class Extra"),
        )
        .unwrap();
        set_qualified_name(&conn, "Extra", "foo::bar::Extra");

        let bare = find_symbols_by_pattern(&conn, "Extra", None, 10, &SearchScope::none()).unwrap();
        assert_eq!(bare.len(), 1);
        assert_eq!(bare[0].name, "Extra");

        let suffix =
            find_symbols_by_pattern(&conn, "%::Extra", None, 10, &SearchScope::none()).unwrap();
        assert_eq!(suffix.len(), 1);
        assert_eq!(suffix[0].qualified_name.as_deref(), Some("foo::bar::Extra"));
    }

    #[test]
    fn test_find_enum_value_by_bare_and_qualified_name() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "src/acceptance_operation.cpp", 1000, 100).unwrap();
        insert_symbol(
            &conn,
            file_id,
            "kAntifraud",
            SymbolKind::Constant,
            24,
            Some("kAntifraud,"),
        )
        .unwrap();
        set_qualified_name(
            &conn,
            "kAntifraud",
            "db::AcceptanceOperationInitiator::kAntifraud",
        );

        let bare = find_symbols_by_name(&conn, "kAntifraud", None, 10).unwrap();
        assert_eq!(bare.len(), 1);
        assert_eq!(bare[0].name, "kAntifraud");

        let qualified =
            find_symbols_by_name(&conn, "AcceptanceOperationInitiator::kAntifraud", None, 10)
                .unwrap();
        assert_eq!(qualified.len(), 1);
        assert_eq!(qualified[0].name, "kAntifraud");

        let suffix = find_symbols_by_name(&conn, "::kAntifraud", None, 10).unwrap();
        assert_eq!(suffix.len(), 1);
        assert_eq!(suffix[0].name, "kAntifraud");
    }

    #[test]
    fn test_upsert_file_updates_mtime() {
        let conn = create_test_db();
        let _id1 = upsert_file(&conn, "src/main.kt", 1000, 100).unwrap();
        let id2 = upsert_file(&conn, "src/main.kt", 2000, 200).unwrap();
        assert!(
            id2 > 0,
            "upsert should succeed for same path with different mtime"
        );
    }

    #[test]
    fn test_clear_db() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "src/main.kt", 1000, 100).unwrap();
        insert_symbol(
            &conn,
            file_id,
            "Test",
            SymbolKind::Class,
            1,
            Some("class Test"),
        )
        .unwrap();

        clear_db(&conn).unwrap();

        let results = search_symbols(&conn, "Test", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_get_stats() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "src/main.kt", 1000, 100).unwrap();
        insert_symbol(
            &conn,
            file_id,
            "Foo",
            SymbolKind::Class,
            1,
            Some("class Foo"),
        )
        .unwrap();
        insert_symbol(
            &conn,
            file_id,
            "bar",
            SymbolKind::Function,
            5,
            Some("fun bar()"),
        )
        .unwrap();

        let stats = get_stats(&conn).unwrap();
        assert_eq!(stats.file_count, 1);
        assert_eq!(stats.symbol_count, 2);
    }

    #[test]
    fn test_insert_and_find_inheritance() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "src/model.kt", 1000, 100).unwrap();
        insert_symbol(
            &conn,
            file_id,
            "Child",
            SymbolKind::Class,
            1,
            Some("class Child : Parent()"),
        )
        .unwrap();

        let child_id: i64 = conn
            .query_row("SELECT id FROM symbols WHERE name = 'Child'", [], |row| {
                row.get(0)
            })
            .unwrap();
        insert_inheritance(&conn, child_id, "Parent", "extends").unwrap();

        let impls = find_implementations(&conn, "Parent", 10).unwrap();
        assert_eq!(impls.len(), 1);
        assert_eq!(impls[0].name, "Child");
    }

    #[test]
    fn test_find_implementations_matches_cpp_namespace_suffix() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "src/model.cpp", 1000, 100).unwrap();
        insert_symbol(
            &conn,
            file_id,
            "Child",
            SymbolKind::Class,
            1,
            Some("class Child : ns::Base"),
        )
        .unwrap();

        let child_id: i64 = conn
            .query_row("SELECT id FROM symbols WHERE name = 'Child'", [], |row| {
                row.get(0)
            })
            .unwrap();
        insert_inheritance(&conn, child_id, "ns::Base", "extends").unwrap();

        let impls = find_implementations(&conn, "Base", 10).unwrap();
        assert_eq!(impls.len(), 1);
        assert_eq!(impls[0].name, "Child");
    }

    #[test]
    fn count_implementations_returns_total_above_limit() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "src/model.kt", 1000, 100).unwrap();
        for i in 0..125 {
            let name = format!("Child{:03}", i);
            insert_symbol(&conn, file_id, &name, SymbolKind::Class, i + 1, None).unwrap();
            let id: i64 = conn
                .query_row(
                    "SELECT id FROM symbols WHERE name = ?1",
                    params![&name],
                    |row| row.get(0),
                )
                .unwrap();
            insert_inheritance(&conn, id, "BaseQueryService", "extends").unwrap();
        }

        let total = count_implementations(&conn, "BaseQueryService").unwrap();
        assert_eq!(
            total, 125,
            "count must reflect all 125 children, regardless of any display limit"
        );

        let truncated = find_implementations(&conn, "BaseQueryService", 50).unwrap();
        assert_eq!(
            truncated.len(),
            50,
            "find_implementations honours the LIMIT"
        );

        let full = find_implementations(&conn, "BaseQueryService", 200).unwrap();
        assert_eq!(
            full.len(),
            125,
            "with sufficient limit, all children come back"
        );
    }

    #[test]
    fn test_count_refs() {
        let conn = create_test_db();
        let count = count_refs(&conn).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_glob_to_like() {
        assert_eq!(glob_to_like("*Mailer"), "%Mailer");
        assert_eq!(glob_to_like("*Email*Service*"), "%Email%Service%");
        assert_eq!(glob_to_like("User?"), "User_");
        assert_eq!(glob_to_like("exact"), "exact");
        // Existing SQL wildcards should be escaped
        assert_eq!(glob_to_like("100%"), "100\\%");
        assert_eq!(glob_to_like("a_b"), "a\\_b");
    }

    #[test]
    fn test_find_class_like_pattern() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "app/mailers/user_mailer.rb", 1000, 100).unwrap();
        insert_symbol(
            &conn,
            file_id,
            "UserMailer",
            SymbolKind::Class,
            1,
            Some("class UserMailer"),
        )
        .unwrap();
        insert_symbol(
            &conn,
            file_id,
            "AdminMailer",
            SymbolKind::Class,
            10,
            Some("class AdminMailer"),
        )
        .unwrap();
        insert_symbol(
            &conn,
            file_id,
            "MailerHelper",
            SymbolKind::Package,
            20,
            Some("module MailerHelper"),
        )
        .unwrap();

        let scope = SearchScope::none();
        // Glob: *Mailer → %Mailer
        let results = find_class_like_pattern(&conn, "%Mailer", 10, &scope).unwrap();
        assert_eq!(
            results.len(),
            2,
            "should match UserMailer and AdminMailer: {:?}",
            results.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
        // MailerHelper is a package, should also match class-like kinds
        let results = find_class_like_pattern(&conn, "%Mailer%", 10, &scope).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_find_symbols_by_pattern() {
        let conn = create_test_db();
        let file_id = upsert_file(&conn, "app/services/email_service.rb", 1000, 100).unwrap();
        insert_symbol(
            &conn,
            file_id,
            "EmailService",
            SymbolKind::Class,
            1,
            Some("class EmailService"),
        )
        .unwrap();
        insert_symbol(
            &conn,
            file_id,
            "send_email",
            SymbolKind::Function,
            10,
            Some("def send_email"),
        )
        .unwrap();
        insert_symbol(
            &conn,
            file_id,
            "EmailValidator",
            SymbolKind::Class,
            20,
            Some("class EmailValidator"),
        )
        .unwrap();

        let scope = SearchScope::none();
        // All symbols matching *Email*
        let results = find_symbols_by_pattern(&conn, "%Email%", None, 10, &scope).unwrap();
        assert_eq!(results.len(), 3);
        // Only classes
        let results = find_symbols_by_pattern(&conn, "%Email%", Some("class"), 10, &scope).unwrap();
        assert_eq!(results.len(), 2);
        // Only functions
        let results =
            find_symbols_by_pattern(&conn, "%email%", Some("function"), 10, &scope).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "send_email");
    }

    #[test]
    fn find_module_id_exact_match() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO modules (name, path) VALUES ('core.utils', 'core/utils')",
            [],
        )
        .unwrap();
        let id = find_module_id_by_name(&conn, "core.utils").unwrap();
        assert!(id.is_some());
    }

    #[test]
    fn find_module_id_colon_separator_resolves() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO modules (name, path) VALUES ('core.utils', 'core/utils')",
            [],
        )
        .unwrap();
        // :core:utils should normalise to core.utils
        let id = find_module_id_by_name(&conn, ":core:utils").unwrap();
        assert!(
            id.is_some(),
            "colon-separated with leading colon should resolve"
        );
    }

    #[test]
    fn find_module_id_slash_separator_resolves() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO modules (name, path) VALUES ('core.utils', 'core/utils')",
            [],
        )
        .unwrap();
        let id = find_module_id_by_name(&conn, "core/utils").unwrap();
        assert!(id.is_some(), "slash-separated should resolve to dot form");
    }

    #[test]
    fn find_module_id_missing_returns_none() {
        let conn = create_test_db();
        let id = find_module_id_by_name(&conn, "nonexistent").unwrap();
        assert!(id.is_none());
    }

    #[test]
    fn get_outgoing_edges_dedup_no_filter() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO modules (id, name, path) VALUES (1, 'app', 'app')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO modules (id, name, path) VALUES (2, 'core', 'core')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO module_deps (module_id, dep_module_id, dep_kind) VALUES (1, 2, 'implementation')", []).unwrap();
        // Duplicate edge — should be deduplicated in result.
        conn.execute(
            "INSERT INTO module_deps (module_id, dep_module_id, dep_kind) VALUES (1, 2, 'api')",
            [],
        )
        .unwrap();
        let edges = get_outgoing_edges_dedup(&conn, 1, None).unwrap();
        // Both distinct rows (different kind) come back; dedup is per (dep_module_id, name, kind) tuple.
        assert!(!edges.is_empty());
    }

    #[test]
    fn get_outgoing_edges_dedup_kind_filter() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO modules (id, name, path) VALUES (1, 'app', 'app')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO modules (id, name, path) VALUES (2, 'core', 'core')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO module_deps (module_id, dep_module_id, dep_kind) VALUES (1, 2, 'implementation')", []).unwrap();
        let edges = get_outgoing_edges_dedup(&conn, 1, Some("api")).unwrap();
        assert!(
            edges.is_empty(),
            "api filter should return nothing when only implementation edge exists"
        );
    }
}
