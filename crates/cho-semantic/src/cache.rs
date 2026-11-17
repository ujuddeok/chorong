//! The on-disk index cache: `<cache>/chorong/indexes/<repo-id>/<project>/{index.scip,meta.json}`.

use crate::SemanticError;
use crate::indexer::Language;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const INDEX_FILE: &str = "index.scip";
pub const META_FILE: &str = "meta.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IndexStatus {
    /// No index yet.
    None,
    /// `HEAD` and the working-tree hash match.
    Current,
    /// They don't. The index is still used; results may be out of date.
    Stale,
    /// The last run failed; see the stored stderr tail.
    Failed,
    /// Built after a prepare step that failed (remote worktrees): the index is used, but
    /// parts that needed the prepared files may be missing. `meta.prepare.log` has the log.
    Partial,
}

impl IndexStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            IndexStatus::None => "none",
            IndexStatus::Current => "current",
            IndexStatus::Stale => "stale",
            IndexStatus::Failed => "failed",
            IndexStatus::Partial => "partial",
        }
    }
}

impl fmt::Display for IndexStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What `meta.json` records about one index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "MetaRecord", try_from = "MetaRecord")]
pub struct Meta {
    pub indexer: String,
    pub indexer_version: Option<String>,
    pub language: Language,
    /// The project root relative to the repository root; empty for the root itself.
    pub project_root: String,
    pub git_head: Option<String>,
    /// Hash of `git status --porcelain`.
    pub git_status_hash: Option<String>,
    /// Seconds since the Unix epoch.
    pub created: u64,
    pub result: BuildResult,
    pub duration_ms: u64,
    pub stderr_tail: String,
    /// The prepare run before this index, when it failed (remote worktrees only).
    pub prepare: Option<PrepareNote>,
}

impl Meta {
    /// The build wrote an index.
    pub fn success(&self) -> bool {
        self.result == BuildResult::Built
    }

    pub fn timed_out(&self) -> bool {
        self.result == BuildResult::Failed(Failure::TimedOut)
    }
}

/// How a recorded build ended. (A cancelled build records nothing.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildResult {
    /// The indexer exited 0 and wrote its index.
    Built,
    Failed(Failure),
}

/// Why a recorded build failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The indexer exited with a failure status. `code` is `None` when it could not start
    /// or a signal ended it (`stderr_tail` says which).
    Exited { code: Option<i32> },
    /// Stopped at the timeout.
    TimedOut,
    /// Exited 0 but wrote no index.
    NoOutput,
}

/// `meta.json` as written: the build result spelled as `success`, `exit_code` and
/// `timed_out`, which earlier versions of Chorong wrote and read.
#[derive(Serialize, Deserialize)]
struct MetaRecord {
    indexer: String,
    indexer_version: Option<String>,
    language: Language,
    project_root: String,
    git_head: Option<String>,
    git_status_hash: Option<String>,
    created: u64,
    success: bool,
    exit_code: Option<i32>,
    timed_out: bool,
    duration_ms: u64,
    stderr_tail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prepare: Option<PrepareNote>,
}

impl From<Meta> for MetaRecord {
    fn from(m: Meta) -> Self {
        let (success, exit_code, timed_out) = match m.result {
            BuildResult::Built => (true, Some(0), false),
            BuildResult::Failed(Failure::Exited { code }) => (false, code, false),
            BuildResult::Failed(Failure::TimedOut) => (false, None, true),
            BuildResult::Failed(Failure::NoOutput) => (false, Some(0), false),
        };
        Self {
            indexer: m.indexer,
            indexer_version: m.indexer_version,
            language: m.language,
            project_root: m.project_root,
            git_head: m.git_head,
            git_status_hash: m.git_status_hash,
            created: m.created,
            success,
            exit_code,
            timed_out,
            duration_ms: m.duration_ms,
            stderr_tail: m.stderr_tail,
            prepare: m.prepare,
        }
    }
}

