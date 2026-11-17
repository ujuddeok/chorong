//! Disk use of the repository cache, and clearing it: one worktree, one repository, or
//! everything.

use crate::paths::Cache;
use crate::state::RepoState;
use crate::{Error, Result};
use cho_core::RepoRef;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeUsage {
    pub slug: String,
    pub path: PathBuf,
    pub bytes: u64,
    /// What it was opened for, from `state.json` when known.
    pub what: Option<String>,
    pub opened: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoUsage {
    pub repo: String,
    pub bare: PathBuf,
    /// The bare clone (objects shared by every worktree).
    pub clone_bytes: u64,
    pub worktrees: Vec<WorktreeUsage>,
    pub total_bytes: u64,
}

/// The kinds of folder at the top of the cache.
const KINDS: [&str; 3] = ["github", "bitbucket", "bitbucket-dc"];

/// Every repository in the cache, sorted by identity.
pub fn list(cache: &Cache) -> Vec<RepoUsage> {
    let mut out = vec![];
    for kind in KINDS {
        let depth = if kind == "bitbucket-dc" { 2 } else { 1 };
        for (parents, bare) in bare_clones(&cache.root().join(kind), depth) {
            let name = bare
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".git"))
                .unwrap_or_default()
                .to_string();
            let spec = format!("{kind}:{}/{name}", parents.join("/"));
            let Ok(repo) = RepoRef::parse(&spec) else {
                continue;
            };
            if let Ok(u) = repo_usage(cache, &repo) {
                out.push(u);
            }
        }
    }
    out.sort_by(|a, b| a.repo.cmp(&b.repo));
    out
}

/// `<dir>/<p1>/…/<pN>/<name>.git` for `depth` = N.
fn bare_clones(dir: &Path, depth: usize) -> Vec<(Vec<String>, PathBuf)> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if depth == 0 {
            if name.ends_with(".git") {
                out.push((vec![], p));
            }
        } else {
            for (mut parents, bare) in bare_clones(&p, depth - 1) {
                parents.insert(0, name.clone());
                out.push((parents, bare));
            }
        }
    }
    out
}

pub fn repo_usage(cache: &Cache, repo: &RepoRef) -> Result<RepoUsage> {
    let paths = cache.repo(repo)?;
    let state = RepoState::load(&paths.state);
    let mut worktrees = vec![];
    if let Ok(rd) = std::fs::read_dir(&paths.worktrees) {
        for e in rd.flatten() {
            let slug = e.file_name().to_string_lossy().into_owned();
            let w = state.worktrees.get(&slug);
            worktrees.push(WorktreeUsage {
                bytes: dir_size(&e.path()),
                path: e.path(),
                what: w.map(|w| w.what.clone()),
                opened: w.map(|w| w.opened),
                slug,
            });
        }
    }
    worktrees.sort_by(|a, b| a.slug.cmp(&b.slug));
    let clone_bytes = dir_size(&paths.bare);
    let total_bytes =
        clone_bytes + worktrees.iter().map(|w| w.bytes).sum::<u64>() + file_size(&paths.state);
    Ok(RepoUsage {
        repo: repo.to_string(),
        bare: paths.bare,
        clone_bytes,
        worktrees,
        total_bytes,
    })
}

fn file_size(p: &Path) -> u64 {
    std::fs::symlink_metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// Bytes of every file under `p`, not following symbolic links.
pub fn dir_size(p: &Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(p) else {
        return 0;
    };
    if !m.is_dir() {
        return m.len();
    }
    std::fs::read_dir(p)
        .map(|rd| rd.flatten().map(|e| dir_size(&e.path())).sum())
        .unwrap_or(0)
}

/// Removes a repository's clone, worktrees and state. Returns the worktree folders that
/// were removed (callers drop their indexes).
pub fn clear_repo(cache: &Cache, repo: &RepoRef) -> Result<Vec<PathBuf>> {
    let paths = cache.repo(repo)?;
    let _lock = cache.lock(&paths, crate::open::LOCK_TIMEOUT)?;
    let removed = worktree_dirs(&paths.worktrees);
    for p in [&paths.dir, &paths.bare] {
        if p.exists() {
            cache.ensure_inside(p)?;
            std::fs::remove_dir_all(p)
                .map_err(|e| Error::msg(format!("cannot remove {}: {e}", p.display())))?;
        }
    }
    Ok(removed)
}

/// Removes every repository in the cache, one at a time under its lock. Nothing else in the
/// cache folder is touched.
pub fn clear_all(cache: &Cache) -> Result<Vec<PathBuf>> {
    let mut removed = vec![];
    for u in list(cache) {
        if let Ok(r) = RepoRef::parse(&u.repo) {
            removed.extend(clear_repo(cache, &r)?);
        }
    }
    Ok(removed)
}

/// Canonical paths of the folders in `worktrees/` (before they are removed).
pub(crate) fn worktree_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path().canonicalize().unwrap_or_else(|_| e.path()))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}
