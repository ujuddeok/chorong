//! Reads from the cache's object store with gix: resolving revisions, commit details, merge
//! bases, how far one commit is behind another, and file contents at a revision. A file whose
//! contents were not fetched yet (the clone is blobless) is fetched with `git cat-file`.

use crate::gitcmd::Git;
use crate::{Error, Result};
use serde::Serialize;
use std::path::Path;

fn open(repo: &Path) -> Result<gix::Repository> {
    gix::open(repo).map_err(|e| Error::msg(format!("cannot open {}: {e}", repo.display())))
}

fn oid(s: &str) -> Result<gix::ObjectId> {
    gix::ObjectId::from_hex(s.as_bytes()).map_err(|e| Error::msg(format!("`{s}`: {e}")))
}

/// The full commit id `rev` names in `repo`, if it names exactly one commit.
pub fn resolve_commit(repo: &Path, rev: &str) -> Option<String> {
    let r = open(repo).ok()?;
    let id = r.rev_parse_single(rev).ok()?;
    let commit = id.object().ok()?.peel_to_commit().ok()?;
    Some(commit.id.to_string())
}

/// The commit a hexadecimal id (full or abbreviated) names, looked up as an object id only:
/// a branch of the same name doesn't count. `None` when it is unknown, ambiguous or not a
/// commit.
pub fn commit_by_id(repo: &Path, hex: &str) -> Option<String> {
    let r = open(repo).ok()?;
    let prefix = gix::hash::Prefix::from_hex(hex).ok()?;
    let id = r.objects.lookup_prefix(prefix, None).ok()??.ok()?;
    let commit = r.find_object(id).ok()?.peel_to_commit().ok()?;
    Some(commit.id.to_string())
}

/// One commit, as shown for Before and After.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommitInfo {
    pub id: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    /// Committer time, seconds since the Unix epoch.
    pub time: i64,
}

pub fn commit_info(repo: &Path, id: &str) -> Result<CommitInfo> {
    let r = open(repo)?;
    let c = r
        .find_commit(oid(id)?)
        .map_err(|e| Error::msg(format!("commit {id}: {e}")))?;
    let subject = c
        .message()
        .map(|m| m.summary().to_string())
        .unwrap_or_default();
    let author = c.author().map(|a| a.name.to_string()).unwrap_or_default();
    let time = c.time().map(|t| t.seconds).unwrap_or(0);
    Ok(CommitInfo {
        id: id.to_string(),
        short: cho_git::short(id),
        subject,
        author,
        time,
    })
}

/// How many commits reachable from `tip` are not reachable from `from`
/// (`git rev-list --count from..tip`).
pub fn count_missing(repo: &Path, from: &str, tip: &str) -> Result<usize> {
    let r = open(repo)?;
    let walk = r
        .rev_walk([oid(tip)?])
        .with_hidden([oid(from)?])
        .all()
        .map_err(|e| Error::msg(format!("walking {from}..{tip}: {e}")))?;
    let mut n = 0;
    for info in walk {
        info.map_err(|e| Error::msg(format!("walking {from}..{tip}: {e}")))?;
        n += 1;
    }
    Ok(n)
}

/// The bytes of `path` at `rev`. When the object store doesn't have the file's contents (a
/// blobless clone), `git cat-file` fetches them from the promisor remote; that runs only in
/// Chorong's cache.
pub fn read_file_at(git: &Git, repo: &Path, rev: &str, path: &str) -> Result<Vec<u8>> {
    let r = open(repo)?;
    let id = r
        .rev_parse_single(rev)
        .map_err(|e| Error::msg(format!("`{rev}` is not in this repository: {e}")))?;
    let commit = id
        .object()
        .map_err(|e| Error::msg(e.to_string()))?
        .peel_to_commit()
        .map_err(|e| Error::msg(format!("`{rev}` is not a commit: {e}")))?;
    let tree = commit.tree().map_err(|e| Error::msg(e.to_string()))?;
    let entry = tree
        .lookup_entry_by_path(path)
        .map_err(|e| Error::msg(e.to_string()))?
        .ok_or_else(|| {
            Error::not_found(format!("{path} does not exist at {}", cho_git::short(rev)))
        })?;
    if !entry.mode().is_blob() {
        return Err(Error::unsupported(format!("{path} is not a file at {rev}")));
    }
    let blob = entry.object_id();
    match r.find_object(blob) {
        Ok(o) => Ok(o.detach().data),
        Err(_) => git
            .run_bytes(Some(repo), &["cat-file", "blob", &blob.to_string()])
            .map_err(|e| Error::msg(e.to_string())),
    }
}