impl From<MetaRecord> for Meta {
    fn from(r: MetaRecord) -> Self {
        let result = match (r.success, r.timed_out, r.exit_code) {
            (true, _, _) => BuildResult::Built,
            (false, true, _) => BuildResult::Failed(Failure::TimedOut),
            // A failure with status 0 is the only way to write `false` with code 0.
            (false, false, Some(0)) => BuildResult::Failed(Failure::NoOutput),
            (false, false, code) => BuildResult::Failed(Failure::Exited { code }),
        };
        Self {
            indexer: r.indexer,
            indexer_version: r.indexer_version,
            language: r.language,
            project_root: r.project_root,
            git_head: r.git_head,
            git_status_hash: r.git_status_hash,
            created: r.created,
            result,
            duration_ms: r.duration_ms,
            stderr_tail: r.stderr_tail,
            prepare: r.prepare,
        }
    }
}

/// A failed prepare that an index was built after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepareNote {
    pub result: PrepareFailure,
    /// The prepare log.
    pub log: String,
}

/// How the prepare before an index failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrepareFailure {
    /// A step exited with a failure status (or prepare could not run at all).
    Failed,
    TimedOut,
    /// A step's program is not installed.
    MissingTool,
}

impl PrepareFailure {
    pub fn as_str(self) -> &'static str {
        match self {
            PrepareFailure::Failed => "failed",
            PrepareFailure::TimedOut => "timed-out",
            PrepareFailure::MissingTool => "missing-tool",
        }
    }
}

impl fmt::Display for PrepareFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The repository state an index is compared against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitState {
    pub head: Option<String>,
    pub status_hash: Option<String>,
}

impl GitState {
    pub fn of(root: &Path) -> Self {
        let git = |args: &[&str]| -> Option<Vec<u8>> {
            // Read-only and safe in untrusted folders: no index rewrite, no programs from the
            // repository's config (see cho_core::git).
            let out = cho_core::git::command(root).args(args).output().ok()?;
            out.status.success().then_some(out.stdout)
        };
        Self {
            head: git(&["rev-parse", "HEAD"]).map(|o| String::from_utf8_lossy(&o).trim().into()),
            status_hash: git(&["status", "--porcelain", "--ignore-submodules=all"])
                .map(|o| hex(fnv1a(&o))),
        }
    }
}

/// Stable 64-bit FNV-1a. Not cryptographic; it only names directories and
/// detects change.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x100000001b3)
    })
}

fn hex(v: u64) -> String {
    format!("{v:016x}")
}

/// A stable identifier for a repository: a hash of its canonical path.
pub fn repo_id(canonical_root: &Path) -> String {
    hex(fnv1a(canonical_root.to_string_lossy().as_bytes()))
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// One cached project index.
#[derive(Debug, Clone)]
pub struct ProjectEntry {
    pub dir: PathBuf,
    pub meta: Meta,
    pub has_index: bool,
    pub status: IndexStatus,
}

impl ProjectEntry {
    pub fn index_path(&self) -> PathBuf {
        self.dir.join(INDEX_FILE)
    }
}

/// Status of a recorded run against the repository as it is now.
pub fn status_of(meta: &Meta, now: &GitState) -> IndexStatus {
    if !meta.success() {
        IndexStatus::Failed
    } else if now.head.is_some()
        && now.status_hash.is_some()
        && meta.git_head == now.head
        && meta.git_status_hash == now.status_hash
    {
        if meta.prepare.is_some() {
            IndexStatus::Partial
        } else {
            IndexStatus::Current
        }
    } else {
        // Different state, or no Git to compare against: can't claim `current`.
        IndexStatus::Stale
    }
}

/// The worst status among projects: failed, then partial, then stale, then current.
pub fn overall_status(projects: &[ProjectEntry]) -> IndexStatus {
    [
        IndexStatus::Failed,
        IndexStatus::Partial,
        IndexStatus::Stale,
        IndexStatus::Current,
    ]
    .into_iter()
    .find(|s| projects.iter().any(|p| p.status == *s))
    .unwrap_or(IndexStatus::None)
}

#[derive(Debug, Clone)]
pub struct IndexCache {
    base: PathBuf,
}

impl IndexCache {
    /// `$XDG_CACHE_HOME/chorong/indexes` (macOS: `~/Library/Caches/chorong/indexes`).
    /// `CHORONG_CACHE_DIR`, if set, names the `indexes` directory itself (tests, CI).
    pub fn default_location() -> Option<Self> {
        if let Some(dir) = std::env::var_os("CHORONG_CACHE_DIR").filter(|v| !v.is_empty()) {
            return Some(Self::at(dir));
        }
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from);
        let cache = if cfg!(target_os = "macos") {
            home?.join("Library/Caches")
        } else {
            match std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
                Some(x) => PathBuf::from(x),
                None => home?.join(".cache"),
            }
        };
        Some(Self::at(cache.join("chorong").join("indexes")))
    }

    /// A cache rooted at an explicit directory (tests, custom setups).
    pub fn at(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    /// Removes every index recorded for the repository at `canonical_root` (a worktree that
    /// was deleted). Returns whether there was one. Only ever removes inside the cache.
    pub fn remove_repo(&self, canonical_root: &Path) -> bool {
        let dir = self.repo_dir(canonical_root);
        dir.starts_with(&self.base) && dir.is_dir() && std::fs::remove_dir_all(&dir).is_ok()
    }

    pub fn repo_dir(&self, canonical_root: &Path) -> PathBuf {
        self.base.join(repo_id(canonical_root))
    }

    /// Every project index recorded for the repository, by directory name.
    #[allow(clippy::disallowed_methods)] // Chorong's own index cache
    pub fn projects(&self, canonical_root: &Path) -> Vec<ProjectEntry> {
        let now = GitState::of(canonical_root);
        let mut out = vec![];
        let Ok(rd) = std::fs::read_dir(self.repo_dir(canonical_root)) else {
            return out;
        };
        for entry in rd.flatten() {
            let dir = entry.path();
            let Ok(text) = std::fs::read_to_string(dir.join(META_FILE)) else {
                continue;
            };
            let Ok(meta) = serde_json::from_str::<Meta>(&text) else {
                continue;
            };
            let status = status_of(&meta, &now);
            out.push(ProjectEntry {
                has_index: dir.join(INDEX_FILE).is_file(),
                dir,
                meta,
                status,
            });
        }
        out.sort_by(|a, b| a.dir.cmp(&b.dir));
        out
    }

    pub fn write_meta(dir: &Path, meta: &Meta) -> Result<(), SemanticError> {
        let text = serde_json::to_string_pretty(meta).expect("meta serialises");
        let path = dir.join(META_FILE);
        // A temporary name of its own: with the shared `meta.json.tmp`, two writers truncated
        // each other's file before the rename and the loser's rename failed. The rename
        // itself is atomic, so a concurrent `projects()` reads the old or the new record,
        // never half of one.
        static WRITE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let tmp = dir.join(format!(
            "meta.json.{}.{}.tmp",
            std::process::id(),
            WRITE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&tmp, text)
            .and_then(|_| std::fs::rename(&tmp, &path))
            .map_err(|e| {
                std::fs::remove_file(&tmp).ok();
                SemanticError::Io { path, source: e }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(success: bool, head: Option<&str>, hash: Option<&str>) -> Meta {
        Meta {
            indexer: "rust-analyzer".into(),
            indexer_version: None,
            language: Language::Rust,
            project_root: String::new(),
            git_head: head.map(Into::into),
            git_status_hash: hash.map(Into::into),
            created: 1,
            result: if success {
                BuildResult::Built
            } else {
                BuildResult::Failed(Failure::Exited { code: Some(1) })
            },
            duration_ms: 1,
            stderr_tail: String::new(),
            prepare: None,
        }
    }

    #[test]
    fn partial_after_a_failed_prepare() {
        let now = GitState {
            head: Some("abc".into()),
            status_hash: Some("h1".into()),
        };
        let mut m = meta(true, Some("abc"), Some("h1"));
        m.prepare = Some(PrepareNote {
            result: PrepareFailure::Failed,
            log: "/cache/prepare/pr-1.log".into(),
        });
        assert_eq!(status_of(&m, &now), IndexStatus::Partial);
        // Out of date is still stale; a failed index is still failed.
        assert_eq!(
            status_of(&meta(true, Some("old"), Some("h1")), &now),
            IndexStatus::Stale
        );
        m.result = BuildResult::Failed(Failure::TimedOut);
        assert_eq!(status_of(&m, &now), IndexStatus::Failed);
        let json = serde_json::to_string(&meta(true, None, None)).unwrap();
        assert!(!json.contains("prepare"), "{json}");
    }

    #[test]
    fn status_rules() {
        let now = GitState {
            head: Some("abc".into()),
            status_hash: Some("h1".into()),
        };
        assert_eq!(
            status_of(&meta(true, Some("abc"), Some("h1")), &now),
            IndexStatus::Current
        );
        assert_eq!(
            status_of(&meta(true, Some("abc"), Some("h2")), &now),
            IndexStatus::Stale
        );
        assert_eq!(
            status_of(&meta(true, Some("zzz"), Some("h1")), &now),
            IndexStatus::Stale
        );
        assert_eq!(
            status_of(&meta(false, Some("abc"), Some("h1")), &now),
            IndexStatus::Failed
        );
        let no_git = GitState {
            head: None,
            status_hash: None,
        };
        assert_eq!(
            status_of(&meta(true, None, None), &no_git),
            IndexStatus::Stale
        );
    }

    /// Cache directories are named by this hash: a change orphans every existing cache.
    #[test]
    fn repo_id_is_stable() {
        assert_eq!(repo_id(Path::new("/a/b")), "363e289cb38ee0cc");
    }

    /// `meta.json` keeps the fields earlier versions wrote, and each build result reads
    /// back as itself.
    #[test]
    fn meta_json_keeps_its_shape() {
        let mut m = meta(true, Some("abc"), Some("h1"));
        m.prepare = Some(PrepareNote {
            result: PrepareFailure::MissingTool,
            log: "/l".into(),
        });
        assert_eq!(
            serde_json::to_string(&m).unwrap(),
            r#"{"indexer":"rust-analyzer","indexer_version":null,"language":"rust","project_root":"","git_head":"abc","git_status_hash":"h1","created":1,"success":true,"exit_code":0,"timed_out":false,"duration_ms":1,"stderr_tail":"","prepare":{"result":"missing-tool","log":"/l"}}"#
        );
        for result in [
            BuildResult::Built,
            BuildResult::Failed(Failure::Exited { code: Some(3) }),
            BuildResult::Failed(Failure::Exited { code: None }),
            BuildResult::Failed(Failure::TimedOut),
            BuildResult::Failed(Failure::NoOutput),
        ] {
            let m = Meta {
                result,
                ..meta(true, None, None)
            };
            let back: Meta = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
            assert_eq!(back, m);
        }
        // As an earlier Chorong wrote a timeout (no `prepare` field).
        let old = r#"{"indexer":"scip-typescript","indexer_version":null,"language":"typescript","project_root":"web","git_head":null,"git_status_hash":null,"created":5,"success":false,"exit_code":null,"timed_out":true,"duration_ms":9,"stderr_tail":"x"}"#;
        let m: Meta = serde_json::from_str(old).unwrap();
        assert_eq!(
            (m.language, m.result, m.prepare),
            (
                Language::TypeScript,
                BuildResult::Failed(Failure::TimedOut),
                None
            )
        );
    }

    #[test]
    fn overall_is_the_worst() {
        let e = |status| ProjectEntry {
            dir: PathBuf::new(),
            meta: meta(true, None, None),
            has_index: true,
            status,
        };
        assert_eq!(overall_status(&[]), IndexStatus::None);
        assert_eq!(
            overall_status(&[e(IndexStatus::Current)]),
            IndexStatus::Current
        );
        assert_eq!(
            overall_status(&[e(IndexStatus::Current), e(IndexStatus::Stale)]),
            IndexStatus::Stale
        );
        assert_eq!(
            overall_status(&[e(IndexStatus::Stale), e(IndexStatus::Failed)]),
            IndexStatus::Failed
        );
        // Failed outranks Partial, and Partial outranks Stale, in either order.
        for (a, b, worst) in [
            (
                IndexStatus::Partial,
                IndexStatus::Failed,
                IndexStatus::Failed,
            ),
            (
                IndexStatus::Stale,
                IndexStatus::Partial,
                IndexStatus::Partial,
            ),
            (
                IndexStatus::Current,
                IndexStatus::Partial,
                IndexStatus::Partial,
            ),
        ] {
            assert_eq!(overall_status(&[e(a), e(b)]), worst);
            assert_eq!(overall_status(&[e(b), e(a)]), worst);
        }
    }

    #[test]
    fn lists_written_projects() {
        let base = std::env::temp_dir().join(format!("cho-cache-{}", std::process::id()));
        let cache = IndexCache::at(&base);
        let root = Path::new("/nonexistent/repo");
        let dir = cache.repo_dir(root).join("rust");
        std::fs::create_dir_all(&dir).unwrap();
        IndexCache::write_meta(&dir, &meta(false, None, None)).unwrap();
        let projects = cache.projects(root);
        assert_eq!(projects.len(), 1);
        assert!(!projects[0].has_index);
        assert_eq!(projects[0].status, IndexStatus::Failed);
        std::fs::remove_dir_all(base).ok();
    }

    /// A reader loading the cache while writers replace `meta.json` (several of them, as when
    /// two Chorong processes finish the same project together) always sees a complete record,
    /// and no writer fails.
    #[test]
    #[allow(clippy::disallowed_methods)] // reads the cache's own file
    fn readers_never_see_a_half_written_record() {
        let base = std::env::temp_dir().join(format!("cho-meta-race-{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        let cache = IndexCache::at(&base);
        let root = base.join("worktree");
        let dir = cache.repo_dir(&root).join("rust");
        std::fs::create_dir_all(&dir).unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writers: Vec<_> = (0..3)
            .map(|w| {
                let (dir, stop) = (dir.clone(), stop.clone());
                std::thread::spawn(move || {
                    let mut n = 0u32;
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        let mut m = meta(true, Some("abc"), Some("h"));
                        // A big, varying record makes a torn write likely to fail to parse.
                        m.stderr_tail = format!("w{w}-{n}\n").repeat(2000);
                        IndexCache::write_meta(&dir, &m)
                            .expect("a concurrent writer must not fail");
                        n += 1;
                    }
                    n
                })
            })
            .collect();
        let started = std::time::Instant::now();
        let (mut reads, mut seen) = (0u32, 0u32);
        while started.elapsed() < std::time::Duration::from_millis(700) {
            let text = std::fs::read_to_string(dir.join(META_FILE));
            reads += 1;
            if let Ok(text) = text {
                serde_json::from_str::<Meta>(&text).expect("a torn meta.json was visible");
                seen += 1;
            }
            // The cache's own loader on the same directory.
            let _ = cache.projects(&root);
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let written: u32 = writers.into_iter().map(|w| w.join().unwrap()).sum();
        assert!(
            written > 3 && seen > 0,
            "written {written}, read {seen}/{reads}"
        );
        assert_eq!(cache.projects(&root).len(), 1);
        let tmp: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(tmp.is_empty(), "{tmp:?}");
        std::fs::remove_dir_all(&base).ok();
    }
}
